use std::fmt::Display;
use super::network::UsageMetadata;
use super::caching::CachedUsageMetadata;

pub struct CostResult {
    pub usd: f64,
    pub eur: f64,
    pub usage: UsageMetadata,
    eur_rate: f64,
    pub cache_saving: f64,
}

pub struct CacheCostResult {
    pub usage: CachedUsageMetadata,
    pub usd: f64,
    pub eur: f64,
    eur_rate: f64,}

impl Display for CostResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            
            "Usage Metadata: {}\n", self.usage
        )?;
        write!(
            f,
            "Cost in USD: ${:.6}\nCost in EUR: €{:.6}\nSaved by Cache in EUR: €{:.6}",
            self.usd, self.eur, self.cache_saving
        )
    }
}

impl Display for CacheCostResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cached Usage Metadata: {}\n", self.usage
        )?;
        write!(
            f,
            "Cost in USD: ${:.6}\nCost in EUR: €{:.6}\nExchange Rate (EUR/USD): {:.4}",
            self.usd, self.eur, self.eur_rate
        )
    }
}

impl CostResult {
    pub fn new(usage: &UsageMetadata) -> Self {
        CostResult {
            usd: 0.0,
            eur: 0.0,
            usage: usage.clone(),
            eur_rate: 0.92,
            cache_saving: 0.0,
        }
    }

    pub fn compute_cost(&self) -> CostResult {
        // Formula:
        // billable_tokens = (prompt - cached) + candidates + thoughts
        let billable_tokens =
            (self.usage.prompt_token_count - self.usage.cached_content_token_count.unwrap_or(0)).max(0)
            + self.usage.candidates_token_count
            + self.usage.thoughts_token_count.unwrap_or(0);

        let billable_tokens_f = billable_tokens as f64;

        // Pricing — adjust to real Gemini pricing
        const USD_PER_1K_TOKENS: f64 = 0.001;

        let cache_saving = (self.usage.cached_content_token_count.unwrap_or(0) as f64 / 1000.0) * USD_PER_1K_TOKENS * self.eur_rate;
        let usd = (billable_tokens_f / 1000.0) * USD_PER_1K_TOKENS;
        let eur = usd * self.eur_rate;

        CostResult {
            usd,
            eur,
            usage: self.usage.clone(),
            eur_rate: self.eur_rate,
            cache_saving,
        }
    }
}

impl CacheCostResult {
    pub fn new(usage: CachedUsageMetadata) -> Self {
        CacheCostResult {
            usd: 0.0,
            eur: 0.0,
            usage: usage,
            eur_rate: 0.92,
        }
    }

    pub fn compute_cost(&self) -> CacheCostResult {
        // Formula:
        let billable_tokens = self.usage.total_token_count;

        let billable_tokens_f = billable_tokens as f64;

        // Pricing — adjust to real Gemini pricing
        const USD_PER_1K_TOKENS: f64 = 0.002;
        const EUR_RATE: f64 = 0.93;

        let usd = (billable_tokens_f / 1000.0) * USD_PER_1K_TOKENS;
        let eur = usd * EUR_RATE;

        CacheCostResult {
            usd,
            eur,
            usage: self.usage.clone(),
            eur_rate: EUR_RATE,
        }
    }
}