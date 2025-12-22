use std::{collections::HashMap};
use std::error::Error;

use super::caching;
use super::billing;
use crate::llm::core::{generate_prompt_with_cached_content, generate_full_prompt, parse_llm_output}; 

use crate::ctx::Ctx;
use crate::llm::providers::gemini::network::{GeminiApiCall};
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
}

impl GeminiResult {
    pub fn new() -> Self {
        Self {
            processed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            retried: AtomicUsize::new(0),
            cost: AtomicF64::new(0.0),
            cache_saving: AtomicF64::new(0.0),
            categories: HashMap::with_capacity(4000000),
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
        }
    }
}

impl Default for GeminiResult {
    fn default() -> Self {
        Self {
            processed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            retried: AtomicUsize::new(0),
            cost: AtomicF64::new(0.0),
            cache_saving: AtomicF64::new(0.0),
            categories: HashMap::with_capacity(4000000),
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
pub async fn async_gemini_handle_cached_content(ctx: &Ctx, cost: &mut AtomicF64) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    if ctx.config.use_gemini_explicit_caching {
        let cache_contents = caching::list_cached_contents().await?;
        let cache_content_name = match cache_contents.cached_contents {
            Some(cached_list) => {
                let cache = cached_list.first().unwrap();
                if let Some(expire_time) = &cache.expire_time {
                    if check_cache_expire_time(expire_time, 1)? {
                        let cache_content = caching::async_gemini_update_cached_content_ttl(&cache.name, ctx.config.use_gemini_custom_cache_duration.as_ref().unwrap().clone()).await?;
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
                let cache_content = caching::async_gemini_create_cached_content(&ctx.config.model[0], ctx.config.max_domain_propositions, ctx.config.use_gemini_custom_cache_duration.clone()).await?;
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

/// Fetches chat completion from Gemini asynchronously
/// # Arguments
/// * `domains` - Slice of domain names to process
/// * `ctx` - Reference to the context containing configuration
/// * `cache_name` - Optional cache name for using cached content
/// * `my_result` - Mutable reference to accumulate Gemini results
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok(()) if successful, or an error
pub async fn async_gemini_fetch_chat_completion(
    domains: &[&str],
    ctx: &Ctx,
    cache_name : &Option<String>,
    my_result: &mut GeminiResult
) -> Result<(), Box<dyn Error>> {

    let user_prompt = if cache_name.is_some() {
        generate_prompt_with_cached_content(domains)
    } else {
        generate_full_prompt(domains, ctx.config.max_domain_propositions)  
    };

    let generating_api_call = GeminiApiCall::Generate{
        model: ctx.config.model[0].clone(),
        prompt: user_prompt,
        cache_name: cache_name.clone(),
        use_url_context: ctx.config.use_gemini_url_context,
        use_google_search: ctx.config.use_gemini_google_search,
        thinking_budget: ctx.config.thinking_budget,
    };

    let result = generating_api_call.process_request().await?;

    let cost = billing::CostResult::new(&result.usage_metadata).compute_cost();
    my_result.cost.fetch_add(cost.eur, Ordering::Relaxed);
    my_result.cache_saving.fetch_add(cost.cache_saving, Ordering::Relaxed);

    if let Some(candidate) = result.candidates.first() {
        match candidate.content.parts.first() {
            None => {
                return Err("No content in the response message.".into());
            },
            Some(content) => {
                match parse_llm_output(domains, content.text.as_deref().unwrap_or("")) {
                    Ok(categories) => {
                        my_result.categories.extend(categories);
                        my_result.processed.fetch_add(domains.len(), Ordering::Relaxed);
                        Ok(())
                    },
                    Err(e) => {
                        return Err(format!("Error parsing LLM output : {}", e).into());
                    }
                }
            }
        }
    } else {
        return Err("No choices in the response.".into());
    } 
}

