use std::{fmt::Display, fs::File, io::BufReader, net::{IpAddr as IPAddr, Ipv4Addr}, path::{Path, PathBuf}};
use std::error::Error;
use serde::{Deserialize};
use clap::{Parser};


#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Sets a custom config file
    #[arg(short = 'c', long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// directory to scan
    #[arg(short = 'd', long, value_name = "DIRECTORY")]
    directory: Option<PathBuf>,
}

#[derive(Deserialize, Debug)]
/// Configuration structure for the application
pub struct Config {
    /// The folder to scan
    pub folder_to_scan: String,
    /// Maximum number of threads to use
    pub max_thread: usize,
    /// Server IP address
    pub server_ip: IPAddr,
    /// Server port number
    pub server_port: u16,
}

/// Reads the configuration from a JSON file
/// # Arguments
/// * `path` - A path to the configuration file
/// # Returns
/// A Result containing the Config instance or an error
fn read_config_from_file<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn Error>> {
    // Open the file in read-only mode with buffer.
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    // Read the JSON contents of the file as an instance of `User`.
    let u = serde_json::from_reader(reader)?;

    // Return the `User`.
    Ok(u)
}

impl Default for Config {
    fn default() -> Self {
        Config {
            folder_to_scan : String::from(""),
            max_thread : 1,
            server_ip : IPAddr::V4(Ipv4Addr::new(127,0,0,1)),
            server_port : 8082,
        }
    }
}

impl Display for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "folder_to_scan : {}\nmax_thread : {}\nserver_ip : {}\nserver_port : {}",
            self.folder_to_scan,
            self.max_thread,
            self.server_ip,
            self.server_port)
    }
}

impl Config {
    /// Build configuration from command line arguments
    /// # Arguments
    /// * `cli` - The parsed command line arguments
    /// # Returns
    /// A Config instance
    pub fn build_config(cli : Cli) -> Result<Config, Box<dyn Error>> 
    {
        if let Some(config) = cli.config{
            let config = match read_config_from_file(config) {
                Ok(cfg) => cfg,
                Err(e) => {
                    return Err(format!("Failed to read config file: {}", e).into());
                }
            };
            Ok(config)
        } else if let Some(directory) = cli.directory {
            let mut config = Config::default();
            config.folder_to_scan = directory
                .to_str()
                .ok_or("Invalid directory path")?
                .to_string();

            Ok(config)
        } else {
            Err("No configuration file or directory provided".into())
        }
    }
}