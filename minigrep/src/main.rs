use std::env;
use std::process;

use minigrep::Config;


fn main() {
    let args: Vec<String> = env::args().collect();

    let config = Config::build_config(&args).unwrap_or_else(|err| {
        eprintln!("Error: {err}");
        process::exit(1);
    });

    if let Err(e) = minigrep::run(config) {
        eprintln!("Application error: {e}");
        process::exit(1);
    }    
}