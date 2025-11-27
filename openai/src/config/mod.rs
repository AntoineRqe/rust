use std::path::{PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SupportedFormat {
    pub input: bool,
    pub output: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    pub support_multithread: bool,
    pub support_csv: SupportedFormat,
    pub support_html: SupportedFormat,
    pub max_domain_propositions: usize,
    pub model: Vec<String>,
    pub chunk_size: usize,
    pub use_internal_replacement: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            support_multithread: false,
            support_csv: SupportedFormat { input: true, output: true },
            support_html: SupportedFormat { input: false, output: true },
            max_domain_propositions: 3,
            model: vec!["Qwen2.5-Coder-32B-Instruct-AWQ".to_string()],
            chunk_size: 100,
            use_internal_replacement: false,
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
        assert!(!config.support_multithread);
        assert!(config.support_csv.input);
        assert!(config.support_csv.output);
        assert!(!config.support_html.input);
        assert!(config.support_html.output);
        assert_eq!(config.max_domain_propositions, 3);
        assert_eq!(config.model, vec!["Qwen2.5-Coder-32B-Instruct-AWQ".to_string()]);
        assert_eq!(config.chunk_size, 100);
        assert!(!config.use_internal_replacement);
    }

    #[test]
    fn test_load_config_from_file() {
        let test_config_path = PathBuf::from("src/config/test/config.json");
        let config = Config::load_from_file(test_config_path);
        assert!(!config.support_multithread);
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
    }
}