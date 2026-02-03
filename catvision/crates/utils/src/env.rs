use once_cell::sync::Lazy;

static API_KEY: Lazy<String> = Lazy::new(|| {
    std::env::var("MY_GEMINI_API_KEY")
        .expect("Set MY_GEMINI_API_KEY environment variable")
});

static PROJECT_ID: Lazy<String> = Lazy::new(|| {
    std::env::var("MY_GEMINI_PROJECT_ID")
        .expect("Set MY_GEMINI_PROJECT_ID environment variable")
});

pub fn get_api_key() -> &'static str {
    &API_KEY
}

pub fn get_project_id() -> &'static str {
    &PROJECT_ID
}