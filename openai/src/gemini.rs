use reqwest::Client;

use serde::{Deserialize, Serialize};
use std::{collections::HashMap};
use crate::llm::{generate_prompt, parse_llm_output};
use std::error::Error;
 

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    pub cache: Option<Cache>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Cache {
    pub no_cache: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GenerationConfig {
    pub response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_contextual_formatting: Option<bool>,
    pub temperature: Option<f32>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Part {
    #[serde(default)]
    pub text: Option<String>,

    #[serde(default)]
    pub function_call: Option<FunctionCall>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct FunctionCall {
    pub name: String,
    pub args: serde_json::Value, // flexible, supports any tool args
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: Vec<MessageContent>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum MessageContent {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ToolDefinition {
    #[serde(rename = "url_context")]
    UrlContext {
        #[serde(default)]
        url_context: UrlContextConfig,
    },
    #[serde(rename = "google_search")]
    GoogleSearch {
        #[serde(default)]
        google_search: GoogleSearchConfig,
    },
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct UrlContextConfig {}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct GoogleSearchConfig {}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct ChatCompletionResponse {
    pub id: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    pub object: Option<String>,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Option<Usage>,
    #[serde(default)]
    pub vertex_ai_grounding_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub vertex_ai_url_context_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub vertex_ai_safety_results: Option<serde_json::Value>,
    #[serde(default)]
    pub vertex_ai_citation_metadata: Option<serde_json::Value>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct ChatCompletionChoice {
    pub message: ChatMessageResponse,
    pub index: usize,
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct ImageContent {
    pub url: String,
    pub alt_text: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct ThinkingBlock {
    pub title: String,
    pub content: String,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct ChatMessageResponse {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>, // Gemini returns plain text
    #[serde(default)]
    pub parts: Option<Vec<Part>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub images: Option<Vec<ImageContent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub thinking_blocks: Option<Vec<ThinkingBlock>>,
    #[serde(rename = "groundingMetadata", skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub grounding_metadata: Option<GroundingMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroundingMetadata {
    #[serde(rename = "webSearchQueries")]
    pub web_search_queries: Vec<String>,

    #[serde(rename = "searchEntryPoint")]
    pub search_entry_point: SearchEntryPoint,

    #[serde(rename = "groundingChunks")]
    pub grounding_chunks: Vec<GroundingChunk>,

    #[serde(rename = "groundingSupports")]
    pub grounding_supports: Vec<GroundingSupport>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchEntryPoint {
    #[serde(rename = "renderedContent")]
    pub rendered_content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroundingChunk {
    pub web: WebInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebInfo {
    pub uri: String,
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroundingSupport {
    pub segment: Segment,
    #[serde(rename = "groundingChunkIndices")]
    pub grounding_chunk_indices: Vec<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Segment {
    #[serde(rename = "startIndex")]
    pub start_index: u32,

    #[serde(rename = "endIndex")]
    pub end_index: u32,

    pub text: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct GeminiResult {
    pub failed: usize,
    pub retried: usize,
    pub cost: f64,
    pub categories: HashMap<String, Vec<String>>,
}

impl Default for GeminiResult {
    fn default() -> Self {
        Self {
            failed: 0,
            retried: 0,
            cost: 0.0,
            categories: HashMap::new(),
        }
    }
}

impl std::fmt::Display for GeminiResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GeminiResult {{ failed: {}, retried: {}, cost: {}, categories: {:?} }}",
            self.failed, self.retried, self.cost, self.categories
        )
    }
}


pub async fn async_gemini_fetch_chat_completion(domains: &[String], model: &String, nb_propositions: usize, my_result: GeminiResult) -> Result<GeminiResult, Box<dyn Error>> {

    let mut my_result = my_result;

    let tools = vec![
        ToolDefinition::UrlContext {
            url_context: UrlContextConfig::default()
        },
        ToolDefinition::GoogleSearch {
            google_search: GoogleSearchConfig::default()
        }
    ];

    let user_prompt = generate_prompt(&domains, nb_propositions);

    let api_key = std::env::var("OPENAI_API_KEY")
        .expect("Set OPENAI_API_KEY environment variable");

    let litellm_url = "https://lab.iaparc.chapsvision.com/llm-gateway/v1/chat/completions".to_string();

    //async_gemini_create_cached_content(model, nb_propositions).await?;

    let req = ChatCompletionRequest {
        model: format!("gemini/{}", model),
        messages: vec![
            ChatMessage {
                role: "user".to_string(),
                content: vec![
                    MessageContent::Text {
                        text: user_prompt,
                    }
                ]
            }
        ],
        cache: Some(Cache {
            no_cache: true,
        }),
        thinking: Some(ThinkingConfig {
            // enabled: Some(true),
            enabled: None,
            budget_tokens: Some(4096)
        }),

        tools: Some(tools),
        generation_config: Some(GenerationConfig {
            response_mime_type: Some("text/plain".into()),
            disable_contextual_formatting: Some(true),
            temperature: Some(0.2),
        }),
    };

    let client = Client::new();
    
    let resp = client
        .post(&litellm_url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&req)
        .send()
        .await?;
    
    match resp.headers().get("x-litellm-response-cost") {
        Some(cost) => {
            if let Ok(cost_str) = cost.to_str() {
                if let Ok(cost_f64) = cost_str.parse::<f64>() {
                    println!("Request sent to LLM cost: {}", cost_f64);
                    my_result.cost += cost_f64;
                } else {
                    println!("Failed to parse cost as f64.");
                }
            } else {
                println!("Failed to convert cost header to string.");
            }
        }
        None => println!("No cost information in response headers."),
    }

    let result : ChatCompletionResponse = resp.json().await?;

    let mut cat_vec: HashMap<String, Vec<String>> = HashMap::new();

    if let Some(candidate) = result.choices.first() {
        match candidate.message.content {
            None => {
                println!("No content in the response message.");
                my_result.failed += domains.len();
                return Ok(my_result);
            },
            Some(ref content) => {
                match parse_llm_output(domains, content, nb_propositions) {
                    Ok(categories) => {
                        cat_vec = categories;
                    },
                    Err(e) => {
                        println!("Error parsing LLM output: {}", e);
                        my_result.failed += domains.len();
                        return Ok(my_result);
                    }
                }
            }
        }
    }

    my_result.categories = cat_vec;

    Ok(my_result)
}
