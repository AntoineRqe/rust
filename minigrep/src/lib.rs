use std::error::Error;
use std::fs;

pub struct Config {
    query: String,
    filename: String,
    ignore_case: bool,
}

impl Config
{
    pub fn build_config(args: &[String]) -> Result<Config, &'static str> {
        if args.len() < 3 {
            return Err("Not enough arguments");
        }

        let query = args[1].clone();
        let filename = args[2].clone();

        Ok(Config { query, filename , ignore_case: std::env::var("IGNORE_CASE").is_ok() } )
    }
}

pub fn search<'a>(query: &str, contents: &'a str) -> Vec<&'a str> {

    contents.lines()
    .filter(|line| line.contains(query))
    .collect()
}

pub fn search_case_insensitive<'a>(query: &str, contents: &'a str) -> Vec<&'a str> {

    contents.lines()
    .filter(|line| line.to_lowercase().contains(&query.to_lowercase()))
    .collect()
}

pub fn run(config: Config) -> Result<(), Box<dyn Error>> {
    println!("Searching for {}", config.query);
    println!("In file {}", config.filename);

    let contents = fs::read_to_string(config.filename)?;

    let results = if config.ignore_case {
        search_case_insensitive(&config.query, &contents)
    } else {
        search(&config.query, &contents)
    };

    for line in results {
        println!("{line}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_sensitive() {
        let query = "duct";
        let contents = "\
Rust:
safe, fast, productive.
Pick three.
Duct tape.";

        assert_eq!(vec!["safe, fast, productive."], search(query, contents));
    }

    #[test]
    fn case_insensitive() {
        let query = "rUsT";
        let contents = "\
Rust:
safe, fast, productive.
Pick three.
Trust me.";

        assert_eq!(
            vec!["Rust:", "Trust me."],
            search_case_insensitive(query, contents)
        );
    }
}