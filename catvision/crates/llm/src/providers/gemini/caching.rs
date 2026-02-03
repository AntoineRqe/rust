// Structure pour les parties de contenu (texte, etc.)
use serde::{Deserialize, Serialize};
use reqwest::Client;
use std::error::Error;
use std::process::Command;

use crate::core::prompt::generate_cached_prompt;
use utils::env::{get_project_id};

//
// Request structure for creating cached content
//

#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum CachedPart {
    #[serde(rename = "text")]
    TextPart { text: String },
}

// Structure pour un bloc de contenu (avec le rôle)
#[derive(Serialize, Debug)]
pub struct CachedContent {
    // DOIT être "user" pour l'initialisation du contexte
    pub role: String, 
    pub parts: Vec<CachedPart>,
}
// Structure principale pour l'appel POST
#[derive(Serialize, Debug)]
pub struct CacheRequest {
    // Le modèle est au niveau principal
    pub model: String,
    // Le nom d'affichage est au niveau principal
    pub display_name: String,
    // Le champ "context" requis
    pub contents: Vec<CachedContent>,
    #[serde(rename = "ttl")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedUsageMetadata {
    #[serde(rename = "textCount")]
    pub text_count: i32,
    #[serde(rename = "totalTokenCount")]
    pub total_token_count: i32,
}

impl Clone for CachedUsageMetadata {
    fn clone(&self) -> Self {
        CachedUsageMetadata {
            text_count: self.text_count,
            total_token_count: self.total_token_count,
        }
    }
}

impl std::fmt::Display for CachedUsageMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Text Count: {}, Total Token Count: {}",
            self.text_count, self.total_token_count
        )
    }
}

//
// Reponse structure for creating cached content
//

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheResponse {
    pub name: String,
    pub model: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "createTime")]
    pub create_time: String,
    #[serde(rename = "updateTime")]
    pub update_time: String,
    #[serde(rename = "expireTime")]
    pub expire_time: String,
    #[serde(rename = "usageMetadata")]
    pub usage: Option<CachedUsageMetadata>,
}

#[derive(Serialize, Deserialize, Debug)]
#[warn(dead_code)]
pub struct CachedContentList {
    #[serde(rename = "cachedContents")]
    pub cached_contents: Option<Vec<CachedContentItem>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[warn(dead_code)]
pub struct CachedContentItem {
    pub name: String,       // e.g. "projects/.../cachedContents/123"
    pub model: Option<String>,
    #[serde(rename = "createTime")]
    pub create_time: Option<String>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
    #[serde(rename = "expireTime")]
    pub expire_time: Option<String>,
    #[serde(rename = "usageMetadata")]
    pub usage: Option<CachedUsageMetadata>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

#[allow(dead_code)]
pub enum CachingRequest {
    List,
    Create {
        model_id: String,
        nb_propositions: usize,
        ttl: Option<String>,
    },
    DeleteAll,
    UpdateTTL {
        cache_name: String,
        ttl: String,
    },
}

#[allow(dead_code)]
pub enum CachingResponse {
    List(CachedContentList),
    Create(CacheResponse),
    DeleteAll,
    UpdateTTL(CacheResponse),
}

#[allow(dead_code)]
impl CachingRequest {
    pub async fn execute(&self) -> Result<CachingResponse, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            CachingRequest::List => {
                let cached_contents = list_cached_contents().await?;
                Ok(CachingResponse::List(cached_contents))
            }
            CachingRequest::Create { model_id, nb_propositions, ttl } => {
                let cache_response = async_gemini_create_cached_content(model_id, *nb_propositions, ttl.clone()).await?;
                Ok(CachingResponse::Create(cache_response))
            }
            CachingRequest::DeleteAll => {
                async_gemini_delete_all_cached_contents().await?;
                Ok(CachingResponse::DeleteAll)
            }
            CachingRequest::UpdateTTL { cache_name, ttl } => {
                let updated_cache = async_gemini_update_cached_content_ttl(cache_name, ttl.clone()).await?;
                Ok(CachingResponse::UpdateTTL(updated_cache))
            }
        }
    }
}

#[allow(dead_code)]
const MODEL_ID: &str = "gemini-2.5-flash";
const REGION: &str = "us-central1";

fn get_gcloud_access_token() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let output = Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // You can match network-related messages here
        if stderr.contains("Unable to reach the server")
            || stderr.contains("Network error")
            || stderr.contains("connection")
            || stderr.contains("timed out")
        {
            return Err(format!("Network error from gcloud: {}", stderr).into());
        }

        return Err(format!("gcloud failed: {}", stderr).into());
    }

    let token = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(token)
}

 pub async fn list_cached_contents() -> 
 Result<CachedContentList, Box<dyn std::error::Error + Send + Sync>> {

    let access_token = get_gcloud_access_token()?;

    let url = format!(
        "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/cachedContents",
        REGION, get_project_id(), REGION
    );

    let client = Client::new();

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .send()
        .await {
            Err(e) => {
                return Err(format!("Network error while listing cached contents: {}", e).into());
            },
            Ok(resp) => resp,
        };

    let body = resp.text().await?;
    // println!("List cached contents response body: {}", body);
    let resp: CachedContentList = serde_json::from_str(&body)?;
    // println!("Parsed cached contents: {:?}", resp);

    Ok(resp)
}

pub async fn async_gemini_create_cached_content(model_id: &String, nb_propositions: usize, ttl: Option<String>) -> Result<CacheResponse, Box<dyn Error + Send + Sync>> {
    let token = get_gcloud_access_token()?;
    let real_model_path = format!("projects/{}/locations/{}/publishers/google/models/{}", get_project_id(), REGION, model_id);

    let url = format!(
        "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/cachedContents",
        REGION, get_project_id(), REGION
    );

    let client: Client = Client::new();

    let request = 
    CacheRequest {
        model: real_model_path.into(),
        display_name: "CACHE_DISPLAY_NAME".into(),
        contents: vec![
            CachedContent {
                // Utiliser "user" pour le contexte
                role: "user".to_string(), 
                parts: vec![
                    CachedPart::TextPart {
                        text: generate_cached_prompt(nb_propositions),
                    }
                ],
            }
        ],
        ttl: ttl,
    };

    let resp = match client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await {
            Err(e) => {
                return Err(format!("Network error while creating cached content: {}", e).into());
            },
            Ok(resp) => resp,
        };

    let body = resp.text().await?;
    //println!("Cache creation response body: {}", body);
    let resp: CacheResponse = serde_json::from_str(&body)?;

    Ok(resp)
}

#[allow(dead_code)]
pub async fn async_gemini_delete_all_cached_contents() -> Result<(), Box<dyn Error + Send + Sync>> {
    let token = get_gcloud_access_token()?;

    let cached_contents = list_cached_contents().await?;

    let client: Client = Client::new();

    if let Some(contents) = cached_contents.cached_contents {
        for content in contents {
            let url = format!(
                "https://{}-aiplatform.googleapis.com/v1/{}",
                REGION, content.name
            );

            let resp = match client
                .delete(&url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .send()
                .await {
                    Err(e) => {
                        return Err(format!("Network error while deleting cached content: {}", e).into());
                    },
                    Ok(resp) => resp,
                };

            if resp.status().is_success() {
                println!("Deleted cached content: {}", content.name);
            } else {
                let body = resp.text().await?;
                eprintln!("Failed to delete cached content {}: {}", content.name, body);
            }
        }
    } else {
        println!("No cached contents to delete.");
    }

    Ok(())
}  

#[allow(dead_code)]
pub async fn async_gemini_update_cached_content_ttl(cache_name: &String, ttl: String) -> Result<CacheResponse, Box<dyn std::error::Error + Send + Sync>> {
    let token = get_gcloud_access_token()?;

    let url = format!(
        "https://{}-aiplatform.googleapis.com/v1/{}",
        REGION, cache_name
    );

    let client: Client = Client::new();

    let request = 
    CacheRequest {
        model: "".into(), // Le modèle n'est pas mis à jour
        display_name: "".into(), // Le nom d'affichage n'est pas mis à jour
        contents: vec![], // Le contenu n'est pas mis à jour
        ttl: Some(ttl),
    };

    let resp = match client
        .patch(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await {
            Err(e) => {
                return Err(format!("Network error while updating cached content TTL: {}", e).into());
            },
            Ok(resp) => resp,
        };

    let body = resp.text().await?;
    //println!("Cache update response body: {}", body);
    let resp: CacheResponse = serde_json::from_str(&body)?;

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_async_gemini_create_cached_content() {
        let nb_propositions = 3;
        let response = async_gemini_create_cached_content(&MODEL_ID.to_string(), nb_propositions, Some("120s".into())).await;
        match response {
            Ok(resp) => {
                println!("Cache created successfully: {:?}", resp);
                assert_eq!(resp.model, format!("projects/ultra-evening-480109-i2/locations/us-central1/publishers/google/models/{}", MODEL_ID));
            },
            Err(e) => {
                eprintln!("Error creating cache: {}", e);
                assert!(false);
            }
        }
    }

    #[tokio::test]
    async fn test_list_cached_contents() {
        let nb_propositions = 3;
        let _ = async_gemini_create_cached_content(&MODEL_ID.to_string(), nb_propositions, Some("120s".into())).await;
        let response = list_cached_contents().await;
        match response {
            Ok(info) => {
                println!("Cached contents retrieved successfully: {:?}", info);
                assert!(info.cached_contents.is_some());
            },
            Err(e) => {
                eprintln!("Error retrieving cache info: {}", e);
                assert!(false);
            }
        }
    }

    #[tokio::test]
    async fn test_async_gemini_delete_all_cached_contents() {
        let nb_propositions = 3;
        let _ = async_gemini_create_cached_content(&MODEL_ID.to_string(), nb_propositions, Some("120s".into())).await;
        let response = async_gemini_delete_all_cached_contents().await;
        match response {
            Ok(_) => {
                println!("All cached contents deleted successfully.");
                assert!(true);
            },
            Err(e) => {
                eprintln!("Error deleting cached contents: {}", e);
                assert!(false);
            }
        }

        let response = list_cached_contents().await;
        match response {
            Ok(info) => {
                println!("Cached contents after deletion: {:?}", info);
                assert!(info.cached_contents.is_none() || info.cached_contents.unwrap().is_empty());
            },
            Err(e) => {
                eprintln!("Error retrieving cache info after deletion: {}", e);
                assert!(false);
            }
        }
    }

    #[tokio::test]
    async fn test_async_gemini_update_cached_content_ttl() {
        let nb_propositions = 3;
        let create_response = async_gemini_create_cached_content(&MODEL_ID.to_string(), nb_propositions, Some("60s".into())).await;
        match create_response {
            Ok(resp) => {
                println!("Cache created successfully for TTL update test: {:?}", resp);
                let cache_name = resp.name;
                let update_response = async_gemini_update_cached_content_ttl(&cache_name, "300s".into()).await;
                match update_response {
                    Ok(updated_resp) => {
                        println!("Cache TTL updated successfully: {:?}", updated_resp);
                        assert_eq!(updated_resp.name, cache_name);
                    },
                    Err(e) => {
                        eprintln!("Error updating cache TTL: {}", e);
                        assert!(false);
                    }
                }
            },
            Err(e) => {
                eprintln!("Error creating cache for TTL update test: {}", e);
                assert!(false);
            }
        }
    }
}