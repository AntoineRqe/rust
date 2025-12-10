use futures::io;
use indexmap::IndexMap;

use crate::statistics::Statistics;
use clap::Parser;
use std::path::PathBuf;

mod category;
pub mod my_traits;
pub mod format;
mod statistics;
mod config;
mod ctx;
mod llm;
mod utils;

use llm::core::{generate_full_prompt, generate_prompt_with_cached_content, sync_llm_runtime};

#[derive(Debug, Clone)]
pub struct CatVisionData {
    pub category_olfeo: Option<String>,
    pub categories_manual: Option<String>,
    pub categories_llm: Option<Vec<String>>,
    pub appsite_name_by_olfeo: Option<String>,
    pub appsite_name_by_gemini: Option<String>,
}

impl CatVisionData {
    pub fn new(
        category_olfeo: Option<String>,
        categories_manual: Option<String>,
        categories_llm: Option<Vec<String>>,
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

fn aggregate_data(
    original_data: &indexmap::IndexMap<String, CatVisionData>,
    llm_data: &std::collections::HashMap<String, Vec<String>>,
    stats: &mut Statistics,
    nb_propositions: usize,
) -> indexmap::IndexMap<String, CatVisionData> {
    let mut aggregated = indexmap::IndexMap::new();

    for (domain, original_categories) in original_data {
        stats.increment_domain_count();

        // Start with original categories
        let mut tmp_categories: CatVisionData = CatVisionData::new(
            original_categories.category_olfeo.clone(),
            original_categories.categories_manual.clone(),
            None,
            original_categories.appsite_name_by_olfeo.clone(),
            original_categories.appsite_name_by_gemini.clone(),
        );

        if let Some(categories) = llm_data.get(domain) {
            // Process LLM categories
            let mut llm_categories = vec![];

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
                        cat.clone()
                    }
                    None => "".to_string(),
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
    let args = Args::parse();
    let input_file = PathBuf::from(&args.input);
    let config_path = args.config.map(PathBuf::from);
    let dict = args.dict.map(PathBuf::from);

    let mut ctx = ctx::Ctx::new(&input_file, config_path, dict);

    let start_time = std::time::Instant::now();

    let domains = ctx
    .parse()
    .expect("Failed to parse input CSV")
    .downcast::<IndexMap<String, CatVisionData>>()
    .expect("Failed to downcast parsed data to IndexMap<String, CatVisionData>>");
    
    let domains_name = domains.keys().cloned().collect::<Vec<String>>();
 
    ctx.prompt = if ctx.config.use_gemini_explicit_caching {
        generate_prompt_with_cached_content(&domains_name)
    } else {
        generate_full_prompt(&domains_name, ctx.config.max_domain_propositions)
    };

    let llm_results = match sync_llm_runtime(&domains_name, &ctx) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Error during LLM processing: {}", e);
            return Err(io::Error::new(io::ErrorKind::Other, "LLM processing failed"));
        }
    };

    ctx.stats.update_llm_statistics(llm_results.cost, llm_results.retried, llm_results.failed, ctx.config.chunk_size, ctx.config.thinking_budget);

    let aggregated = aggregate_data(&domains, &llm_results.categories, &mut ctx.stats, ctx.config.max_domain_propositions);

    ctx.stats.elapsed_time = start_time.elapsed();

    println!("Classification of {} domains finished in {:?} for {}€", domains.len(), ctx.stats.elapsed_time, llm_results.cost);

    ctx.write(&aggregated).expect("Failed to write output data");

    Ok(())
}