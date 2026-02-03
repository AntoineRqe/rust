use std::{collections::HashMap};
use std::error::Error;

use super::caching;
use super::billing;
use crate::core::prompt::{
    generate_categorization_full_prompt,
    generate_categorization_prompt_with_cached_content,
    generate_description_full_prompt,
};

use crate::core::description::parse_description_output;
use crate::core::categorization::parse_categorization_output;
use crate::core::LLMCommand; 
use crate::providers::gemini::network::{GeminiApiCall};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use atomic_float::AtomicF64;

#[derive(Debug)]
pub struct GeminiResult {
    pub processed: AtomicUsize,
    pub failed: AtomicUsize,
    pub retried: AtomicUsize,
    pub cost: AtomicF64,
    pub cache_saving: AtomicF64,
    pub categories: HashMap<String, Vec<&'static str>>,
    pub descriptions: HashMap<String, HashMap<&'static str, String>>,
}

impl GeminiResult {
    pub fn new() -> Self {
        Self {
            processed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            retried: AtomicUsize::new(0),
            cost: AtomicF64::new(0.0),
            cache_saving: AtomicF64::new(0.0),
            categories: HashMap::with_capacity(10000),
            descriptions: HashMap::with_capacity(10000),
        }
    }

    pub fn merge(&mut self, other: &GeminiResult) {
        self.processed.fetch_add(other.processed.load(Ordering::Relaxed), Ordering::Relaxed);
        self.failed.fetch_add(other.failed.load(Ordering::Relaxed), Ordering::Relaxed);
        self.retried.fetch_add(other.retried.load(Ordering::Relaxed), Ordering::Relaxed);
        self.cost.fetch_add(other.cost.load(Ordering::Relaxed), Ordering::Relaxed);
        self.cache_saving.fetch_add(other.cache_saving.load(Ordering::Relaxed), Ordering::Relaxed);

        for (domain, categories) in &other.categories {
            self.categories.entry(domain.clone())
                .or_insert_with(Vec::new)
                .extend(categories.iter().cloned());
        }
        for (domain, descriptions) in &other.descriptions {
            self.descriptions.entry(domain.clone())
                .or_insert_with(HashMap::new)
                .extend(descriptions.iter().map(|(k, v)| (*k, v.clone())));
        }
    }
}

impl Clone for GeminiResult {
    fn clone(&self) -> Self {
        Self {
            processed: AtomicUsize::new(self.processed.load(Ordering::Relaxed)),
            failed: AtomicUsize::new(self.failed.load(Ordering::Relaxed)),
            retried: AtomicUsize::new(self.retried.load(Ordering::Relaxed)),
            cost: AtomicF64::new(self.cost.load(Ordering::Release)),
            cache_saving: AtomicF64::new(self.cache_saving.load(Ordering::Release)),
            categories: self.categories.clone(),
            descriptions: self.descriptions.clone(),
        }
    }
}

impl std::fmt::Display for GeminiResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "GeminiResult = processed: {}, failed: {}, retried: {}, cost: {}, cache_saving: {}",
            self.processed.load(Ordering::Relaxed),
            self.failed.load(Ordering::Relaxed),
            self.retried.load(Ordering::Relaxed),
            self.cost.load(Ordering::Relaxed),
            self.cache_saving.load(Ordering::Relaxed)
        )?;
        for (category, items) in &self.categories {
            writeln!(f, "\nCategory: {}", category)?;
            for item in items {
                writeln!(f, " - {}", item)?;
            }
        }
        Ok(())
    }
}

use chrono::{DateTime, Utc, Duration};

/// Checks if the cached content needs to be refreshed based on its expiration time
/// # Arguments
/// * `expire_time_str` - Expiration time as a string in RFC3339 format
/// * `minutes` - Threshold in minutes to determine if refresh is needed
/// # Returns
/// * `Result<bool, chrono::ParseError>` - Ok(true) if refresh is needed, Ok(false) otherwise, or an error if parsing fails 
fn check_cache_expire_time(expire_time_str: &str, minutes: i64) -> Result<bool, chrono::ParseError> {
    let current_time: DateTime<Utc> = Utc::now();
    let expire_time: DateTime<Utc> = DateTime::parse_from_rfc3339(expire_time_str)?.with_timezone(&Utc);

    let threshold: Duration = Duration::minutes(minutes);

    let time_difference: Duration = expire_time.signed_duration_since(current_time);

    let need_refresh = time_difference < threshold;

    Ok(need_refresh)
}

/// Handles cached content for Gemini asynchronously
/// # Arguments
/// * `ctx` - Reference to the context containing configuration
/// * `cost` - Mutable reference to accumulate cost
/// # Returns
/// * `Result<Option<String>, Box<dyn Error + Send + Sync>>` - Ok(Some(cache_name)) if cached content is used or created, Ok(None) if not using caching, or an error
pub async fn async_gemini_handle_cached_content(config: &GeminiConfig, cost: &mut AtomicF64) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    if config.use_gemini_explicit_caching {
        let cache_contents = caching::list_cached_contents().await?;
        let cache_content_name = match cache_contents.cached_contents {
            Some(cached_list) => {
                let cache = cached_list.first().unwrap();
                if let Some(expire_time) = &cache.expire_time {
                    if check_cache_expire_time(expire_time, 1)? {
                        let cache_content = caching::async_gemini_update_cached_content_ttl(
                            &cache.name,
                        config.use_gemini_custom_cache_duration.as_ref().unwrap().clone()
                    ).await?;
                        let cache_cost = billing::CacheCostResult::new(cache_content.usage.unwrap()).compute_cost();
                        cost.fetch_add(cache_cost.eur, Ordering::Relaxed);
                        println!("Updated cached contents with name : {}.", cache_content.name);
                        return Ok(Some(cache_content.name));
                    } else {
                        println!("Cached content {} is still valid (expires at {}).", cache.name, expire_time);
                    }
                }
                cache.name.clone()
            },
            None => {
                let cache_content = caching::async_gemini_create_cached_content(&config.model, config.max_domain_propositions,   config.use_gemini_custom_cache_duration.clone()).await?;
                let cache_cost = billing::CacheCostResult::new(cache_content.usage.unwrap()).compute_cost();
                cost.fetch_add(cache_cost.eur, Ordering::Relaxed);
                println!("Created cached contents with name : {}.", cache_content.name);
                cache_content.name.clone()
            }
        };
        Ok(Some(cache_content_name))
    } else {
        Ok(None)
    }
}

pub struct GeminiConfig {
    pub model: String,
    pub prompt: String,
    pub cache_name: Option<String>,
    pub use_url_context: bool,
    pub use_google_search: bool,
    pub thinking_budget: i64,
    pub use_gemini_explicit_caching: bool,
    pub use_gemini_custom_cache_duration: Option<String>,
    pub max_domain_propositions: usize,
}

/// Fetches chat completion from Gemini asynchronously
/// # Arguments
/// * `domains` - Slice of domain names to process
/// * `config` - Reference to the Gemini configuration
/// * `cache_name` - Optional cache name for using cached content
/// * `my_result` - Mutable reference to accumulate Gemini results
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok(()) if successful, or an error
pub async fn async_gemini_fetch_chat_completion(
    domains: Vec<String>,
    config: &GeminiConfig,
    cache_name : &Option<String>,
    my_result: &mut GeminiResult,
    command: &LLMCommand,
    client: &reqwest::Client,
) -> Result<Vec<String>, Box<dyn Error>> {

    let user_prompt = match command {
        LLMCommand::CategorizeDomains => {
            if cache_name.is_some() {
                generate_categorization_prompt_with_cached_content(&domains)
            } else {
                generate_categorization_full_prompt(&domains, config.max_domain_propositions)  
            }
        },
        LLMCommand::DescribeDomains => {
            generate_description_full_prompt(&domains)   
        },
    };

    let generating_api_call = GeminiApiCall::Generate{
        model: config.model.clone(),
        prompt: user_prompt,
        cache_name: cache_name.clone(),
        use_url_context: config.use_url_context,
        use_google_search: config.use_google_search,
        thinking_budget: config.thinking_budget,
    };

    let result = generating_api_call.process_request(client).await?;

    let cost = billing::CostResult::new(&result.usage_metadata).compute_cost();
    my_result.cost.fetch_add(cost.eur, Ordering::Relaxed);
    my_result.cache_saving.fetch_add(cost.cache_saving, Ordering::Relaxed);

    if let Some(candidate) = result.candidates.first() {
        match candidate.content.parts.first() {
            None => {
                return Err("No content in the response message.".into());
            },
            Some(content) => {
                let response = content.text.as_deref().unwrap_or("");
                println!("LLM Response: {}", response);
                match command {
                    LLMCommand::CategorizeDomains => {
                        match parse_categorization_output(domains.clone(), response) {
                            Ok((valid, errors)) => {
                                my_result.processed.fetch_add(valid.len(), Ordering::Relaxed);
                                my_result.categories.extend(valid);
                                
                                if errors.len() > 0 {
                                    my_result.failed.fetch_add(errors.len(), Ordering::Relaxed);
                                    Ok(errors.keys().cloned().collect()) // Return list of domains that failed
                                } else {
                                    Ok(vec![])
                                }
                            },
                            Err(e) => {
                                return Err(format!("Error parsing LLM output : {}", e).into());
                            }
                        }
                    },
                    LLMCommand::DescribeDomains => {
                        match parse_description_output(domains.clone(), response) {
                            Ok((valid, errors)) => {
                                my_result.processed.fetch_add(valid.len(), Ordering::Relaxed);
                                my_result.descriptions.extend(valid);
                                
                                if errors.len() > 0 {
                                    my_result.failed.fetch_add(errors.len(), Ordering::Relaxed);
                                    Ok(errors.keys().cloned().collect()) // Return list of domains that failed
                                } else {
                                    Ok(vec![])
                                }
                            },
                            Err(e) => {
                                return Err(format!("Error parsing LLM output : {}", e).into());
                            }
                        }
                    }
                }
            }
        }
    } else {
        return Err("No choices in the response.".into());
    } 
}

