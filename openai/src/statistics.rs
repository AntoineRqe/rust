use std::fmt::Display;

pub struct Statistics {
    domaine_count: usize,
    iota_match_count : usize,
    llm_level_match_count : Vec<usize>
}

impl Statistics {
    pub fn new(levels: usize) -> Self {
        Statistics {
            domaine_count: 0,
            iota_match_count: 0,
            llm_level_match_count: vec![0; levels],
        }
    }

    pub fn increment_domain_count (&mut self) {
        self.domaine_count += 1;
    }

    pub fn increment_iota_match_count (&mut self) {
        self.iota_match_count += 1;
    }

    pub fn increment_llm_level_match_count (&mut self, level: usize) {
        if level < self.llm_level_match_count.len() {
            self.llm_level_match_count[level] += 1;
        }
    }

    pub fn process_iota_matches_percentage(&self) -> f64 {
        if self.domaine_count == 0 {
            0.0
        } else {
            (self.iota_match_count as f64 / self.domaine_count as f64) * 100.0
        }
    }

    pub fn process_llm_level_matches_percentage(&self, level: usize) -> f64 {
        if self.domaine_count == 0 || level >= self.llm_level_match_count.len() {
            0.0
        } else {
            (self.llm_level_match_count[level] as f64 / self.domaine_count as f64) * 100.0
        }
    }
}

impl Display for Statistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut total_percentage = 0;
        write!(f, "Statistics with {} domains ", self.domaine_count)?;
        write!(f, "\n\t Iota match percentage: {:.2} %", self.process_iota_matches_percentage())?;
        for level in 0..self.llm_level_match_count.len() {
            total_percentage += self.llm_level_match_count[level];
            write!(f, "\n\t Level {} match percentage: {:.2} %", level + 1, self.process_llm_level_matches_percentage(level))?;
        }
        write!(f, "\n\t Total LLM match percentage: {:.2} %", (total_percentage as f64 / self.domaine_count as f64) * 100.0)?;

        Ok(())

    }    
}