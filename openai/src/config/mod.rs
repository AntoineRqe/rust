use std::path::{PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SupportedFormat {
    pub input: bool,
    pub output: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    pub max_threads: usize,
    pub support_csv: SupportedFormat,
    pub support_html: SupportedFormat,
    pub max_domain_propositions: usize,
    pub model: Vec<String>,
    pub chunk_size: usize,
    pub thinking_budget: i64,
    pub use_internal_replacement: bool,
    pub use_gemini_caching: bool,
    pub use_gemini_url_context: bool,
    pub use_gemini_google_search: bool,
    pub use_gemini_custom_cache_duration: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_threads: 1,
            support_csv: SupportedFormat { input: true, output: true },
            support_html: SupportedFormat { input: false, output: true },
            max_domain_propositions: 3,
            model: vec!["Qwen2.5-Coder-32B-Instruct-AWQ".to_string()],
            chunk_size: 100,
            thinking_budget: 1024,
            use_internal_replacement: false,
            use_gemini_caching: false,
            use_gemini_url_context: false,
            use_gemini_google_search: false,
            use_gemini_custom_cache_duration: None,
        }
    }
}

impl Config {
    pub fn new(config_file: Option<PathBuf>) -> Self {
        match config_file {
            Some(path) => Self::load_from_file(path),
            None => Self::default(),
        }
    }

    fn load_from_file(config_file: PathBuf) -> Self {
        let config_data = std::fs::read_to_string(config_file)
            .expect("Failed to read config file");
        serde_json::from_str(&config_data)
            .expect("Failed to parse config file")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.max_threads, 1);
        assert!(config.support_csv.input);
        assert!(config.support_csv.output);
        assert!(!config.support_html.input);
        assert!(config.support_html.output);
        assert_eq!(config.max_domain_propositions, 3);
        assert_eq!(config.model, vec!["Qwen2.5-Coder-32B-Instruct-AWQ".to_string()]);
        assert_eq!(config.chunk_size, 100);
        assert!(!config.use_internal_replacement);
        assert_eq!(config.thinking_budget, 1024);
        assert!(!config.use_gemini_caching);
        assert!(!config.use_gemini_url_context);
        assert!(!config.use_gemini_google_search);
        assert!(config.use_gemini_custom_cache_duration.is_none());
    }

    #[test]
    fn test_load_config_from_file() {
        let test_config_path = PathBuf::from("src/config/test/config.json");
        let config = Config::load_from_file(test_config_path);
        assert_eq!(config.max_threads, 1);
        assert!(config.support_csv.input);
        assert!(config.support_csv.output);
        assert!(!config.support_html.input);
        assert!(config.support_html.output);
        assert_eq!(config.max_domain_propositions, 3);
        assert_eq!(config.model[0], "Qwen2.5-Coder-32B-Instruct-AWQ".to_string());
        assert_eq!(config.model[1], "Mistral-Small".to_string());
        assert_eq!(config.model[2], "claude-sonnet-4".to_string());
        assert_eq!(config.model[3], "gemini-2.5-flash".to_string());
        assert_eq!(config.chunk_size, 50);
        assert!(config.use_internal_replacement);
        assert_eq!(config.thinking_budget, 2048);
        assert!(config.use_gemini_caching);
        assert!(config.use_gemini_url_context);
        assert!(config.use_gemini_google_search);
        assert_eq!(config.use_gemini_custom_cache_duration.unwrap(), "30s".to_string());
    }
}