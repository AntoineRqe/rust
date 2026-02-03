use statistics::Statistics;
use std::path::PathBuf;
use std::any::Any;

/// Information for the HTML or other output formats
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

/// Trait for output formats (e.g., CSV, HTML, JSON)
pub trait Output: Send + Sync {
    /// Clones the boxed Output trait object
    fn clone_box(&self) -> Box<dyn Output>;
    /// Writes data to the output formats
    fn write(&mut self, data: &dyn std::any::Any, infos: &Infos) -> Result<(), Box<dyn std::error::Error>>;
    /// Creates the output header based on input headers and levels count
    fn create_output_header(&mut self, input_headers: &std::collections::HashMap<String, usize>, levels_count: usize);
    /// Creates a new instance of the output format handler
    fn new(filename: &PathBuf) -> Result<Self, Box<dyn std::error::Error>>
    where
        Self: Sized; // Need the Sized bound for constructors because they return Self
}

impl Clone for Box<dyn Output> {
    fn clone(&self) -> Box<dyn Output> {
        self.clone_box()
    }
}

/// Trait for input formats (e.g., CSV, HTML, JSON)
pub trait Input: Send + Sync {
    /// Clones the boxed Input trait object
    fn clone_box(&self) -> Box<dyn Input>;
    /// Parses the input data and returns it as a boxed Any type
    fn parse(&mut self, stats: &mut Statistics, dict: Option<&std::collections::HashMap<String, String>>) -> Result<Box<dyn std::any::Any>, Box<dyn std::error::Error>>;
   
    /// Creates a new instance of the input format handler
    fn new(filename: &PathBuf) -> Self
    where
        Self: Sized; // Need the Sized bound for constructors because they return Self
    
    /// Returns a reference to the underlying Any type for downcasting
    fn as_any(&self) -> &dyn Any;
    /// Returns a mutable reference to the underlying Any type for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl Clone for Box<dyn Input> {
    fn clone(&self) -> Box<dyn Input> {
        self.clone_box()
    }
}