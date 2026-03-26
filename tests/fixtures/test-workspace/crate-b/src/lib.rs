use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub name: String,
    pub value: i32,
    #[serde(with = "humantime_serde")]
    pub timeout: std::time::Duration,
}

pub fn to_json(config: &Config) -> String {
    serde_json::to_string(config).unwrap()
}
