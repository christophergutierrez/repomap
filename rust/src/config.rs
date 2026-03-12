use std::path::PathBuf;

/// Resolved configuration from environment variables and defaults.
pub struct Config {
    pub index_path: PathBuf,
    pub log_level: String,
    pub github_token: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub google_api_key: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let index_path = std::env::var("CODE_INDEX_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs_home().join(".code-index")
            });

        Self {
            index_path,
            log_level: std::env::var("REPOMAP_LOG_LEVEL")
                .unwrap_or_else(|_| "WARNING".into()),
            github_token: std::env::var("GITHUB_TOKEN").ok(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            google_api_key: std::env::var("GOOGLE_API_KEY").ok(),
        }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
