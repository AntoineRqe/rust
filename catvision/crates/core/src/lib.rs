use std::collections::HashMap;
use std::path::{Path, PathBuf};
use config::Config;
use traits::{Input, Output};
use statistics::{Statistics};
use format::csv::{MyCSVInput, MyCSVOutput};
use format::html;
use std::fs::File;

#[derive(Clone)]
pub struct Ctx
{   
    input_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
    inputs: Vec<Box<dyn Input>>,
    outputs: Vec<Box<dyn Output>>,
    pub stats: Statistics,
    pub config: Config,
    pub prompt: String,
    pub dict: Option<HashMap<String, String>>,
}

fn extract_directory_from_path(file_path: &Path) -> Option<PathBuf> {
    file_path.parent().map(|parent| parent.to_path_buf())
}

impl Ctx
{
    pub fn new(input_path: &Path, config: Option<PathBuf>, dict: Option<PathBuf>) -> Self {
        let config = Config::new(config);
        
        let mut ctx = Ctx {
            input_path: input_path.to_path_buf(),
            inputs: vec![],
            output_path: extract_directory_from_path(input_path).unwrap_or_else(|| PathBuf::from("/outputs/")).join("outputs"),
            outputs: vec![],
            stats: Statistics::new(config.max_domain_propositions),
            config: config,
            prompt: String::from(""),
            dict: None,
        };
            
        if ctx.config.support_csv.input {
            println!("CSV input is enabled.");
            let input = MyCSVInput::new(&ctx.input_path);
            ctx.inputs.push(Box::new(input));
        }

        if ctx.config.support_csv.output {
            println!("CSV output is enabled.");
            let output = MyCSVOutput::new(&ctx.output_path.join(ctx.input_path.file_name().unwrap()).with_extension(format!("{}-chunk_{}-thinking_{}.{}", ctx.config.model[0], ctx.config.chunk_size, ctx.config.thinking_budget, "csv")));
            ctx.outputs.push(Box::new(output.unwrap()));
        }
        
        if ctx.config.support_html.input {
            println!("HTML input is enabled but not supported.");
        }

        if ctx.config.support_html.output {
            println!("HTML output is enabled.");
            let output = html::HTMLGenerator::new(&ctx.output_path.join(ctx.input_path.file_name().unwrap()).with_extension(format!("{}-chunk_{}-thinking_{}.{}", ctx.config.model[0], ctx.config.chunk_size, ctx.config.thinking_budget, "html")));
            ctx.outputs.push(Box::new(output.unwrap()));
        }

        if dict.is_some() {
            let dict_path = dict.unwrap();
            let dict_map = ctx.load_dictionary(&dict_path);
            ctx.dict = match dict_map {
                Ok(d) => Some(d.clone()),
                Err(e) => {
                    eprintln!("Failed to load dictionary from {}: {}", dict_path.display(), e);
                    None
                }
            };
        }

        return ctx;
    }

    pub fn load_dictionary(&self, dict_path: &PathBuf) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
        let file: File = match File::open(dict_path) {
            Err(e) => {
                eprintln!("Error opening file {}: {}", dict_path.display(), e);
                return Err(Box::new(e));
            },
            Ok(f) => f,
        };

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(b';') // Set the delimiter to tab
            .from_reader(file);

        let headers: Vec<String> = rdr.headers()?.iter().map(|s| s.trim().to_string()).collect();
    
        let mut header_map = HashMap::new();
        
        for (index, header) in headers.iter().enumerate() {
            header_map.insert(header.clone(), index);
        }
        

        let mut res: HashMap<String, String> = HashMap::new();

        for record in rdr.records() {
            let record = record?;
            let domain = record.get(*header_map.get("domain").unwrap()).unwrap().trim();
            let category = record.get(*header_map.get("llm_category_1").unwrap()).unwrap().trim();

            // Assuming the first column is the domain and the fourth column is the expected category
            res.insert(domain.to_string(), category.to_string());
        }

        Ok(res)
    }

    pub fn write(&mut self, data: &dyn std::any::Any) -> Result<(), Box<dyn std::error::Error>> {
        let infos = traits::Infos::new(            
            &(self.config.model[0].clone() + " LLM Classification Results for " + &self.input_path.to_string_lossy()),
            &self.stats.generate_output_summary(),
            &self.prompt,
            self.config.max_domain_propositions,
        );
    
        for output in &mut self.outputs {
            if let Err(e) = output.write(data, &infos) {
                eprintln!("Error writing output: {}", e);
            }
        }
        Ok(())
    }

    pub fn parse(&mut self) -> Result<Box<dyn std::any::Any>, Box<dyn std::error::Error>> {
        let input = self.inputs.first_mut().ok_or("No input defined")?;
        let res = input.parse(&mut self.stats, self.dict.as_ref());

        let csv_input = input
            .as_any()
            .downcast_ref::<MyCSVInput>()
            .ok_or("Input is not a MyCSVInput")?;

        for output in &mut self.outputs {
            output.create_output_header(&csv_input.headers, self.config.max_domain_propositions);
        }

        res
    }
}