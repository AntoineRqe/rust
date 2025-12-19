# Domain Classifier

The **Domain Classifier** is a command-line tool that analyzes input data (typically CSV files) and predicts domain categories based on a configurable classification pipeline.

Its behavior is entirely controlled through a JSON configuration file, making it easy to adapt for development, testing, or production environments.

---

## Pre requisites

- Rust programming language (latest stable version)
- MY_GEMINI_API_KEY environment variable set for Gemini model access
- MY_GEMINI_PROJECT_ID environment variable set for Gemini project on google cloud
- gcloud CLI installed and authenticated for Google Cloud access

---

## Features

- Load and parse CSV input files
- Apply rule-based and/or ML-based domain classification
- Fully asynchronous processing
- Config-driven model selection, thresholds, features, and preprocessing
- Simple, flexible CLI interface
- Output results in various formats (JSON, CSV, HTML.)

---

## Usage

Run the classifier:

```bash
cargo run --release -- --input ~/microsoft.csv --config configs/config-prod.json
```

---

## Arguments

| Flag        | Description                             | Required |
|-------------|-----------------------------------------|----------|
| `--input`   | Path to the input CSV file              | Yes      |
| `--config`  | Path to the JSON configuration file     | Yes      |
| `--dict`    | Path to the dictionary file (optional)  | No       |
| `--verbose` | Enable verbose logging (optional)       | No       |

---

## Example Configuration (`config-prod.json`)

```json
{
    "max_threads": 3,
    "support_csv": {
        "input": true,
        "output": true
    },
    "support_html": {
        "input": false,
        "output": false
    },
    "max_domain_propositions": 1,
    "chunk_size": 100,
    "thinking_budget": 1024,
    "use_gemini_explicit_caching": true,
    "use_gemini_url_context": false,
    "use_gemini_google_search": false,
    "use_gemini_custom_cache_duration": "3600s",
    "model": [
        "gemini-2.5-flash",
        "Qwen2.5-Coder-32B-Instruct-AWQ",
        "Mistral-Small",
        "claude-sonnet-4"
    ]
}
```

---

## Example Input (CSV)

```csv
domain,llm_category_1
microsoft.com,Technology company headquartered in Redmond
xbox.com,Microsoft gaming division
linkedin.com,Professional social network
```

---

---

## Project Structure

```
.
├── Cargo.lock
├── Cargo.toml
├── configs
│   ├── config-debug.json
│   └── config-prod.json
├── README.md
└── src
    ├── category.rs
    ├── config
    │   ├── mod.rs
    │   └── test
    │       └── config.json
    ├── ctx.rs
    ├── format
    │   ├── csv.rs
    │   ├── html.rs
    │   └── mod.rs
    ├── llm
    │   ├── core
    │   │   └── mod.rs
    │   ├── mod.rs
    │   └── providers
    │       ├── gemini
    │       │   ├── billing.rs
    │       │   ├── caching.rs
    │       │   ├── generating.rs
    │       │   ├── mod.rs
    │       │   └── network.rs
    │       └── mod.rs
    ├── main.rs
    ├── my_traits.rs
    ├── statistics.rs
    └── utils
        ├── env.rs
        └── mod.rs
```

---

## Development

Build:

```bash
cargo build
```

Run with example data:

```bash
cargo run -- --input ./data/example.csv --config ./configs/config-prod.json
```
