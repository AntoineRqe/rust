use crate::statistics::Statistics;
use std::path::PathBuf;

pub struct Infos {
    pub filename: PathBuf,
    pub title: String,
    pub header: String,
    pub footer: String,
    pub levels_count: usize,
}

impl Infos {
    pub fn new(filename: &PathBuf, title: &str, header: &str, footer: &str, levels_count: usize) -> Self {
        Infos {
            filename: filename.to_path_buf(),
            title: title.to_string(),
            header: header.to_string(),
            footer: footer.to_string(),
            levels_count,
        }
    }
}

pub trait Output {
    fn write(&mut self, data: &dyn std::any::Any, infos: &Infos) -> Result<(), Box<dyn std::error::Error>>;
    fn new(filename: &PathBuf) -> Self
    where
        Self: Sized;
}

pub trait Input{
    fn parse(&mut self, stats: &mut Statistics) -> Result<Box<dyn std::any::Any>, Box<dyn std::error::Error>>;
    fn new(filename: &PathBuf) -> Self
    where
        Self: Sized;
}

