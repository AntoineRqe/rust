use std::{fmt::Display, sync::atomic::{AtomicUsize, Ordering}};
use crate::utils::seconds_to_pretty;
use atomic_float::AtomicF64;



#[derive(Debug, Clone)]
/// Statistics for the CatVision application
pub struct Statistics {
    /// Number of domains processed
    domaine_count: usize,
    /// Number of Olfeo matches found
    olfeo_match_count : usize,
    /// Number of LLM matches found at each level
    llm_level_match_count : Vec<usize>,
    /// Number of prioritized domains processed
    priorized_done_count : usize,
    /// Number of prioritized matches found
    prioritized_match_count : usize,
    /// Number of successful prioritized matches
    prioritized_done_success: usize,
    /// Total cost incurred
    pub cost : f64,
    /// Number of retries performed
    retried : usize,
    /// Number of failed requests
    failed : usize,
    /// LLM chunk size used
    chunk_size: usize,
    /// LLM thinking budget used
    thinking_budget: i64,
    /// Elapsed time for processing
    pub elapsed_time: std::time::Duration,
}

/// Methods for the Statistics struct
impl Statistics {
    /// Creates a new Statistics instance
    ///
    /// # Arguments
    ///
    /// * `levels` - Number of LLM levels.
    ///
    /// # Errors
    ///
    /// Panics if memory allocation fails.
    pub fn new(levels: usize) -> Self {
        Statistics {
            domaine_count: 0,
            olfeo_match_count: 0,
            llm_level_match_count: vec![0; levels],
            priorized_done_count: 0,
            prioritized_match_count: 0,
            prioritized_done_success: 0,
            cost: 0.0,
            retried: 0,
            failed: 0,
            chunk_size: 0,
            thinking_budget: 0,
            elapsed_time: std::time::Duration::new(0, 0),
        }
    }

    pub fn increment_domain_count (&mut self) {
        self.domaine_count += 1;
    }

    pub fn increment_olfeo_match_count (&mut self) {
        self.olfeo_match_count += 1;
    }

    pub fn increment_prioritized_match_count(&mut self) {
        self.prioritized_match_count += 1;
    }

    pub fn increment_llm_level_match_count (&mut self, level: usize) {
        if level < self.llm_level_match_count.len() {
            self.llm_level_match_count[level] += 1;
        }
    }

    pub fn increment_priorized_done_count(&mut self) {
        self.priorized_done_count += 1;
    }

    pub fn increment_prioritized_done_success(&mut self) {
        self.prioritized_done_success += 1;
    }

    pub fn process_prioritized_matches_percentage(&self) -> f64 {
        if self.domaine_count == 0 {
            0.0
        } else {
            (self.prioritized_match_count as f64 / self.domaine_count as f64) * 100.0
        }
    }

    pub fn process_prioritized_done_success_percentage(&self) -> f64 {
        if self.priorized_done_count == 0 {
            0.0
        } else {
            println!("prioritized_done_success: {}, priorized_done_count: {}", self.prioritized_done_success, self.priorized_done_count);
            (self.prioritized_done_success as f64 / self.priorized_done_count as f64) * 100.0
        }
    }

    pub fn process_priorized_done_percentage(&self) -> f64 {
        if self.domaine_count == 0 {
            0.0
        } else {
            (self.priorized_done_count as f64 / self.domaine_count as f64) * 100.0
        }
    }

    pub fn process_olfeo_matches_percentage(&self) -> f64 {
        if self.domaine_count == 0 {
            0.0
        } else {
            (self.olfeo_match_count as f64 / self.domaine_count as f64) * 100.0
        }
    }

    pub fn process_llm_level_matches_percentage(&self, level: usize) -> f64 {
        if self.domaine_count == 0 || level >= self.llm_level_match_count.len() {
            0.0
        } else {
            (self.llm_level_match_count[level] as f64 / self.domaine_count as f64) * 100.0
        }
    }

    pub fn update_llm_statistics(&mut self,
        cost: AtomicF64,
        retried: AtomicUsize,
        failed: AtomicUsize,
        chunk_size: usize,
        thinking_budget: i64
    ){
        self.cost += cost.load(Ordering::Relaxed);
        self.retried += retried.load(Ordering::Relaxed);
        self.failed += failed.load(Ordering::Relaxed);
        self.chunk_size = chunk_size;
        self.thinking_budget = thinking_budget;
    }

    /// Generates a summary of the statistics
    ///
    /// # Arguments
    ///
    /// * `levels` - Number of LLM levels.
    ///
    /// # Errors
    ///
    /// Panics if memory allocation fails.
    pub fn generate_output_summary(&self) -> String {
        let mut summary = String::new();
        summary.push_str(&format!("Statistics with {} domains\n", self.domaine_count));
        summary.push_str(&format!("\t Olfeo match percentage: {:.2} %\n", self.process_olfeo_matches_percentage()));
        for level in 0..self.llm_level_match_count.len() {
            summary.push_str(&format!("\t Level {} match percentage: {:.2} %\n", level + 1, self.process_llm_level_matches_percentage(level)));
        }
        let total_percentage: usize = self.llm_level_match_count.iter().sum();
        summary.push_str(&format!("\t Total LLM match percentage: {:.2} %\n", (total_percentage as f64 / self.domaine_count as f64) * 100.0));
        summary.push_str(&format!("\t LLM cost: {:.6}\n", self.cost));
        summary.push_str(&format!("\t LLM retried: {}\n", self.retried));
        summary.push_str(&format!("\t LLM failed: {}\n", self.failed));
        summary.push_str(&format!("\t LLM chunk size: {}\n", self.chunk_size));
        summary.push_str(&format!("\t LLM thinking budget: {}\n", self.thinking_budget));
        summary.push_str(&format!("\t Elapsed time : {}\n", seconds_to_pretty(self.elapsed_time.as_secs()).unwrap_or_else(|| format!("{:?}", self.elapsed_time))));
        summary.push_str("\t Estimated cost for 4000000 domains: ");
        let estimated_cost = if self.domaine_count > 0 {
            (4000000.0 / self.domaine_count as f64) * self.cost
        } else {
            0.0
        };
        summary.push_str(&format!("{:.6}\n", estimated_cost));

        summary.push_str("\t Estimated time for 4000000 domains: ");
        let estimated_time = if self.domaine_count > 0 {
            self.elapsed_time.mul_f64(4000000.0 / self.domaine_count as f64)
        } else {
            std::time::Duration::new(0, 0)
        };
        summary.push_str(&format!("{}\n", seconds_to_pretty(estimated_time.as_secs()).unwrap_or_else(|| format!("{:?}", estimated_time))));



        summary
    }
}

impl Display for Statistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut total_percentage = 0;
        write!(f, "Statistics with {} domains ", self.domaine_count)?;
        write!(f, "\n\t Olfeo match percentage: {:.2} %", self.process_olfeo_matches_percentage())?;
        for level in 0..self.llm_level_match_count.len() {
            total_percentage += self.llm_level_match_count[level];
            write!(f, "\n\t Level {} match percentage: {:.2} %", level + 1, self.process_llm_level_matches_percentage(level))?;
        }
        write!(f, "\n\t Total LLM match percentage: {:.2} %", (total_percentage as f64 / self.domaine_count as f64) * 100.0)?;
        write!(f, "\n\t Prioritized match percentage: {:.2} %", self.process_prioritized_matches_percentage())?;
        write!(f, "\n\t Priorized done percentage: {:.2} %", self.process_priorized_done_percentage())?;
        write!(f, "\n\t Prioritized done success percentage: {:.2} %", self.process_prioritized_done_success_percentage())?;
        write!(f, "\n\t LLM cost: {:.2}", self.cost)?;
        write!(f, "\n\t LLM retried: {}", self.retried)?;
        write!(f, "\n\t LLM failed: {}", self.failed)?;
        write!(f, "\n\t LLM chunk size: {}", self.chunk_size)?;
        write!(f, "\n\t LLM thinking budget: {}", self.thinking_budget)?;
        write!(f, "\n\t Elapsed time: {:?}", self.elapsed_time)?;

        Ok(())

    }    
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_statistics_initialization() {
        let stats = Statistics::new(3);
        assert_eq!(stats.domaine_count, 0);
        assert_eq!(stats.olfeo_match_count, 0);
        assert_eq!(stats.llm_level_match_count, vec![0, 0, 0]);
        assert_eq!(stats.priorized_done_count, 0);
        assert_eq!(stats.prioritized_match_count, 0);
    }

    #[test]
    fn test_statistics_increment() {
        let mut stats = Statistics::new(2);
        stats.increment_domain_count();
        stats.increment_olfeo_match_count();
        stats.increment_llm_level_match_count(0);
        stats.increment_llm_level_match_count(1);
        stats.increment_priorized_done_count();
        stats.increment_prioritized_match_count();
        assert_eq!(stats.domaine_count, 1);
        assert_eq!(stats.olfeo_match_count, 1);
        assert_eq!(stats.llm_level_match_count, vec![1, 1]);
        assert_eq!(stats.priorized_done_count, 1);
        assert_eq!(stats.prioritized_match_count, 1);
    }
    #[test]
    fn test_statistics_percentage_calculation() {
        let mut stats = Statistics::new(2);
        stats.increment_domain_count();
        stats.increment_olfeo_match_count();
        stats.increment_llm_level_match_count(0);
        stats.increment_priorized_done_count();
        stats.increment_prioritized_match_count();
        assert_eq!(stats.process_olfeo_matches_percentage(), 100.0);
        assert_eq!(stats.process_llm_level_matches_percentage(0), 100.0);
        assert_eq!(stats.process_llm_level_matches_percentage(1), 0.0);
        assert_eq!(stats.process_priorized_done_percentage(), 100.0);
        assert_eq!(stats.process_prioritized_matches_percentage(), 100.0);
    }

    #[test]
    fn test_statistics_output_summary() {
        let mut stats = Statistics::new(2);
        stats.increment_domain_count();
        stats.increment_olfeo_match_count();
        stats.increment_llm_level_match_count(0);
        stats.increment_priorized_done_count();
        stats.increment_prioritized_match_count();
        let summary = stats.generate_output_summary();
        println!("{}", summary);
        assert!(summary.contains("Statistics with 1 domains"));
        assert!(summary.contains("Olfeo match percentage: 100.00 %"));
        assert!(summary.contains("Level 1 match percentage: 100.00 %"));
        assert!(summary.contains("Level 2 match percentage: 0.00 %"));
        assert!(summary.contains("Total LLM match percentage: 100.00 %"));
        assert!(summary.contains("LLM cost: 0.000000"));
        assert!(summary.contains("LLM retried: 0"));
        assert!(summary.contains("LLM failed: 0"));
        assert!(summary.contains("Elapsed time : 00:00:00"));
        assert!(summary.contains("Estimated cost for 4000000 domains: 0.000000"));
        assert!(summary.contains("Estimated time for 4000000 domains: 00:00:00"));
    }
}