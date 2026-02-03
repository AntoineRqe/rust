use futures::io;
use indexmap::IndexMap;

use statistics::Statistics;
use clap::Parser;
use std::{path::PathBuf};
use std::collections::HashMap;
use llm::core::{sync_llm_runtime};
use llm::core::LLMCommand;
use utils::seconds_to_pretty;
use utils::CatVisionData;
use core::Ctx;

/// Aggregates original data with LLM results into a single IndexMap
///
/// # Arguments
///
/// * `original_data` - Reference to the original data map
/// * `llm_data` - Reference to the LLM results map
/// * `stats` - Mutable reference to statistics object for updating stats
/// * `nb_propositions` - Number of LLM category propositions to consider
///
/// # Returns
/// * An IndexMap containing the aggregated data
///
fn aggregate_data(
    original_data: indexmap::IndexMap<String, CatVisionData>,
    llm_data: HashMap<String, Vec<&'static str>>,
    stats: &mut Statistics,
    nb_propositions: usize,
) -> indexmap::IndexMap<String, CatVisionData> {
    let mut aggregated = indexmap::IndexMap::new();

    for (domain, original_categories) in original_data {
        stats.increment_domain_count();

        // Start with original categories
        let mut tmp_categories: CatVisionData = CatVisionData::new(
            original_categories.category_olfeo,
            original_categories.categories_manual,
            None,
            original_categories.appsite_name_by_olfeo,
            original_categories.appsite_name_by_gemini,
            original_categories.description_fr_by_gemini,
            original_categories.description_en_by_gemini,
        );

        if let Some(categories) = llm_data.get(domain.as_str()) {
            // Process LLM categories
            let mut llm_categories = Vec::with_capacity(nb_propositions);

            let expected_category = match &original_categories.categories_manual {
                Some(cat) => cat,
                None => "",
            };
        
            for level in 0..nb_propositions {
                let cat_to_push = match categories.get(level) {
                    Some(cat) => {
                        if expected_category.contains(cat) {
                            stats.increment_llm_level_match_count(level);
                        }
                        *cat
                    }
                    None => "",
                };

                llm_categories.push(cat_to_push);
            }

            tmp_categories.categories_llm = if llm_categories.is_empty() { None } else { Some(llm_categories) };
        }

        aggregated.insert(domain.clone(), tmp_categories);
    }

    aggregated
}

// Define the command-line arguments using `clap::Parser`
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input file path
    #[arg(short, long)]
    input: String,
    #[arg(long)]
    config: Option<String>,
    #[arg(long)]
    dict: Option<String>,
    #[arg(long)]
    command: String,
}

fn process_classification(
    input_file: PathBuf,
    config_path: Option<PathBuf>,
    dict: Option<PathBuf>)
     -> io::Result<()> 
     {

    // Initialize context wihth input file and optional config and dictionary
    let mut ctx = Ctx::new(&input_file, config_path, dict);

    // Parse input data
    let domains = ctx
        .parse()
        .expect("Failed to parse input CSV")
        .downcast::<IndexMap<String, CatVisionData>>()
        .expect("Failed to downcast parsed data to IndexMap<String, CatVisionData>>");
    
    // Create domais name list from input file
    let domains_name = domains
        .keys()
        .map(|s| s.to_string())
        .collect::<Vec<String>>();

    let start_time = std::time::Instant::now();
    // Generate prompt and call LLM based on caching configuration for Gemini

    // Calling Gemini LLM synchronously to get categories
    let llm_results = match sync_llm_runtime(domains_name, &ctx.config, LLMCommand::CategorizeDomains) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Error during LLM processing: {}", e);
            return Err(io::Error::new(io::ErrorKind::Other, "LLM processing failed"));
        }
    };

    // Update statistics based on Gemini results
    ctx.stats.update_llm_statistics(
        llm_results.processed,
        llm_results.cost,
        llm_results.retried,
        llm_results.failed,
        ctx.config.chunk_size,
        ctx.config.thinking_budget
    );

    // Aggregate original data with LLM results
    let aggregated = aggregate_data(*domains, llm_results.categories, &mut ctx.stats, ctx.config.max_domain_propositions);

    ctx.stats.elapsed_time = start_time.elapsed();

    println!("Classification of {} domains finished in {} for {}€",
        ctx.stats.processed,
        seconds_to_pretty(ctx.stats.elapsed_time.as_secs()).unwrap(),
        ctx.stats.cost
    );

    // Write categories to output files (HTML, CSV, JSON...)
    ctx.write(&aggregated).expect("Failed to write output data");

    Ok(())
}

fn process_description(
    input_file: PathBuf,
    config_path: Option<PathBuf>,
    dict: Option<PathBuf>,
) -> io::Result<()> {
      // Initialize context wihth input file and optional config and dictionary
    let mut ctx = Ctx::new(&input_file, config_path, dict);

    // Parse input data
    let domains = ctx
        .parse()
        .expect("Failed to parse input CSV")
        .downcast::<IndexMap<String, CatVisionData>>()
        .expect("Failed to downcast parsed data to IndexMap<String, CatVisionData>>");
    
    // Create domais name list from input file
    let domains_name = domains
        .keys()
        .map(|s| s.to_string())
        .collect::<Vec<String>>();

    let start_time = std::time::Instant::now();
    // Generate prompt and call LLM based on caching configuration for Gemini
    ctx.prompt = String::new();

    // Calling Gemini LLM synchronously to get categories
    let llm_results = match sync_llm_runtime(domains_name, &ctx.config, LLMCommand::DescribeDomains) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Error during LLM processing: {}", e);
            return Err(io::Error::new(io::ErrorKind::Other, "LLM processing failed"));
        }
    };

    // Update statistics based on Gemini results
    ctx.stats.update_llm_statistics(
        llm_results.processed,
        llm_results.cost,
        llm_results.retried,
        llm_results.failed,
        ctx.config.chunk_size,
        ctx.config.thinking_budget
    );


    println!("{:?}", llm_results.descriptions);

    ctx.stats.elapsed_time = start_time.elapsed();

    println!("Descriptions of {} domains finished in {} for {}€",
        ctx.stats.processed,
        seconds_to_pretty(ctx.stats.elapsed_time.as_secs()).unwrap(),
        ctx.stats.cost
    );

    let _ = write_descriptions_to_file(&llm_results.descriptions, "domains.json");


    // Write categories to output files (HTML, CSV, JSON...)
    // ctx.write(&aggregated).expect("Failed to write output data");

    Ok(())
}

fn write_descriptions_to_file(
    descriptions: &HashMap<String, HashMap<&str, String>>,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::Write;

    // Open file for writing
    let mut file = File::create(path)?;

    // Serialize the HashMap to pretty JSON
    let json = serde_json::to_string_pretty(&descriptions)?;

    // Write to the file
    file.write_all(json.as_bytes())?;

    Ok(())
}

fn main() -> io::Result<()> {
    // Parse command-line arguments
    let args = Args::parse();
    let input_file = PathBuf::from(&args.input);
    let config_path = args.config.map(PathBuf::from);
    let dict = args.dict.map(PathBuf::from);
    let command = args.command.as_str();
 
    match command {
        "classify" => {
            process_classification(input_file, config_path, dict)?;
            Ok(())
        },
        "describe" => {
            process_description(input_file, config_path, dict)?;
            Ok(())
        },
        _ => {
            eprintln!("Unsupported command: {}", command);
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unsupported command"));
        }
    }


}