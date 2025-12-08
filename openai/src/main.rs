use futures::io;
use indexmap::IndexMap;

use crate::statistics::Statistics;
use clap::Parser;
use std::path::PathBuf;

mod category;
pub mod my_traits;
pub mod csv;
mod statistics;
mod html;
mod config;
mod ctx;
mod llm;

use llm::core::{generate_full_prompt, generate_prompt_with_cached_content, sync_llm_runtime};

#[derive(Debug, Clone)]
pub struct CatVisionData {
    pub category_olfeo: Option<String>,
    pub categories_manual: Option<String>,
    pub categories_llm: Option<Vec<String>>,
    pub prioritized_category: Option<String>,
    pub appsite_name: Option<String>,
}

impl CatVisionData {
    pub fn new(category_olfeo: Option<String>, categories_manual: Option<String>, categories_llm: Option<Vec<String>>, prioritized_category: Option<String>, appsite_name: Option<String>) -> Self {
        CatVisionData {
            category_olfeo,
            categories_manual,
            categories_llm,
            prioritized_category,
            appsite_name,
        }
    }
}

fn aggregate_data(
    original_data: &indexmap::IndexMap<String, CatVisionData>,
    llm_data: &std::collections::HashMap<String, Vec<String>>,
    stats: &mut Statistics,
    use_internal_replacement: bool,
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
            None,
            original_categories.appsite_name.clone(),
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

            // Determine prioritized category
            let mut prioritized_cat = if use_internal_replacement {
                if categories.len() > 1 {
                    match category::get_prioritized_category(&categories[0], &vec![categories[1].clone()]) {
                        Some(prioritized) => {
                            stats.increment_priorized_done_count();
                            if expected_category.contains(&prioritized) {
                                stats.increment_prioritized_done_success();
                            }
                            prioritized.to_string()
                        }
                        None => categories.get(0).cloned().unwrap_or_default(),
                    }
                } else {
                    categories.get(0).cloned().unwrap_or_default()
                }
            } else {
                categories.get(0).cloned().unwrap_or_default()
            };

            // Mark if prioritized category mismatches
            if !expected_category.contains(&prioritized_cat) {
                prioritized_cat.push_str("*RED*");
            } else {
                stats.increment_prioritized_match_count();
            }

            tmp_categories.prioritized_category = Some(prioritized_cat);
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
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let input_file = PathBuf::from(&args.input);
    let config_path = args.config.map(PathBuf::from);

    let mut ctx = ctx::Ctx::new(&input_file, config_path);

    let start_time = std::time::Instant::now();

    let domains = ctx.parse().expect("Failed to parse input CSV");

    let domains = domains.downcast_ref::<IndexMap<String, CatVisionData>>().expect("Failed to downcast parsed data to IndexMap<String, Vec<CatVisionData>>");
    
    let domains_name = domains.keys().cloned().collect::<Vec<String>>();
    
    ctx.prompt = if ctx.config.use_gemini_explicit_caching {
        generate_prompt_with_cached_content(&domains_name)
    } else {
        generate_full_prompt(&domains_name, ctx.config.max_domain_propositions)
    };

    let llm_results = sync_llm_runtime(&domains_name, &ctx).expect("Failed to get LLM results");

    ctx.stats.update_llm_statistics(llm_results.cost, llm_results.retried, llm_results.failed, ctx.config.chunk_size, ctx.config.thinking_budget);

    println!("LLM processing completed.");

    let aggregated = aggregate_data(&domains, &llm_results.categories, &mut ctx.stats, ctx.config.use_internal_replacement, ctx.config.max_domain_propositions);

    let duration = start_time.elapsed();
    println!("Time elapsed {:?}", duration);
    ctx.stats.elapsed_time = duration;

    ctx.write(&aggregated).expect("Failed to write output data");



    Ok(())
}