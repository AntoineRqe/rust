use futures::io;
use indexmap::IndexMap;

use crate::my_traits::{Input, Output};
use crate::csv::{MyCSVInput, MyCSVOutput};
use crate::statistics::Statistics;
use std::path::Path;
use clap::Parser;
use std::path::PathBuf;


mod llm;
mod category;
pub mod my_traits;
pub mod csv;
mod statistics;
mod html;
mod config;

use config::Config;


struct Ctx
{   
    input_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
    inputs: Vec<Box<dyn Input>>,
    outputs: Vec<Box<dyn Output>>,
    stats: Statistics,
    config: Config,
    prompt: String,
}

fn extract_directory_from_path(file_path: &Path) -> Option<PathBuf> {
    file_path.parent().map(|parent| parent.to_path_buf())
}

impl Ctx
{
    fn new(input_path: &Path, config: Option<PathBuf>) -> Self {
        let config = Config::new(config);
        
        let mut ctx = Ctx {
            input_path: input_path.to_path_buf(),
            inputs: vec![],
            output_path: extract_directory_from_path(input_path).unwrap_or_else(|| PathBuf::from("/outputs/")).join("outputs"),
            outputs: vec![],
            stats: Statistics::new(config.max_concurrent_requests),
            config: config,
            prompt: String::from("")   
        };

        if ctx.config.support_csv.input {
            println!("CSV input is enabled.");
            let input = MyCSVInput::new(&ctx.input_path);
            ctx.inputs.push(Box::new(input));
        }

        if ctx.config.support_csv.output {
            println!("CSV output is enabled.");
            let output = MyCSVOutput::new(&ctx.output_path.join(ctx.input_path.file_name().unwrap()).with_extension(format!("{}_{}.csv", ctx.config.model[0], "csv")));
            ctx.outputs.push(Box::new(output));
        }
        
        if ctx.config.support_html.input {
            println!("HTML input is enabled but not supported.");
        }

        if ctx.config.support_html.output {
            println!("HTML output is enabled.");
            let output = html::HTMLGenerator::new(&ctx.output_path.join(ctx.input_path.file_name().unwrap()).with_extension(format!("{}_{}.html", ctx.config.model[0], "html")));
            ctx.outputs.push(Box::new(output));
        }

        return ctx;
    }

    fn write(&mut self, data: &dyn std::any::Any) -> Result<(), Box<dyn std::error::Error>> {
        let infos = my_traits::Infos::new(
            
            &self.output_path.join("my_reference_").join(self.config.model[0].as_str()).with_extension("html"),
            &(self.config.model[0].clone() + " LLM Classification Results for " + &self.input_path.to_string_lossy()),
            &self.stats.generate_output_summary(),
            &self.prompt,
            self.config.max_concurrent_requests,
        );
    
        for output in &mut self.outputs {
            if let Err(e) = output.write(data, &infos) {
                eprintln!("Error writing output: {}", e);
            }
        }
        Ok(())
    }

    fn parse(&mut self) -> Result<Box<dyn std::any::Any>, Box<dyn std::error::Error>> {
        for input in &mut self.inputs {
            return input.parse(&mut self.stats);
        }
        Err("No input available".into())
    }
}

fn aggregate_data(original_data: &indexmap::IndexMap<String, Vec<String>>,
                llm_data: &std::collections::HashMap<String, Vec<String>>,
                stats: &mut Statistics) -> IndexMap<String, Vec<String>> {

    let mut aggregated = IndexMap::new();

    for (domain, original_categories) in original_data {
        stats.increment_domain_count();
        let mut tmp_categories: Vec<String> = vec![];

        let expected_category = &original_categories[0];

        for original_category in original_categories {
            tmp_categories.push(original_category.clone());
        }

        if let Some(categories) = llm_data.get(domain) {
            for (level, cat) in categories.iter().enumerate() {
                if expected_category.contains(cat) {
                    tmp_categories.push(cat.clone());
                    stats.increment_llm_level_match_count(level);
                } else {
                    // No match at this level
                    let cat = &(cat.clone() + "*RED*");
                    tmp_categories.push(cat.clone());
                }
            }

            let mut prioritized_cat: String;
        
            if let Some(prioritized_category) = category::get_prioritized_category(domain, &categories[0], &vec![categories[1].clone()]) {
                stats.increment_priorized_done_count();
                prioritized_cat = prioritized_category.to_string();
                if expected_category.contains(&prioritized_cat) {
                    stats.increment_prioritized_done_success();
                }
            } else {
                prioritized_cat = categories[0].clone();
            }

            if expected_category.contains(&prioritized_cat) {
                stats.increment_prioritized_match_count();
            } else {
                prioritized_cat.push_str("*RED*");
            }

            tmp_categories.push(prioritized_cat);
        }

        aggregated.insert(domain.clone(), tmp_categories);
    }

    return aggregated;
}

// Define the command-line arguments using `clap::Parser`
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input file path
    #[arg(short, long)]
    input: String,
    #[arg(short, long)]
    config: Option<String>,
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let input_file = PathBuf::from(&args.input);
    let config_path = args.config.map(PathBuf::from);

    let mut ctx = Ctx::new(&input_file, config_path);

    let start_time = std::time::Instant::now();

    let domains = ctx.parse().expect("Failed to parse input CSV");
    let domains = domains.downcast_ref::<IndexMap<String, Vec<String>>>().expect("Failed to downcast parsed data to IndexMap<String, Vec<String>>");
    let domains_name = domains.keys().cloned().collect::<Vec<String>>();
    let mut llm_results: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for chunk in domains_name.chunks(1) {
        let domains_name = chunk.to_vec();
        println!("Processing LLM for {} domains : {}", domains_name.len(), domains_name.join(", "));
        ctx.prompt = llm::generate_prompt(&domains_name, ctx.config.max_concurrent_requests);
        llm_results.extend(llm::sync_llm_runtime(domains_name, ctx.config.model[0].clone(), ctx.config.support_multithread, ctx.config.max_concurrent_requests));
    }

    println!("LLM processing completed.");
    println!("{:?}", llm_results);

    let aggregated = aggregate_data(&domains, &llm_results, &mut ctx.stats);

    println!("Data aggregation completed : {} domains processed.", aggregated.len());
    // println!("Statistics:\n{}", ctx.stats);

    ctx.write(&aggregated).expect("Failed to write output CSV");

    let duration = start_time.elapsed();
    println!("Time elapsed {:?}", duration);

    Ok(())
}