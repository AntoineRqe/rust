use crate::statistics::Statistics;
use std::path::PathBuf;
use std::any::Any;

pub struct Infos {
    pub title: String,
    pub header: String,
    pub footer: String,
    pub levels_count: usize,
}

impl Infos {
    pub fn new(title: &str, header: &str, footer: &str, levels_count: usize) -> Self {
        Infos {
            title: title.to_string(),
            header: header.to_string(),
            footer: footer.to_string(),
            levels_count,
        }
    }
}

pub trait Output: Send + Sync {
    fn clone_box(&self) -> Box<dyn Output>;
    fn write(&mut self, data: &dyn std::any::Any, infos: &Infos) -> Result<(), Box<dyn std::error::Error>>;
    fn create_output_header(&mut self, input_headers: &std::collections::HashMap<String, usize>, levels_count: usize);
    fn new(filename: &PathBuf) -> Result<Self, Box<dyn std::error::Error>>
    where
        Self: Sized;
}

impl Clone for Box<dyn Output> {
    fn clone(&self) -> Box<dyn Output> {
        self.clone_box()
    }
}

pub trait Input: Send + Sync {
    fn clone_box(&self) -> Box<dyn Input>;
    fn parse(&mut self, stats: &mut Statistics, dict: Option<&std::collections::HashMap<String, String>>) -> Result<Box<dyn std::any::Any>, Box<dyn std::error::Error>>;
    fn new(filename: &PathBuf) -> Self
    where
        Self: Sized;
    
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl Clone for Box<dyn Input> {
    fn clone(&self) -> Box<dyn Input> {
        self.clone_box()
    }
}