use reqwest::Client;

use serde::{Deserialize, Serialize};
use std::{collections::HashMap};
use std::error::Error;

use super::caching;
use super::billing;
use crate::llm::core::{generate_prompt_with_cached_content, generate_full_prompt, parse_llm_output}; 

use crate::ctx::Ctx;

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
            categories: HashMap::new(),
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

use rand::prelude::*; // brings Rng and thread_rng into scope

pub fn generate_seed() -> i32 {
    let mut rng = rand::rng();
    rng.random()  // new API in rand 0.9
}

use chrono::{DateTime, Utc, Duration};

fn is_expired_within_threshold(expire_time_str: &str, minutes: i64) -> Result<bool, chrono::ParseError> {
    let current_time: DateTime<Utc> = Utc::now();
    let expire_time: DateTime<Utc> = DateTime::parse_from_rfc3339(expire_time_str)?.with_timezone(&Utc);
    let zero_duration: Duration = Duration::zero();

    let threshold: Duration = Duration::minutes(minutes);

    let time_difference: Duration = current_time.signed_duration_since(expire_time);

    let need_refresh = time_difference < zero_duration || time_difference < threshold;
    println!("Current time: {}", current_time);
    println!("Expire time: {}", expire_time);
    println!("Time difference (in minutes): {}", time_difference.num_minutes());
    println!("Is recently expired (within {} minutes): {}", minutes, need_refresh);
    Ok(need_refresh)
}

pub async fn async_gemini_handle_cached_content(ctx: &Ctx) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    if ctx.config.use_gemini_caching {
        let cache_contents = caching::list_cached_contents().await?;
        let cache_content_name = match cache_contents.cached_contents {
            Some(cached_list) => {
                let cache = cached_list.first().unwrap();
                println!("Retrieved cached contents with name : {}.", cache.name);
                if let Some(expire_time) = &cache.expire_time {
                    if is_expired_within_threshold(expire_time, 5)? {
                        println!("Cached content {} expired within threshold. Creating new cached content.", cache.name);
                        let cache_content = caching::async_gemini_update_cached_content_ttl(&cache.name, ctx.config.use_gemini_custom_cache_duration.as_ref().unwrap().clone()).await?;
                        let cache_cost = billing::CacheCostResult::new(cache_content.usage.unwrap()).compute_cost();
                        println!("{}", cache_cost);
                        println!("Created cached contents with name : {}.", cache_content.name);
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
                println!("{}", cache_cost);
                println!("Created cached contents with name : {}.", cache_content.name);
                cache_content.name.clone()
            }
        };
        Ok(Some(cache_content_name))
    } else {
        Ok(None)
    }
}

pub async fn async_gemini_fetch_chat_completion(domains: &[String], ctx: &Ctx, cache_name : Option<String>, my_result: &mut GeminiResult) -> Result<(), Box<dyn Error>> {

    let client: Client = Client::new();

    let user_prompt = if cache_name.is_some() {
        generate_prompt_with_cached_content(&domains)
    } else {
        generate_full_prompt(&domains, ctx.config.max_domain_propositions)  
    };

    let api_key = std::env::var("MY_GEMINI_API_KEY")
        .expect("Set MY_GEMINI_API_KEY environment variable");

    let api_endpoint = "aiplatform.googleapis.com";
    let generate_content_api = "generateContent";

    let url = format!(
        "https://{}/v1/publishers/google/models/{}:{}?key={}",
        api_endpoint,                    // e.g. "us-central1-aiplatform.googleapis.com"
        ctx.config.model[0],             // e.g. "gemini-2.5-flash"
        generate_content_api,            // "generateContent"
        api_key                          // Your API key
    );

    
    let mut tools = vec![];

    if ctx.config.use_gemini_url_context {
        tools.push(
            Tool {
                url_context: Some(UrlContextTool {}),
                google_search: None,
            }
        );
    }
    if ctx.config.use_gemini_google_search {
        tools.push(
            Tool {
                url_context: None,
                google_search: Some(GoogleSearchTool {}),
            }
        );
    }

    let request = GeminiRequest {
        contents: vec![
            Content {
                role: Some(String::from("user")),
                parts: vec![
                    Part {
                        text: Some(user_prompt),
                        inline_data: None,
                        file_data: None,
                        video_metadata: None,
                    }
                ]
            }
        ],

        cached_content: cache_name, // Replace with actual cache ID
        system_instruction: None,
        tools: Some(tools),
        safety_settings: None,
        generation_config: Some(GenerationConfig {
            temperature: Some(1.0),
            top_p: None,
            top_k: None,
            candidate_count: None,
            max_output_tokens: None,
            presence_penalty: None,
            frequency_penalty: None,
            stop_sequences: None,
            response_mime_type: Some("application/json".to_string()),
            response_schema: 
                ResponseSchema {
                    schema_type: "object".into(),
                    additional_properties: AdditionalProperties {
                        value_type: "array".into(),
                        items: Items { item_type: "string".into() }
                    }
                },
            seed: Some(generate_seed() as i32),
            response_logprobs: None,
            logprobs: None,
            audio_timestamp: None,
            thinking_config: Some(ThinkingConfig {
                thinking_budget: Some(ctx.config.thinking_budget as i64),
            }),
            disable_nvcc: None,
        }),
        labels: None,
    };

    let result = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    let status = result.status();

    let body = result.text().await?;

    // If the API returned an error status, print the body
    if !status.is_success() {
        eprintln!("Gemini API request failed. Status: {}", status);
        eprintln!("Response body: {}", body);
        return Err(format!("Gemini API error: {}", status).into());
    }

    //println!("Gemini API response body: {}", body);
    let result: ApiResponse = serde_json::from_str(&body)?;

    let cost = billing::CostResult::new(result.usage_metadata.clone()).compute_cost();
    println!("{}", cost);
    my_result.cost += cost.eur;
    my_result.cache_saving += cost.cache_saving;

    if let Some(candidate) = result.candidates.first() {
        match candidate.content.parts.first() {
            None => {
                println!("No content in the response message.");
                return Err("No content in the response message.".into());
            },
            Some(content) => {
                match parse_llm_output(domains, content.text.as_deref().unwrap_or("")) {
                    Ok(categories) => {
                        my_result.categories.extend(categories);
                        Ok(())
                    },
                    Err(e) => {
                        println!("Error parsing LLM output: {}", e);
                        return Err("Error parsing LLM output.".into());
                    }
                }
            }
        }
    } else {
        println!("No choices in the response.");
        return Err("No choices in the response.".into());
    } 
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiRequest {
    #[serde(rename = "cachedContent")]
    pub cached_content: Option<String>,

    pub contents: Vec<Content>,

    #[serde(rename = "systemInstruction")]
    pub system_instruction: Option<SystemInstruction>,

    pub tools: Option<Vec<Tool>>,

    #[serde(rename = "safetySettings")]
    pub safety_settings: Option<Vec<SafetySetting>>,

    #[serde(rename = "generationConfig")]
    pub generation_config: Option<GenerationConfig>,

    pub labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Content {
    pub role: Option<String>,
    pub parts: Vec<Part>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemInstruction {
    pub role: String,
    pub parts: Vec<SystemPart>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemPart {
    pub text: String,
}

//
// ─────────────────────────────────────────────
//  Union Part Type
// ─────────────────────────────────────────────
//
#[derive(Debug, Serialize, Deserialize)]
pub struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,

    #[serde(rename = "fileData", skip_serializing_if = "Option::is_none")]
    pub file_data: Option<FileData>,

    #[serde(
        rename = "videoMetadata",
        skip_serializing_if = "Option::is_none"
    )]
    pub video_metadata: Option<VideoMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InlineData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VideoMetadata {
    #[serde(rename = "startOffset")]
    pub start_offset: VideoTimestamp,

    #[serde(rename = "endOffset")]
    pub end_offset: VideoTimestamp,

    pub fps: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VideoTimestamp {
    pub seconds: i64,
    pub nanos: i32,
}

//
// ─────────────────────────────────────────────
//  Tools & Function Declarations
// ─────────────────────────────────────────────
//
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub struct Tool {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_context: Option<UrlContextTool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_search: Option<GoogleSearchTool>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UrlContextTool {}

#[derive(Serialize, Deserialize, Debug)]
pub struct GoogleSearchTool {}
//
// ─────────────────────────────────────────────
//  Safety Settings
// ─────────────────────────────────────────────
//
#[derive(Debug, Serialize, Deserialize)]
pub struct SafetySetting {
    pub category: HarmCategory,
    pub threshold: HarmBlockThreshold,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmCategory {
    Harassment,
    HateSpeech,
    SexuallyExplicit,
    DangerousContent,
    Violence,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HarmBlockThreshold {
    BlockNone,
    BlockLow,
    BlockMedium,
    BlockHigh,
}

//
// ─────────────────────────────────────────────
//  Generation Config
// ─────────────────────────────────────────────
//
#[derive(Debug, Serialize, Deserialize)]
pub struct GenerationConfig {
    pub temperature: Option<f64>,
    #[serde(rename = "topP")]
    pub top_p: Option<f64>,
    #[serde(rename = "topK")]
    pub top_k: Option<i32>,
    #[serde(rename = "candidateCount")]
    pub candidate_count: Option<i32>,
    #[serde(rename = "maxOutputTokens")]
    pub max_output_tokens: Option<i32>,
    #[serde(rename = "presencePenalty")]
    pub presence_penalty: Option<f32>,
    #[serde(rename = "frequencyPenalty")]
    pub frequency_penalty: Option<f32>,

    #[serde(rename = "stopSequences")]
    pub stop_sequences: Option<Vec<String>>,

    #[serde(rename = "responseMimeType")]
    pub response_mime_type: Option<String>,

    #[serde(rename = "responseSchema")]
    pub response_schema: ResponseSchema,

    pub seed: Option<i32>,

    #[serde(rename = "responseLogprobs")]
    pub response_logprobs: Option<bool>,

    pub logprobs: Option<i32>,

    #[serde(rename = "audioTimestamp")]
    pub audio_timestamp: Option<bool>,

    #[serde(rename = "thinkingConfig")]
    pub thinking_config: Option<ThinkingConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "disableNvcc")]
    pub disable_nvcc: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResponseSchema {
    #[serde(rename = "type")]
    pub schema_type: String,

    #[serde(rename = "additionalProperties")]
    pub additional_properties: AdditionalProperties,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AdditionalProperties {
    #[serde(rename = "type")]
    pub value_type: String,

    pub items: Items,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Items {
    #[serde(rename = "type")]
    pub item_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ThinkingConfig {
    #[serde(rename = "thinkingBudget")]
    pub thinking_budget: Option<i64>,
}

// ─────────────────────────────────────────────
//  API Response Structures
// ─────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct ApiResponse {
    pub candidates: Vec<Candidate>,

    #[serde(rename = "usageMetadata")]
    pub usage_metadata: UsageMetadata,

    #[serde(rename = "modelVersion")]
    pub model_version: String,

    #[serde(rename = "createTime")]
    pub create_time: String,

    #[serde(rename = "responseId")]
    pub response_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Candidate {
    pub content: Content,

    #[serde(rename = "finishReason")]
    pub finish_reason: String,   // "STOP"

    #[serde(rename = "groundingMetadata")]
    pub grounding_metadata: Option<GroundingMetadata>, // NEW

    #[serde(rename = "avgLogprobs")]
    pub avg_logprobs: Option<f64>,   // Present
                                     // These fields are NOT present, so must be Option:
    #[serde(rename = "safetyRatings")]
    pub safety_ratings: Option<Vec<SafetyRating>>,

    #[serde(rename = "citationMetadata")]
    pub citation_metadata: Option<CitationMetadata>,

    #[serde(rename = "logprobsResult")]
    pub logprobs_result: Option<LogprobsResult>,
}

// ---------- Optional fields that may or may not appear ----------
#[derive(Debug, Deserialize, Serialize)]
pub struct SafetyRating {
    pub category: Option<String>,
    pub probability: Option<String>,
    pub blocked: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CitationMetadata {
    pub citations: Vec<Citation>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Citation {
    #[serde(rename = "startIndex")]
    pub start_index: i32,
    #[serde(rename = "endIndex")]
    pub end_index: i32,
    pub uri: String,
    pub title: String,
    pub license: String,

    #[serde(rename = "publicationDate")]
    pub publication_date: Option<PublicationDate>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PublicationDate {
    pub year: i32,
    pub month: i32,
    pub day: i32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LogprobsResult {
    #[serde(rename = "topCandidates")]
    pub top_candidates: Vec<TopCandidateSet>,
    #[serde(rename = "chosenCandidates")]
    pub chosen_candidates: Vec<ChosenCandidate>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TopCandidateSet {
    pub candidates: Vec<TokenProbability>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TokenProbability {
    pub token: String,

    #[serde(rename = "logProbability")]
    pub log_probability: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChosenCandidate {
    pub token: String,

    #[serde(rename = "logProbability")]
    pub log_probability: f32,
}

// ---------------- Grounding Metadata ----------------
#[derive(Debug, Deserialize, Serialize)]
pub struct GroundingMetadata {
    #[serde(rename = "webSearchQueries")]
    pub web_search_queries: Option<Vec<String>>,

    #[serde(rename = "searchEntryPoint")]
    pub search_entry_point: Option<SearchEntryPoint>,

    #[serde(rename = "groundingChunks")]
    pub grounding_chunks: Option<Vec<GroundingChunk>>,

    #[serde(rename = "groundingSupports")]
    pub grounding_supports: Option<Vec<GroundingSupport>>,

    #[serde(rename = "retrievalMetadata")]
    pub retrieval_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchEntryPoint {
    #[serde(rename = "renderedContent")]
    pub rendered_content: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GroundingChunk {
    pub web: WebChunk,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WebChunk {
    pub uri: String,
    pub title: String,
    pub domain: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GroundingSupport {
    pub segment: Segment,
    #[serde(rename = "groundingChunkIndices")]
    pub grounding_chunk_indices: Vec<usize>,
    #[serde(rename = "confidenceScores")]
    pub confidence_scores: Vec<f32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Segment {
    #[serde(rename = "startIndex")]
    pub start_index: usize,
    #[serde(rename = "endIndex")]
    pub end_index: usize,
    pub text: String,
}

// ------------ Usage Metadata -------------
#[derive(Debug, Deserialize, Serialize)]
pub struct UsageMetadata {
    #[serde(rename = "promptTokenCount")]
    pub prompt_token_count: i64,

    #[serde(rename = "candidatesTokenCount")]
    pub candidates_token_count: i64,

    #[serde(rename = "totalTokenCount")]
    pub total_token_count: i64,

    #[serde(rename = "trafficType")]
    pub traffic_type: Option<String>,

    #[serde(rename = "promptTokensDetails")]
    pub prompt_tokens_details: Option<Vec<TokenDetail>>,

    #[serde(rename = "candidatesTokensDetails")]
    pub candidates_tokens_details: Option<Vec<TokenDetail>>,

    #[serde(rename = "thoughtsTokenCount")]
    pub thoughts_token_count: Option<i64>,

    #[serde(rename = "cachedContentTokenCount")]
    pub cached_content_token_count: Option<i64>,
}

impl Clone for UsageMetadata {
    fn clone(&self) -> Self {
        UsageMetadata {
            prompt_token_count: self.prompt_token_count,
            candidates_token_count: self.candidates_token_count,
            total_token_count: self.total_token_count,
            traffic_type: self.traffic_type.clone(),
            prompt_tokens_details: None,
            candidates_tokens_details: None,
            thoughts_token_count: self.thoughts_token_count,
            cached_content_token_count: self.cached_content_token_count,
        }
    }
}

impl std::fmt::Display for UsageMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "UsageMetadata {{")?;
        writeln!(f, "\t\tprompt_token_count:\t\t{}", self.prompt_token_count)?;
        writeln!(f, "\t\tcandidates_token_count:\t\t{}", self.candidates_token_count)?;
        writeln!(f, "\t\ttotal_token_count:\t\t{}", self.total_token_count)?;
        writeln!(
            f,
            "\t\tthoughts_token_count:\t\t{}",
            self.thoughts_token_count.unwrap_or(0)
        )?;
        writeln!(
            f,
            "\t\tcached_content_token_count:\t{}",
            self.cached_content_token_count.unwrap_or(0)
        )?;
        write!(f, "}}")
    }
}


#[derive(Debug, Deserialize, Serialize)]
pub struct TokenDetail {
    pub modality: String,
    #[serde(rename = "tokenCount")]
    pub token_count: i32,
}