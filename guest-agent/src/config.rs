use std::{collections::HashSet, path::PathBuf};

use figment::Figment;
use load_config::load_config;
use serde::Deserialize;

pub const DEFAULT_CONFIG: &str = include_str!("../dstack.toml");

pub fn load_config_figment(config_file: Option<&str>) -> Figment {
    load_config("dstack", DEFAULT_CONFIG, config_file, true)
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct BindAddr {
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub app_name: String,
    pub keys_file: String,
    pub public_logs: bool,
    pub public_sysinfo: bool,
    pub compose_file: String,
    #[serde(default)]
    pub pccs_url: Option<String>,
    pub simulator: Simulator,
    // List of disks to be shown in the dashboard
    pub data_disks: HashSet<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Simulator {
    pub enabled: bool,
    pub quote_file: String,
    pub event_log_file: String,
}
