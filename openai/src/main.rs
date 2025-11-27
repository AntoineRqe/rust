use futures::io;
use indexmap::IndexMap;

use crate::statistics::Statistics;
use clap::Parser;
use std::path::PathBuf;


mod llm;
mod gemini;
mod category;
pub mod my_traits;
pub mod csv;
mod statistics;
mod html;
mod config;
mod ctx;


fn aggregate_data(original_data: &indexmap::IndexMap<String, Vec<String>>,
                llm_data: &std::collections::HashMap<String, Vec<String>>,
                stats: &mut Statistics, use_internal_replacement: bool) -> IndexMap<String, Vec<String>> {

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
                if cat.is_empty() {
                    tmp_categories.push(cat.clone());
                } else if expected_category.contains(cat) {
                    tmp_categories.push(cat.clone());
                    stats.increment_llm_level_match_count(level);
                } else {
                    // No match at this level
                    let cat = &(cat.clone() + "*RED*");
                    tmp_categories.push(cat.clone());
                }
            }

            let mut prioritized_cat: String;

            if use_internal_replacement {
                if let Some(prioritized_category) = category::get_prioritized_category(&categories[0], &vec![categories[1].clone()]) {
                    stats.increment_priorized_done_count();
                    prioritized_cat = prioritized_category.to_string();
                    if expected_category.contains(&prioritized_cat) {
                        stats.increment_prioritized_done_success();
                    }
                } else {
                    prioritized_cat = categories[0].clone();
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

    let mut ctx = ctx::Ctx::new(&input_file, config_path);

    let start_time = std::time::Instant::now();

    let domains = ctx.parse().expect("Failed to parse input CSV");
    let domains = domains.downcast_ref::<IndexMap<String, Vec<String>>>().expect("Failed to downcast parsed data to IndexMap<String, Vec<String>>");
    let domains_name = domains.keys().cloned().collect::<Vec<String>>();
    let mut llm_results: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    ctx.prompt = llm::generate_prompt(&domains_name, ctx.config.max_domain_propositions);
    llm_results.extend(llm::sync_llm_runtime(&domains_name, &ctx).expect("Failed to get LLM results"));

    println!("LLM processing completed.");

    let aggregated = aggregate_data(&domains, &llm_results, &mut ctx.stats, ctx.config.use_internal_replacement);

    ctx.write(&aggregated).expect("Failed to write output CSV");

    let duration = start_time.elapsed();
    println!("Time elapsed {:?}", duration);

    Ok(())
}