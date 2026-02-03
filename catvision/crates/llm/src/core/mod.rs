use async_scoped::TokioScope;
use tokio::runtime::{Runtime};
use std::sync::atomic::Ordering;
use crate::core::tools::write_domain_in_garbage_file;
use crate::providers::gemini::generating::{async_gemini_fetch_chat_completion, GeminiResult, GeminiConfig, async_gemini_handle_cached_content};
use crate::providers::gemini::network::GeminiNetworkClient;
use config::Config;
pub mod categorization;
pub mod prompt;
pub mod description;
pub mod tools;

pub enum LLMCommand {
    CategorizeDomains,
    DescribeDomains,
}

enum LLMResult {
    Gemini(GeminiResult),
}


type DynError = Box<dyn std::error::Error + Send + Sync>;

async fn async_llm_process_command(
    domains: &Vec<String>,
    config: &Config,
    cache_name: &Option<String>,
    id: usize,
    client: &reqwest::Client,
    command: &LLMCommand
) -> Result<LLMResult, DynError> {

    let mut gemini_result = GeminiResult::new();
    let mut retries_chunk = 0;
    let mut domains = domains.clone();
    
    let gemini_config = GeminiConfig {
        model: config.model[0].clone(),
        prompt: String::new(), // Prompt will be generated in the fetch function
        cache_name: cache_name.clone(),
        use_url_context: config.use_gemini_url_context,
        use_google_search: config.use_gemini_google_search,
        thinking_budget: config.thinking_budget,
        use_gemini_explicit_caching: config.use_gemini_explicit_caching,
        use_gemini_custom_cache_duration: config.use_gemini_custom_cache_duration.clone(),
        max_domain_propositions: config.max_domain_propositions,
    };

    loop {
        if retries_chunk == 3 {
            eprintln!("Thread {} Failed to get LLM response after 3 attempts for chunk starting with domain: {}", id, domains[0]);
            gemini_result.failed.fetch_add(domains.len(), Ordering::Relaxed);
            write_domain_in_garbage_file(&domains, id);

            // If no domains were processed successfully, return an error
            if gemini_result.processed.load(Ordering::Relaxed) == 0 {
                return Err("Max retries reached for LLM request".into());
            }

            break;
        }

        match async_gemini_fetch_chat_completion(
            domains.clone(),
            &gemini_config,
            cache_name,
  &mut gemini_result,
            command,
            client,
        )
            .await 
            {
            Ok(remaining) => {
                if remaining.len() > 0 {
                    eprintln!("Thread {} Some domains were not processed, retrying: {:?}", id, remaining);
                    // Update domains to only the remaining ones for the next attempt
                    domains = remaining;
                    retries_chunk += 1;
                    continue;
                } else {
                    println!("Thread {} Successfully processed chunk starting with domain: {}", id, domains[0]);
                    break;
                }
            },
            Err(e) => {
                eprintln!("Thread {} Error during LLM request (attempt {}): {}", id, retries_chunk + 1, e);
                retries_chunk += 1;
                gemini_result.retried.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
    }

    //println!("Thread {} Final Gemini Result: {}", id, gemini_result);

    Ok(LLMResult::Gemini(GeminiResult {
        processed: gemini_result.processed,
        retried: gemini_result.retried,
        failed: gemini_result.failed,
        cost: gemini_result.cost,
        cache_saving: gemini_result.cache_saving,
        categories: gemini_result.categories,
        descriptions: gemini_result.descriptions,
    }))
}

async fn llm_runtime(domains: Vec<String>, config: &Config, command: &LLMCommand) -> Result<GeminiResult, DynError> {

    let mut chunks = domains.chunks(config.chunk_size);
    let mut processed_domains = 0;
    let total_domains = domains.len();

    let mut final_gemini_result = GeminiResult::new();

    let gemini_config = GeminiConfig {
        model: config.model[0].clone(),
        prompt: String::new(), // Prompt will be generated in the fetch function
        cache_name: None,
        use_url_context: config.use_gemini_url_context,
        use_google_search: config.use_gemini_google_search,
        thinking_budget: config.thinking_budget,
        use_gemini_explicit_caching: config.use_gemini_explicit_caching,
        use_gemini_custom_cache_duration: config.use_gemini_custom_cache_duration.clone(),
        max_domain_propositions: config.max_domain_propositions,
    };

    let network_clients = GeminiNetworkClient::new(config.max_threads);

    // We want to make sure all chunks are processed
    while chunks.len() > 0 {
        // Handle cached content creation or update for the next batch of chunks
        let cache_name = match async_gemini_handle_cached_content(&gemini_config, &mut final_gemini_result.cost).await {
            Ok(cache_name) => cache_name,
            Err(e) => {
                eprintln!("Error handling cached content: {}", e);
                while let Some(chunk) = chunks.next() {
                    processed_domains += chunk.len();
                    println!("Skipping LLM runtime on {} with chunk size [{}-{}]/{} due to caching error",
                        config.model[0], processed_domains - chunk.len(), processed_domains, total_domains
                    );
                    final_gemini_result.failed.fetch_add(chunk.len(), Ordering::Relaxed);
                    write_domain_in_garbage_file(&chunk.to_vec(), 666); // Using 666 as an arbitrary ID for skipped chunks
                }
                break;
            }
        };
    
        let (_, results) = TokioScope::scope_and_block(|scope| {
            for id in 0..config.max_threads {
                let client = &network_clients.client[id];
                if let Some(chunk) = chunks.next() {

                    processed_domains += chunk.len();
                    println!("Starting LLM runtime on {} with chunk size [{}-{}]/{}",
                        config.model[0],
                        processed_domains - chunk.len(),
                        processed_domains,
                        total_domains
                    );

                    let cache_name = &cache_name;

                    scope.spawn(async move {
                        match async_llm_process_command(
                            &chunk.to_vec(),
                            &config,
                            cache_name,
                            id,
                            client,
                            command)
                            .await {
                            Ok(LLMResult::Gemini(gemini_result)) => {
                                Ok(gemini_result)
                            },
                            Err(e) => {
                                eprintln!("Thread {} LLM classification failed: {}", id, e);
                                Err(e)
                            }
                        } 
                    });

                } else {
                    break;
                }
            }
        });

        // Process the results
        for result in results {
            match result {
                Ok(Ok(gemini_result)) => {
                    // Successfully got a result
                    final_gemini_result.merge(&gemini_result); // or whatever you want to do
                }
                Ok(Err(e)) => {
                    eprintln!("Task returned error: {}", e);
                }
                Err(join_error) => {
                    eprintln!("Task panicked: {:?}", join_error);
                }
            }
        }

        println!(
            "Completed LLM runtime on {} with chunk size [{}-{}]/{}",
            config.model[0],
            processed_domains.checked_sub(config.chunk_size * config.max_threads).unwrap_or(0),
            processed_domains,
            total_domains
        );

    }

    println!(
        "LLM runtime completed on {} for total domains: {}",
        config.model[0],
        total_domains
    );

    Ok(final_gemini_result)

}

pub fn sync_llm_runtime(domains: Vec<String>, config: &Config, command: LLMCommand) -> Result<GeminiResult, DynError> {
    // Create a new Tokio runtime
    let rt = Runtime::new().expect("Failed to create Tokio runtime");

    // Block on the async function
    rt.block_on(llm_runtime(domains, config, &command))
}
