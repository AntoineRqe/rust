use std::{collections::HashMap, fs::File, path::{PathBuf}};
use crate::{my_traits::{Infos, Input, Output}, statistics::Statistics};
use csv::{Reader, Writer};
use indexmap::IndexMap;
use csv::StringRecord;


#[derive(Debug)]
pub struct MyCSVInput {
    pub filename: PathBuf,
    pub headers: Vec<String>
}

impl Input for MyCSVInput {

    fn parse(&mut self, stats: &mut Statistics) -> Result<Box<dyn std::any::Any>, Box<dyn std::error::Error>> {
        let file: File = match File::open(&self.filename) {
            Err(e) => {
                eprintln!("Error opening file {}: {}", self.filename.display(), e);
                return Err(Box::new(e));
            },
            Ok(f) => f,
        };

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(b';') // Set the delimiter to tab
            .from_reader(file);

        let headers = self.parse_header(&mut rdr)?;
        println!("Parsed headers: {:?}", headers);
        let mut res = IndexMap::new();

        for result in rdr.records() {
            let record = result?;
            let domain = record.get(0).unwrap().trim();

            let olfeo_cat = record.get(*headers.get("old_category").expect("Old category header not found")).unwrap().trim();
            let expected_category = record.get(*headers.get("categories_manual").expect("Expected category manual header not found")).expect("Could not find expected category manual header").trim();
            
            if expected_category.contains(olfeo_cat) {
                stats.increment_olfeo_match_count();
            }
            // Assuming the first column is the domain and the fourth column is the expected category
            res.insert(domain.to_string(), vec![expected_category.to_string()]);
        }

        println!("Parsed {} records from {}", res.len(), self.filename.display());
        Ok(Box::new(res))
    }

    fn new(filename: &PathBuf) -> Self {
        MyCSVInput {
            filename: filename.to_path_buf(),
            headers: vec![]
        }
    }

}

impl MyCSVInput {
     fn parse_header(&mut self, rdr: &mut Reader<File>) -> Result<HashMap<String, usize>, Box<dyn std::error::Error>> {
        self.headers = rdr.headers()?.iter().map(|s| s.trim().to_string()).collect();
    
        let mut header_map = HashMap::new();
        
        for (index, header) in self.headers.iter().enumerate() {
            header_map.insert(header.clone(), index);
        }
        
        Ok(header_map)
    }
}

// OUTPUT
#[derive(Debug)]

pub struct MyCSVOutput {
    pub filename: PathBuf,
    pub original_header: Vec<String>,
}

impl Output for MyCSVOutput {

    fn write(&mut self, data : &dyn std::any::Any, infos: &Infos) -> Result<(), Box<dyn std::error::Error>> {
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(b'\t') // Set the delimiter to tab
            .from_path(self.filename.clone())?;

        Self::prepare_output_header(&mut wtr, self.generate_header(infos.levels_count))?;

        let data = data.downcast_ref::<IndexMap<String, Vec<String>>>().ok_or("Failed to downcast data to IndexMap<String, Vec<String>>")?;

        for (domain, categories) in data {
            let mut new_row = StringRecord::new();

            new_row.push_field(&domain);
            for cat in categories.iter() {
                new_row.push_field(&cat);
            }

            wtr.write_record(&new_row)?;
        }
            
        wtr.flush()?;

        println!("Output written to {}", self.filename.display());

        Ok(())
    }

    fn new(filename: &PathBuf) -> Self {

        MyCSVOutput { 
            filename: filename.to_path_buf(),
            original_header: vec![],
        }
    }
}

impl MyCSVOutput {
    pub fn generate_header(&self, levels_count: usize) -> StringRecord {
        let mut new_header = StringRecord::new();

        new_header.push_field("domain");
        new_header.push_field("expected_output_catvision");

        for i in 1..=levels_count {
            let cat_header = format!("cat_llm_{}", i);
            new_header.push_field(&cat_header);
        }

        new_header.push_field("prioritized_category");

        new_header
    }

    fn prepare_output_header(wtr : &mut Writer<File>, header : StringRecord) -> Result<(), Box<dyn std::error::Error>> {
        wtr.write_record(&header)?;
        Ok(())
    }
}