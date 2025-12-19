use std::{collections::HashMap};
use std::error::Error;

use super::caching;
use super::billing;
use crate::llm::core::{generate_prompt_with_cached_content, generate_full_prompt, parse_llm_output}; 

use crate::ctx::Ctx;
use crate::llm::providers::gemini::network::{GeminiApiCall};

#[derive(Debug, Clone)]
pub struct GeminiResult {
    pub failed: usize,
    pub retried: usize,
    pub cost: f64,
    pub cache_saving: f64,
    pub categories: HashMap<String, Vec<String>>,
}

impl Default for GeminiResult {
    fn default() -> Self {
        Self {
            failed: 0,
            retried: 0,
            cost: 0.0,
            cache_saving: 0.0,
            categories: HashMap::with_capacity(4000000),
        }
    }
}

impl std::fmt::Display for GeminiResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "GeminiResult = failed: {}, retried: {}, cost: {}, cache_saving: {}",
            self.failed, self.retried, self.cost, self.cache_saving
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
pub async fn async_gemini_handle_cached_content(ctx: &Ctx, cost: &mut f64) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    if ctx.config.use_gemini_explicit_caching {
        let cache_contents = caching::list_cached_contents().await?;
        let cache_content_name = match cache_contents.cached_contents {
            Some(cached_list) => {
                let cache = cached_list.first().unwrap();
                if let Some(expire_time) = &cache.expire_time {
                    if check_cache_expire_time(expire_time, 1)? {
                        let cache_content = caching::async_gemini_update_cached_content_ttl(&cache.name, ctx.config.use_gemini_custom_cache_duration.as_ref().unwrap().clone()).await?;
                        let cache_cost = billing::CacheCostResult::new(cache_content.usage.unwrap()).compute_cost();
                        *cost += cache_cost.eur;
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
                *cost += cache_cost.eur;
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
    domains: &[String],
    ctx: &Ctx,
    cache_name : Option<String>,
    my_result: &mut GeminiResult
) -> Result<(), Box<dyn Error>> {

    let user_prompt = if cache_name.is_some() {
        generate_prompt_with_cached_content(&domains)
    } else {
        generate_full_prompt(&domains, ctx.config.max_domain_propositions)  
    };

    let generating_api_call = GeminiApiCall::Generate{
        model: ctx.config.model[0].clone(),
        prompt: user_prompt,
        cache_name,
        use_url_context: ctx.config.use_gemini_url_context,
        use_google_search: ctx.config.use_gemini_google_search,
        thinking_budget: ctx.config.thinking_budget,
    };

    let result = generating_api_call.process_request().await?;

    let cost = billing::CostResult::new(result.usage_metadata.clone()).compute_cost();
    my_result.cost += cost.eur;
    my_result.cache_saving += cost.cache_saving;

    if let Some(candidate) = result.candidates.first() {
        match candidate.content.parts.first() {
            None => {
                return Err("No content in the response message.".into());
            },
            Some(content) => {
                match parse_llm_output(domains, content.text.as_deref().unwrap_or("")) {
                    Ok(categories) => {
                        my_result.categories.extend(categories);
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

