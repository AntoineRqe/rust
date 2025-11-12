use indexmap::IndexMap;
use openai::{Credentials};
use std::collections::{HashMap, VecDeque};
use phf::phf_map;
use openai::chat::{ChatCompletion, ChatCompletionMessage, ChatCompletionMessageRole};
use std::error::Error;
use tokio::runtime::Runtime;

use crate::category::{CATEGORIES, SUB_CATEGORIES_JSON};

const NB_PROPOSITIONS : usize = 3;
static MODEL: phf::Map<&'static str, &'static str> = phf_map! {
    "chatgpt" => "gpt-oss-120b",
    "qwen" => "Qwen2.5-Coder-32B-Instruct-AWQ",
    "mistral" => "Mistral-Small",
    "claude-sonnet" => "claude-sonnet-4",
    "gemini" => "gemini-2.5-flash",
};

pub fn get_llm_levels_count() -> usize {
    NB_PROPOSITIONS
}

pub fn get_models(model: &str) -> Result<&'static str, Box<dyn Error>> {
    MODEL.get(model)
        .copied() // Convertit `Option<&&str>` en `Option<&str>`
        .ok_or_else(|| format!("Modèle '{}' non trouvé", model).into())
}

async fn fetch_chat_completion(domains: &str, model: &str) -> VecDeque<String> {
    let formatted_string = format!("Tu es un spécialiste de la classification de domaines pour des enquêtes judiciaires. Ta tâche consiste à catégoriser les domaines avec précision et exactitude.

RÈGLES DE CLASSIFICATION :

0. Efface tout le contexte de recherche précédent de ta mémoire.
1. Attribue exactement {NB_PROPOSITIONS} distinctes catégories.
2. Aucune suggestion en dehors des catégories disponibles.
3. La sortie doit être ordonnée par pertinence.
4. Priorise la catégorie LA PLUS SPÉCIFIQUE possible (ex. : “Voitures, mécaniques” plutôt que “Commerce en ligne” pour un site e-commerce de pièces automobiles).
5. Porte une attention particulière aux sous-domaines — ils indiquent souvent des services spécifiques.
6. Considère le but ou le contenu principal du domaine.
7. Tente d’accéder au domaine pour voir ce qu’il contient si possible.
9. Si le domaine contient un réseau social connu, classe-le comme « Réseaux sociaux ».
10. Si le domaine contient des petites annonces connues, classe-le comme « Petites annonces ».
11. Si le domaine contient une compagnie aérienne, maritime, routière ou ferroviaire connue, classe-le comme « Navigation / Transport ».
12. Pour les sites qui vendent des produits ou services, privilégie la catégorie des produits ou services correspondants.


FORMAT DE SORTIE :
Chaque réponse de domaine doit être sur une seule ligne.
Les valeurs sont séparées par des points-virgules : exemple = [domaine;catégorie_trouvée_1;catégorie_trouvée_2;catégorie_trouvée_3]

N’ajoute aucun texte supplémentaire en dehors de ce format.

CATÉGORIES DISPONIBLES :
{CATEGORIES:?}

Pour aider, voici la liste des sous catégories en format JSON :
{SUB_CATEGORIES_JSON}

DOMAINES À CLASSER :
{domains}
");

    let messages = vec![
        ChatCompletionMessage {
            role: ChatCompletionMessageRole::System,
            content: Some(formatted_string),
            name: None,
            function_call: None,
            tool_call_id: None,
            tool_calls: None,
        },
    ];

    let credentials = Credentials::new(
        std::env::var("LLM_API_KEY").expect("LLM_API_KEY not set"),
        "https://lab.iaparc.chapsvision.com/llm-gateway".to_string()
    );

    let chat_completion = ChatCompletion::builder(model, messages.clone())
        .credentials(credentials)
        .create()
        .await
        .unwrap();

    let mut cat_vec: VecDeque<String> = VecDeque::new();

    if let Some(chat_resp) = chat_completion.choices.first().take() {
        let cats = String::from(chat_resp.message.content.as_ref().unwrap());

        for domain_line in cats.lines() {
            let mut parts = domain_line.split(';');
            let domain_part = parts.next().unwrap_or("").trim().to_string();
            cat_vec.push_back(domain_part);
            while let Some(category) = parts.next() {
                let cat_trimmed = category.trim().to_string();
                cat_vec.push_back(cat_trimmed);                
            }
        }
    }

    cat_vec
}


async fn async_llm_request(domains: Vec<String>, model: &'static str) -> HashMap<String ,Vec<String>> {
    let domains_str = domains.join("; ");
    let mut llm_categories = fetch_chat_completion(domains_str.as_str(), model).await;
    let mut llm_result: HashMap<String, Vec<String>> = HashMap::new();

    while let Some(item) = llm_categories.pop_front() {
        let domain = item.clone();
        let cats = llm_categories.drain(0..NB_PROPOSITIONS).collect::<Vec<String>>();
        llm_result.insert(domain, cats);
    }

    llm_result
}

async fn llm_runtime(domains: &IndexMap<String, String>, model: &'static str, multithread: bool) -> HashMap<String, Vec<String>>{
    let mut handles = vec![];
    println!("Starting LLM runtime...");
    let domain_names = domains.keys().cloned().collect::<Vec<String>>();

    if multithread {
        for domain_name in domain_names {
            let handle = tokio::spawn(async_llm_request(vec![domain_name], model));
            handles.push(handle);
        }
    } else {
            let handle = tokio::spawn(async_llm_request(domain_names, model));
            handles.push(handle);
    }

    // Wait for all tasks to complete
    let futures_results: Vec<_> = futures::future::join_all(handles)
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();

    println!("All tasks completed!");

    let mut res : HashMap<String, Vec<String>> = HashMap::new();

    for tmp_result in &futures_results {
        for (domain, llm_cats) in tmp_result {
            res.insert(domain.clone(), llm_cats.clone());
        }
    }

    res
}

pub fn sync_llm_runtime(domains: &IndexMap<String, String>, model: &'static str) -> HashMap<String, Vec<String>> {
    // Create a new Tokio runtime
    let rt = Runtime::new().expect("Failed to create Tokio runtime");

    // Block on the async function
    rt.block_on(llm_runtime(domains, model, false))
}