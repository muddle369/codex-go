use std::path::PathBuf;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayEnvironmentReport {
    pub proxy_variables: Vec<String>,
    pub codex_env_path: String,
    pub codex_env_exists: bool,
    pub clash_verge_candidates: Vec<String>,
}

pub fn inspect_relay_environment() -> RelayEnvironmentReport {
    let proxy_variables = std::env::vars()
        .filter(|(name, _)| matches!(name.to_ascii_uppercase().as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY"))
        .map(|(name, _)| name)
        .collect();
    let home = crate::relay_config::default_codex_home_dir();
    let env_path = home.join(".env");
    let clash_verge_candidates = [
        PathBuf::from("/Applications/Clash Verge Rev.app"),
        directories::BaseDirs::new().map(|dirs| dirs.home_dir().join("AppData/Local/Clash Verge Rev")).unwrap_or_default(),
    ].into_iter().filter(|path| path.exists()).map(|path| path.to_string_lossy().to_string()).collect();
    RelayEnvironmentReport { proxy_variables, codex_env_path: env_path.to_string_lossy().to_string(), codex_env_exists: env_path.exists(), clash_verge_candidates }
}
