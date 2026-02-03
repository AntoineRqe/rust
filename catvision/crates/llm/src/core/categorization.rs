use serde_json::Value;
use std::collections::HashMap;
use utils::category::{check_category_validity};

#[derive(Debug)]
pub enum DomainError {
    Missing,                    // domain not found
    NotArray,                   // domain exists but is not an array
    InvalidStrings,             // elements are not strings
    }

/// Parses the categorization output from the LLM and maps domains to their categories.
/// # Arguments
/// * `domains` - A slice of domain strings that were categorized.
/// * `content` - The raw JSON string output from the LLM containing categorization results.
/// # Returns
/// A Result containing a HashMap mapping each domain to a vector of category &str on success,
/// or an error message on failure.
/// 
pub fn parse_categorization_output(
    domains: Vec<String>,
    content: &str,
) -> Result<
        (HashMap<String, Vec<&'static str>>, HashMap<String, DomainError>),
        Box<dyn std::error::Error>> 
        {
    // Avoid unnecessary trim/allocation
    let content = content.trim_start_matches(|c: char| c.is_whitespace())
                         .trim_end_matches(|c: char| c.is_whitespace());

    
    let json: Value = serde_json::from_str(content)?;
    let obj = json.as_object().ok_or("Expected top-level JSON object")?;

    // Pre allocate the result map for better performance
    let mut result: HashMap<String, Vec<&'static str>> = HashMap::with_capacity(domains.len());
    let mut errors: HashMap<String, DomainError> = HashMap::new();

    for domain in domains {
        let value = match obj.get(&domain)
        {
            Some(v) => v,
            None => {
                errors.insert(domain.clone(), DomainError::Missing);
                continue;
            }
        };

        let arr = match value.as_array() {
            Some(arr) => arr,
            None => {
                errors.insert(domain.clone(), DomainError::NotArray);
                continue;
            }
        };

        if arr.is_empty() {
            errors.insert(domain.clone(), DomainError::InvalidStrings);
            continue;
        }

        // Collect categories as &str to avoid allocations
        let categories: Vec<String> = arr.iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();

        if categories.is_empty() {
            errors.insert(domain.clone(), DomainError::InvalidStrings);
            continue;
        }

        let mut categories_ref: Vec<&'static str> = Vec::with_capacity(categories.len());

        let mut invalid_found = false;

        for category in &categories {
            if let Some(valid_category) = check_category_validity(category) {
                categories_ref.push(valid_category);
            } else {
                errors.insert(domain.clone(), DomainError::InvalidStrings);
                invalid_found = true;
                break;
            }
        }

        if invalid_found {
            continue;
        }

        result.insert(domain.to_string(), categories_ref);
    }

    Ok((result, errors))
}