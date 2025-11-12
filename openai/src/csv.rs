use std::fs::File;
use crate::my_traits::{Input, Output};
use csv::{Reader, Writer};
use indexmap::IndexMap;
use csv::StringRecord;
use crate::llm::get_llm_levels_count;

pub struct MyCSVInput {
    pub filename: String,
    pub headers: Vec<String>
}

pub struct MyCSVOutput {
    pub filename: String,
    pub original_header: Vec<String>,
}

impl Input for MyCSVInput {
    type Output = IndexMap<String, String>;

    fn parse(&mut self) -> Result<Self::Output, Box<dyn std::error::Error>> {
        let file: File = match File::open(self.filename.as_str()) {
            Err(e) => {
                eprintln!("Error opening file {}: {}", self.filename, e);
                return Err(Box::new(e));
            },
            Ok(f) => f,
        };

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(b'\t') // Set the delimiter to tab
            .from_reader(file);

        let (domain_index, expected_index) = self.parse_header(&mut rdr)?;

        let mut res = IndexMap::new();

        for result in rdr.records() {
            let record = result?;

            let expected: &str = record.get(expected_index).unwrap().trim();
            let domain = record.get(domain_index).unwrap().trim();
            
            // Assuming the first column is the domain and the fourth column is the expected category
            res.insert(domain.to_string(), expected.to_string());
        }

        Ok(res)
    }

}

impl MyCSVInput {
     fn parse_header(&mut self, rdr: &mut Reader<File>) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        self.headers = rdr.headers()?.iter().map(|s| s.trim().to_string()).collect();

        let domain_index = self.headers.iter().position(|h| h == "domain").ok_or("No 'domain' header found")?;
        let expected_index = self.headers.iter().position(|h| h == "expected_output_catvision").ok_or("No 'expected_output_catvision' header found")?;
        
        Ok((domain_index, expected_index))
    }

    pub fn new(filename: &str) -> Self {
        MyCSVInput {
            filename: filename.to_string(),
            headers: vec![]
        }
    }
}

impl Output for MyCSVOutput {
    type Data = IndexMap<String, Vec<String>>;

    fn write(&mut self, data : &Self::Data) -> Result<(), Box<dyn std::error::Error>> {
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(b'\t') // Set the delimiter to tab
            .from_path(self.filename.clone())?;

        Self::prepare_output_header(&mut wtr, self.generate_header(get_llm_levels_count()))?;

        for (domain, categories) in data {
            let mut new_row = StringRecord::new();

            new_row.push_field(&domain);
            for cat in categories.iter() {
                new_row.push_field(&cat);
            }

            println!("{:?}", new_row);
            wtr.write_record(&new_row)?;
        }
            
        wtr.flush()?;

        println!("Output written to {}", self.filename);

        Ok(())
    }   
}

impl MyCSVOutput {
    pub fn new(filename: String, original_header: Vec<String>) -> Self {

        MyCSVOutput { 
            filename,
            original_header,
        }
    }

    pub fn generate_header(&self, levels_count: usize) -> StringRecord {
        let mut new_header = StringRecord::new();

        new_header.push_field("domain");
        new_header.push_field("expected_output_catvision");

        for i in 1..=levels_count {
            let cat_header = format!("cat_llm_{}", i);
            new_header.push_field(&cat_header);
        }

        new_header
    }

    fn prepare_output_header(wtr : &mut Writer<File>, header : StringRecord) -> Result<(), Box<dyn std::error::Error>> {
        wtr.write_record(&header)?;
        Ok(())
    }
}