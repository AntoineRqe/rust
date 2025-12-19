use std::path::{PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
/// Supported format for input and output
pub struct SupportedFormat {
    /// Whether the format is supported for input
    pub input: bool,
    /// Whether the format is supported for output
    pub output: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
/// Configuration for the CatVision application
pub struct Config {
    /// Maximum number of threads to use
    pub max_threads: usize,
    /// Supported CSV format for input and output
    pub support_csv: SupportedFormat,
    /// Supported HTML format for input and output
    pub support_html: SupportedFormat,
    /// Maximum number of domain propositions to consider
    pub max_domain_propositions: usize,
    /// Models to use for LLM classification
    pub model: Vec<String>,
    /// Size of chunks to process
    pub chunk_size: usize,
    /// Thinking budget for the LLM
    pub thinking_budget: i64,
    /// Whether to use explicit caching for Gemini
    pub use_gemini_explicit_caching: bool,
    /// Whether to use URL context for Gemini
    pub use_gemini_url_context: bool,
    /// Whether to use Google search for Gemini
    pub use_gemini_google_search: bool,
    /// Custom cache duration for Gemini
    pub use_gemini_custom_cache_duration: Option<String>,
}

/// Default configuration values
impl Default for Config {
    /// Provides default configuration settings
    fn default() -> Self {
        Self {
            max_threads: 1,
            support_csv: SupportedFormat { input: true, output: true },
            support_html: SupportedFormat { input: false, output: true },
            max_domain_propositions: 3,
            model: vec!["Qwen2.5-Coder-32B-Instruct-AWQ".to_string()],
            chunk_size: 100,
            thinking_budget: 1024,
            use_gemini_explicit_caching: false,
            use_gemini_url_context: false,
            use_gemini_google_search: false,
            use_gemini_custom_cache_duration: None,
        }
    }
}

/// Methods for the Config struct
impl Config {

    /// Creates a new Config instance, loading from a file if provided
    ///
    /// # Arguments
    ///
    /// * `config_file` - Optional path to a JSON configuration file
    ///
    /// # Errors
    ///
    /// Panics if the configuration file cannot be read or parsed
    pub fn new(config_file: Option<PathBuf>) -> Self {
        match config_file {
            Some(path) => Self::load_from_file(path),
            None => Self::default(),
        }
    }

    /// Loads configuration from a JSON file
    ///
    /// # Arguments
    ///
    /// * `config_file` - Path to JSON configuration file.
    ///
    /// # Errors
    ///
    /// Panics if the file cannot be read or parsed.
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
        assert_eq!(config.thinking_budget, 1024);
        assert!(!config.use_gemini_explicit_caching);
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
        assert_eq!(config.thinking_budget, 2048);
        assert!(config.use_gemini_explicit_caching);
        assert!(config.use_gemini_url_context);
        assert!(config.use_gemini_google_search);
        assert_eq!(config.use_gemini_custom_cache_duration.unwrap(), "3600s".to_string());
    }
}