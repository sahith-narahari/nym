use serde_derive::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(rename(deserialize = "healthcheck"))]
    pub health_check: healthcheck::config::HealthCheck,
}
