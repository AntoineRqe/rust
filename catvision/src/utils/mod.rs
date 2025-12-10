use tldextract::{TldExtractor, TldOption, TldResult};
use std::collections::HashMap;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Ctx;
    use std::path::PathBuf;

    #[test]
    fn test_trim_domain_by_olfeo() {
        let input_domain = "gateway.bingviz.microsoftapp.net";
        let expected_output = Some("gateway.bingviz.microsoftapp.net".to_string());

        let config_path = None;
        let ctx = Ctx::new(&PathBuf::from("dummy_input.csv"), config_path, Some(PathBuf::from("/home/anr-cv/outputs/microsoft_subdomains.gemini-2.5-flash-chunk_100-thinking_1024.csv")));
        
        let result = trim_domain_by_llm(&ctx.dict.unwrap(), input_domain);
        assert_eq!(result.0, expected_output);
    }
}
