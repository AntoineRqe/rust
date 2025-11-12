use indexmap::IndexMap;

use crate::my_traits::{Input, Output};
use crate::statistics::Statistics;

mod llm;
mod category;
pub mod my_traits;
pub mod csv;
mod statistics;
mod html;


const OUTPUT_PATH : &str = "/home/anr-cv/remplacement_olfeo_netify/outputs/";
const FILENAME : &str = "/home/anr-cv/remplacement_olfeo_netify/reference_domains_v5.csv";
const TEST_FILE : &str = "/home/anr-cv/remplacement_olfeo_netify/test.csv";

struct Ctx {
    input: csv::MyCSVInput,
    output: csv::MyCSVOutput,
    model: &'static str,
    stats: Statistics,
}

impl Ctx {
    fn new(input_filename: &str, model: &'static str) -> Self {
        let input = csv::MyCSVInput::new(input_filename);
        let output = csv::MyCSVOutput::new(
            OUTPUT_PATH.to_string() + "my_reference_" + model + ".csv",
            input.headers.clone(),
        );

        Ctx {
            input,
            output,
            model,
            stats: Statistics::new(llm::get_llm_levels_count()),
        }
    }
}

fn aggregate_data(original_data: &indexmap::IndexMap<String, String>,
                llm_data: &std::collections::HashMap<String, Vec<String>>,
                stats: &mut Statistics) -> IndexMap<String, Vec<String>> {

    let mut aggregated = IndexMap::new();

    for (domain, expected_category) in original_data {
        stats.increment_domain_count();
        let mut tmp_categories = vec![];

        tmp_categories.push(expected_category.clone());

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
        }

        aggregated.insert(domain.clone(), tmp_categories);
    }

    return aggregated;
}

fn main() {
    let mut ctx = Ctx::new(TEST_FILE, llm::get_models("qwen").expect("Model not found"));

    let start_time = std::time::Instant::now();

    let domains = ctx.input.parse().expect("Failed to parse input CSV");

    let llm_results  = llm::sync_llm_runtime(&domains, ctx.model);

    let aggregated = aggregate_data(&domains, &llm_results, &mut ctx.stats);

    ctx.output.write(&aggregated).expect("Failed to write output CSV");

    let _ = html::generate_html_table(&aggregated, "/home/anr-cv/remplacement_olfeo_netify/test.html");

    let duration = start_time.elapsed();
    println!("Time elapsed {:?}", duration);

}