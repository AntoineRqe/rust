use std::collections::HashMap;

#[derive(Debug)]
pub enum DomainError {
    Missing,                    // domain not found
    NotArray,                   // domain exists but is not an array
    InvalidLength,       // array length != 2
    InvalidStrings,             // elements are not strings
    PurposeNotFound,            // purpose not found in description
}


/// Strips JSON code fences (```json ... ```) from the input string.
/// # Arguments
/// * `input` - The input string potentially containing JSON code fences.
/// # Returns
/// A &str slice with the code fences removed.
/// 
fn strip_json_fence(input: &str) -> &str {
    let trimmed = input.trim();

    // Fast path: no code fence
    if !trimmed.starts_with("```") {
        return trimmed;
    }

    // Remove opening fence line (``` or ```json)
    let without_opening = match trimmed.find('\n') {
        Some(idx) => &trimmed[idx + 1..],
        None => return trimmed, // malformed fence, return as-is
    };

    // Remove closing fence if present
    let without_closing = without_opening
        .trim_end()
        .strip_suffix("```")
        .unwrap_or(without_opening);

    without_closing.trim()
}

/// Parses the description output from the LLM and maps domains to their descriptions.
/// # Arguments
/// * `domains` - A slice of domain strings that were described.
/// * `content` - The raw JSON string output from the LLM containing description results.
/// # Returns
/// A Result containing a HashMap mapping each domain to a HashMap of language keys and descriptions on success,
/// or an error message on failure.
/// 
pub fn parse_description_output<'a>(
    domains: Vec<String>,
    content: &str,
) -> Result<
        (HashMap<String, HashMap<&'static str, String>>, HashMap<String, DomainError>),
        Box<dyn std::error::Error>> 
        {
    let content = content.trim();
    let cleaned = strip_json_fence(content);

    let json: serde_json::Value = serde_json::from_str(cleaned)?;
    let obj = json.as_object().ok_or("Expected top-level JSON object")?;

    let mut result = HashMap::with_capacity(domains.len());
    let mut errors = HashMap::new();


    for domain in domains {
        match obj.get(&domain) {
            Some(value) => match value.as_array() {
                Some(array) => {
                    if array.len() != 2 {
                        errors.insert(domain, DomainError::InvalidLength);
                        continue;
                    }
                    let fr = array[0].as_str();
                    let en = array[1].as_str();
                    match (fr, en) {
                        (Some(fr), Some(en)) => {
                            if fr.is_empty() || en.is_empty() { // Empty description
                                errors.insert(domain, DomainError::InvalidStrings);
                                continue;
                            } else if fr == "Objectif non clair" || en == "Purpose unclear" { // LLM could not find purpose
                                errors.insert(domain, DomainError::PurposeNotFound);
                                continue;
                            } else {    // Valid descriptions
                                result.insert(domain.to_string(), HashMap::from([
                                    ("description_fr", fr.to_string()),
                                    ("description_en", en.to_string()),
                                ]));
                            }
                        }
                        _ => {
                            errors.insert(domain, DomainError::InvalidStrings);
                        }
                    }
                }
                None => {
                    errors.insert(domain, DomainError::NotArray);
                }
            },
            None => {
                errors.insert(domain, DomainError::Missing);
            }
        }
    }

    Ok((result, errors))
}
