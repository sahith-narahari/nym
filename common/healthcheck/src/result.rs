use crate::path_check::{PathChecker, PathStatus};
use crate::score::NodeScore;
use crypto::identity::{DummyMixIdentityKeyPair, MixnetIdentityKeyPair};
use log::{debug, error, info, warn};
use sphinx::route::NodeAddressBytes;
use std::collections::HashMap;
use std::fmt::{Error, Formatter};
use std::time::Duration;
use topology::NymTopology;

#[derive(Debug)]
pub struct HealthCheckResult(Vec<NodeScore>);

impl std::fmt::Display for HealthCheckResult {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "NETWORK HEALTH\n==============\n")?;
        self.0
            .iter()
            .for_each(|score| write!(f, "{}\n", score).unwrap());
        Ok(())
    }
}

impl HealthCheckResult {
    pub fn sort_scores(&mut self) {
        self.0.sort();
    }

    fn zero_score<T: NymTopology>(topology: T) -> Self {
        warn!("The network is unhealthy, could not send any packets - returning zero score!");
        let mixes = topology.get_mix_nodes();
        let providers = topology.get_mix_provider_nodes();

        let health = mixes
            .into_iter()
            .map(|node| NodeScore::from_mixnode(node))
            .chain(
                providers
                    .into_iter()
                    .map(|node| NodeScore::from_provider(node)),
            )
            .collect();

        HealthCheckResult(health)
    }

    // TODO: that is O(n) so maybe not the most efficient considering it will be called n times...
    fn node_score(&self, node_key: NodeAddressBytes) -> Option<f64> {
        self.0
            .iter()
            .find(|&node_score| node_score.pub_key() == node_key)
            .map(|node| node.score())
    }

    pub fn filter_topology_by_score<T: NymTopology>(
        &self,
        topology: &T,
        score_threshold: f64,
    ) -> T {
        let filtered_mix_nodes = topology
            .get_mix_nodes()
            .into_iter()
            .filter(|node| {
                match self.node_score(NodeAddressBytes::from_b64_string(node.pub_key.clone())) {
                    None => {
                        error!("Unknown node in topology - {:?}", node);
                        false
                    }
                    Some(score) => score > score_threshold,
                }
            })
            .collect();

        let filtered_provider_nodes = topology
            .get_mix_provider_nodes()
            .into_iter()
            .filter(|node| {
                match self.node_score(NodeAddressBytes::from_b64_string(node.pub_key.clone())) {
                    None => {
                        error!("Unknown node in topology - {:?}", node);
                        false
                    }
                    Some(score) => score > score_threshold,
                }
            })
            .collect();
        // coco nodes remain unchanged as no healthcheck is being run on them or time being
        let filtered_coco_nodes = topology.get_coco_nodes();

        T::new_from_nodes(
            filtered_mix_nodes,
            filtered_provider_nodes,
            filtered_coco_nodes,
        )
    }

    pub async fn calculate<T: NymTopology>(
        topology: T,
        iterations: usize,
        resolution_timeout: Duration,
    ) -> Self {
        // currently healthchecker supports only up to 255 iterations - if we somehow
        // find we need more, it's relatively easy change
        assert!(iterations <= 255);

        let all_paths = match topology.all_paths() {
            Ok(paths) => paths,
            Err(_) => return Self::zero_score(topology),
        };

        // create entries for all nodes
        let mut score_map = HashMap::new();
        topology.get_mix_nodes().into_iter().for_each(|node| {
            score_map.insert(node.get_pub_key_bytes(), NodeScore::from_mixnode(node));
        });

        topology
            .get_mix_provider_nodes()
            .into_iter()
            .for_each(|node| {
                score_map.insert(node.get_pub_key_bytes(), NodeScore::from_provider(node));
            });

        let ephemeral_keys = DummyMixIdentityKeyPair::new();
        let providers = topology.get_mix_provider_nodes();

        let mut path_checker = PathChecker::new(providers, ephemeral_keys).await;
        for i in 0..iterations {
            debug!("running healthcheck iteration {} / {}", i + 1, iterations);
            for path in &all_paths {
                path_checker.send_test_packet(&path, i as u8).await;
                // increase sent count for each node
                for node in path {
                    let current_node_score = score_map.get_mut(&node.pub_key.0).unwrap();
                    current_node_score.increase_sent_packet_count();
                }
            }
        }

        info!(
            "waiting {:?} for pending requests to resolve",
            resolution_timeout
        );
        tokio::time::delay_for(resolution_timeout).await;
        path_checker.resolve_pending_checks().await;

        let all_statuses = path_checker.get_all_statuses();
        for (path_key, status) in all_statuses.into_iter() {
            let node_keys = PathChecker::path_key_to_node_keys(path_key);
            for node in node_keys {
                if status == PathStatus::Healthy {
                    let current_node_score = score_map.get_mut(&node).unwrap();
                    current_node_score.increase_received_packet_count();
                }
            }
        }

        HealthCheckResult(score_map.into_iter().map(|(_, v)| v).collect())
    }
}
