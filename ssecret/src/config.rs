
pub struct Config {
    pub folder_to_scan: String,
    max_thread: u32,
}

impl Config {
    pub fn build_config(args: Vec<String>) -> Result<Config, &'static str> {
        if args.len() < 2 {
            return Err("Not enough arguments!");
        }

        Ok(Config { folder_to_scan: args[1].clone(), max_thread: 1 })
    }
}

#[cfg(test)]
mod tests {

    use crate::Config;
    #[test]
    fn test_build_config() {
        let args: Vec<String> = vec!["hello".to_string(), "/mnt/test".to_string()];
        let config : Config = Config::build_config(args).unwrap();

        assert_eq!(config.folder_to_scan, "/mnt/test");
        assert_eq!(config.max_thread, 1);
    }
}