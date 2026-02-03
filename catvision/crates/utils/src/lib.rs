use tldextract::{TldExtractor, TldOption, TldResult};
use std::collections::HashMap;

pub mod env;
pub mod category;

#[derive(Debug, Clone)]
/// Structure to hold various categories data for a domain
pub struct CatVisionData {
    pub category_olfeo: Option<&'static str>,
    pub categories_manual: Option<&'static str>,
    pub categories_llm: Option<Vec<&'static str>>,
    pub appsite_name_by_olfeo: Option<String>,
    pub appsite_name_by_gemini: Option<String>,
    pub description_fr_by_gemini: Option<String>,
    pub description_en_by_gemini: Option<String>,
}

impl CatVisionData {
    pub fn new(
        category_olfeo: Option<&'static str>,
        categories_manual: Option<&'static str>,
        categories_llm: Option<Vec<&'static str>>,
        appsite_name_by_olfeo: Option<String>,
        appsite_name_by_gemini: Option<String>,
        description_fr_by_gemini: Option<String>,
        description_en_by_gemini: Option<String>,
    ) -> Self {
        
        CatVisionData {
            category_olfeo,
            categories_manual,
            categories_llm,
            appsite_name_by_olfeo,
            appsite_name_by_gemini,
            description_fr_by_gemini,
            description_en_by_gemini,
        }
    }
}

/// Converts a duration in seconds to a human-readable string format
/// # Arguments
/// * `total_seconds` - Duration in seconds
/// # Returns
/// * `Option<String>` - Formatted duration string or None if input is zero
pub fn seconds_to_pretty(total_seconds: u64) -> Option<String> {

    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if days > 0 {
        Some(format!("{days} days {:02}:{:02}:{:02}", hours, minutes, seconds))
    } else {
        Some(format!("{:02}:{:02}:{:02}", hours, minutes, seconds))
    }
}

pub fn trim_domain_by_llm(dict: &HashMap<String, String>, domain: &str) -> (Option<String>, Option<String>) {
    //Trim subdomains of the given domain with the Olfeo method.
    // The basic idea of the Olfeo method is the following. As input take a domain,
    // e.g. "abc.teams.microsoft.com". Successively get categories for suffixes
    // of the domain, e.g. "abc.teams.microsoft.com", "teams.microsoft.com",
    // "microsoft.com". Keep doing this until you get a different category,
    // and then return the last suffix before the category changed.
    // For example, "abc.teams.microsoft.com" has category "VoIP, Internet Telephony",
    // "teams.microsoft.com" has category "VoIP, Internet Telephony", and
    // "microsoft.com" has category "Business Services", so the result will be
    // "teams.microsoft.com", because that's the last domain suffix before the
    // category changed.
    // Args:
    //     domain (str): Domain to be trimmed.
    //
    // Returns:
    //     str | None: Suffix of the given domain, or None if the input is not a valid
    //         domain.

    let ext: TldExtractor = TldOption::default().cache_path(".tld_cache").build();

    let root_domain = match ext.extract(domain) {
        Ok(TldResult { domain, suffix, .. }) => {
            if domain.is_none() || suffix.is_none() {
                String::new()
            } else {
                format!("{}.{}", domain.unwrap(), suffix.unwrap())
            }
        }
        _ => String::new(),
    };

    if root_domain.is_empty() {
        return (None, None);
    }

    let subelts = domain.split(".").collect::<Vec<&str>>();
    let mut llm_classification = None;
    let mut previous_subdomain = String::from(domain);

    for i in 0..subelts.len() {
        let subdomain = subelts[i..].join(".");
        let new_llm_classification = dict
        .get(&subdomain)
        .cloned()
        .unwrap_or_else(|| "Unknown".to_string());

        if llm_classification.is_none() {
            llm_classification = Some(new_llm_classification.clone());
        }

        let llm_classification_ref = llm_classification.as_ref().unwrap();
    
        if new_llm_classification != *llm_classification_ref && llm_classification_ref != "CDN et Non Définissable" {
            return (Some(previous_subdomain.clone()), dict
        .get(&previous_subdomain)
        .cloned());
        }
        
        if subdomain == root_domain {
            return (Some(root_domain.clone()), dict
        .get(&root_domain)
        .cloned());
        }

        previous_subdomain = subdomain;
        llm_classification = Some(new_llm_classification.clone());
    }

    (Some(root_domain), llm_classification)
}
