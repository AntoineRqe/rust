use std::path::{Path, PathBuf};
use crate::config::Config;
use crate::my_traits::{Input, Output};
use crate::Statistics;
use crate::csv::{MyCSVInput, MyCSVOutput};
use crate::html;
use crate::my_traits;

pub struct Ctx
{   
    input_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
    inputs: Vec<Box<dyn Input>>,
    outputs: Vec<Box<dyn Output>>,
    pub stats: Statistics,
    pub config: Config,
    pub prompt: String,
}

fn extract_directory_from_path(file_path: &Path) -> Option<PathBuf> {
    file_path.parent().map(|parent| parent.to_path_buf())
}

impl Ctx
{
    pub fn new(input_path: &Path, config: Option<PathBuf>) -> Self {
        let config = Config::new(config);
        
        let mut ctx = Ctx {
            input_path: input_path.to_path_buf(),
            inputs: vec![],
            output_path: extract_directory_from_path(input_path).unwrap_or_else(|| PathBuf::from("/outputs/")).join("outputs"),
            outputs: vec![],
            stats: Statistics::new(config.max_domain_propositions),
            config: config,
            prompt: String::from(""),
        };

        if ctx.config.support_csv.input {
            println!("CSV input is enabled.");
            let input = MyCSVInput::new(&ctx.input_path);
            ctx.inputs.push(Box::new(input));
        }

        if ctx.config.support_csv.output {
            println!("CSV output is enabled.");
            let output = MyCSVOutput::new(&ctx.output_path.join(ctx.input_path.file_name().unwrap()).with_extension(format!("{}_{}.csv", ctx.config.model[0], "csv")));
            ctx.outputs.push(Box::new(output));
        }
        
        if ctx.config.support_html.input {
            println!("HTML input is enabled but not supported.");
        }

        if ctx.config.support_html.output {
            println!("HTML output is enabled.");
            let output = html::HTMLGenerator::new(&ctx.output_path.join(ctx.input_path.file_name().unwrap()).with_extension(format!("{}_{}.html", ctx.config.model[0], "html")));
            ctx.outputs.push(Box::new(output));
        }

        return ctx;
    }

    pub fn write(&mut self, data: &dyn std::any::Any) -> Result<(), Box<dyn std::error::Error>> {
        let infos = my_traits::Infos::new(
            
            &self.output_path.join("my_reference_").join(self.config.model[0].as_str()).join(self.config.chunk_size.to_string()).with_extension("html"),
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
        for input in &mut self.inputs {
            return input.parse(&mut self.stats);
        }
        Err("No input available".into())
    }
}