use std::fmt::Display;

#[derive(Debug)]
pub struct Statistics {
    domaine_count: usize,
    olfeo_match_count : usize,
    llm_level_match_count : Vec<usize>,
    priorized_done_count : usize,
    prioritized_match_count : usize,
    prioritized_done_success: usize,
}

impl Statistics {
    pub fn new(levels: usize) -> Self {
        Statistics {
            domaine_count: 0,
            olfeo_match_count: 0,
            llm_level_match_count: vec![0; levels],
            priorized_done_count: 0,
            prioritized_match_count: 0,
            prioritized_done_success: 0,
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

    pub fn generate_output_summary(&self) -> String {
        let mut summary = String::new();
        summary.push_str(&format!("Statistics with {} domains\n", self.domaine_count));
        summary.push_str(&format!("\t Olfeo match percentage: {:.2} %\n", self.process_olfeo_matches_percentage()));
        for level in 0..self.llm_level_match_count.len() {
            summary.push_str(&format!("\t Level {} match percentage: {:.2} %\n", level + 1, self.process_llm_level_matches_percentage(level)));
        }
        let total_percentage: usize = self.llm_level_match_count.iter().sum();
        summary.push_str(&format!("\t Total LLM match percentage: {:.2} %\n", (total_percentage as f64 / self.domaine_count as f64) * 100.0));
        summary.push_str(&format!("\t Prioritized match percentage: {:.2} %\n", self.process_prioritized_matches_percentage()));
        summary.push_str(&format!("\t Priorized done percentage: {:.2} %\n", self.process_priorized_done_percentage()));
        summary.push_str(&format!("\t Prioritized done success percentage: {:.2} %\n", self.process_prioritized_done_success_percentage()));
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
        assert!(summary.contains("Statistics with 1 domains"));
        assert!(summary.contains("Olfeo match percentage: 100.00 %"));
        assert!(summary.contains("Level 1 match percentage: 100.00 %"));
        assert!(summary.contains("Level 2 match percentage: 0.00 %"));
        assert!(summary.contains("Total LLM match percentage: 100.00 %"));
        assert!(summary.contains("Prioritized match percentage: 100.00 %"));
        assert!(summary.contains("Priorized done percentage: 100.00 %"));
        assert!(summary.contains("Prioritized done success percentage: 0.00 %"));
    }
}