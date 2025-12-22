use futures::io;
use indexmap::IndexMap;

use crate::statistics::Statistics;
use clap::Parser;
use std::{path::PathBuf};
use std::collections::HashMap;

mod category;
pub mod my_traits;
pub mod format;
mod statistics;
mod config;
mod ctx;
mod llm;
mod utils;

use llm::core::{generate_full_prompt, generate_prompt_with_cached_content, sync_llm_runtime};
use utils::seconds_to_pretty;

#[derive(Debug, Clone)]
/// Structure to hold various categories data for a domain
pub struct CatVisionData {
    pub category_olfeo: Option<&'static str>,
    pub categories_manual: Option<&'static str>,
    pub categories_llm: Option<Vec<&'static str>>,
    pub appsite_name_by_olfeo: Option<String>,
    pub appsite_name_by_gemini: Option<String>,
}

impl CatVisionData {
    pub fn new(
        category_olfeo: Option<&'static str>,
        categories_manual: Option<&'static str>,
        categories_llm: Option<Vec<&'static str>>,
        appsite_name_by_olfeo: Option<String>,
        appsite_name_by_gemini: Option<String>) -> Self {
        
        CatVisionData {
            category_olfeo,
            categories_manual,
            categories_llm,
            appsite_name_by_olfeo,
            appsite_name_by_gemini,
        }
    }
}

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
    #[arg(short, long)]
    config: Option<String>,
    #[arg(short, long)]
    dict: Option<String>,
}

fn main() -> io::Result<()> {
    // Parse command-line arguments
    let args = Args::parse();
    let input_file = PathBuf::from(&args.input);
    let config_path = args.config.map(PathBuf::from);
    let dict = args.dict.map(PathBuf::from);

    // Initialize context wihth input file and optional config and dictionary
    let mut ctx = ctx::Ctx::new(&input_file, config_path, dict);

    let start_time = std::time::Instant::now();

    // Parse input data
    let domains = ctx
        .parse()
        .expect("Failed to parse input CSV")
        .downcast::<IndexMap<String, CatVisionData>>()
        .expect("Failed to downcast parsed data to IndexMap<String, CatVisionData>>");
    
    // Create domais name list from input file
    let domains_name = domains
        .keys()
        .map(|s| s.as_str())
        .collect::<Vec<&str>>();
 
    // Generate prompt and call LLM based on caching configuration for Gemini
    ctx.prompt = if ctx.config.use_gemini_explicit_caching {
        generate_prompt_with_cached_content(&domains_name)
    } else {
        generate_full_prompt(&domains_name, ctx.config.max_domain_propositions)
    };

    // Calling Gemini LLM synchronously to get categories
    let llm_results = match sync_llm_runtime(&domains_name, &ctx) {
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