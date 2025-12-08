use std::{collections::HashMap, fs::File, path::{PathBuf}};
use crate::{my_traits::{Infos, Input, Output}, statistics::Statistics};
use csv::{Reader};
use indexmap::IndexMap;
use csv::StringRecord;
use itertools::Itertools;
use crate::CatVisionData;
use std::any::Any;


#[derive(Debug)]
pub struct MyCSVInput {
    pub filename: PathBuf,
    pub headers: HashMap<String, usize>,
}

impl Input for MyCSVInput {

    fn clone_box(&self) -> Box<dyn Input> {
        Box::new(Self {
            filename: self.filename.clone(),
            headers: self.headers.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

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

        self.parse_header(&mut rdr)?;
        let mut res: IndexMap<String, CatVisionData> = IndexMap::new();

        for record in rdr.records() {

            let record = record?;
            let mut new_data = CatVisionData::new(None, None, None, None, None);    
            let domain = record.get(*self.headers.get("domain").unwrap()).unwrap().trim();

            match self.headers.get("appsite_name") {
                Some(idx) => {
                    let appsite_name = record.get(*idx).unwrap().trim();
                    if !appsite_name.is_empty() {
                        new_data.appsite_name = Some(appsite_name.to_string());
                    }
                },
                None => {}
            }

            match self.headers.get("categories_manual") {
                Some(idx) => {
                    let expected_category = record.get(*idx).unwrap().trim();
                    if !expected_category.is_empty() {
                        new_data.categories_manual = Some(expected_category.to_string());
                    }
                },
                None => {}
            }

            match self.headers.get("old_category") {
                Some(idx) => {
                    let olfeo_cat = record.get(*idx).unwrap().trim();
                    if !olfeo_cat.is_empty() {
                        new_data.category_olfeo = Some(olfeo_cat.to_string());
                        if let Some(ref expected_category) = new_data.categories_manual {
                            if expected_category.contains(olfeo_cat) {
                                stats.increment_olfeo_match_count();
                            }
                        }
                    }
                },
                None => {}
            }
            // Assuming the first column is the domain and the fourth column is the expected category
            res.insert(domain.to_string(), new_data);
        }

        Ok(Box::new(res))
    }

    fn new(filename: &PathBuf) -> Self {
        MyCSVInput {
            filename: filename.to_path_buf(),
            headers: HashMap::new(),
        }
    }

}

impl MyCSVInput {
     fn parse_header(&mut self, rdr: &mut Reader<File>) -> Result<(), Box<dyn std::error::Error>> {
        let headers: Vec<String> = rdr.headers()?.iter().map(|s| s.trim().to_string()).collect();
    
        let mut header_map = HashMap::new();
        
        for (index, header) in headers.iter().enumerate() {
            header_map.insert(header.clone(), index);
        }
        
        // Required headers
        let required_headers = vec!["domain"];

        for req_header in required_headers {
            if !header_map.contains_key(req_header) {
                return Err(format!("Required header '{}' not found in CSV file", req_header).into());
            }
        }

        self.headers = header_map;

        Ok(())
    }
}

// OUTPUT
#[derive(Debug)]

pub struct MyCSVOutput {
    pub filename: PathBuf,
    pub headers: HashMap<String, usize>,
}

impl Output for MyCSVOutput {

    fn clone_box(&self) -> Box<dyn Output> {
        Box::new(Self {
            filename: self.filename.clone(),
            headers: self.headers.clone(),
        })
    }

    fn write(&mut self, data : &dyn std::any::Any, _infos: &Infos) -> Result<(), Box<dyn std::error::Error>> {
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(b'\t') // Set the delimiter to tab
            .from_path(self.filename.clone())?;

        let headers = self.generate_header();
        wtr.write_record(&headers)?;

        let mut fails: usize = 0;

        let data = data.downcast_ref::<IndexMap<String, CatVisionData>>().ok_or("Failed to downcast data to IndexMap<String, Vec<String>>")?;

        for (domain, categories) in data {
            if fails > 10 {
                eprintln!("Too many failures while writing CSV output. Aborting.");
                break;
            }
        
            let mut new_row = StringRecord::new();

            for (header, _) in self.headers.iter().sorted_by_key(|(_header, idx)| *idx) {
                match header.as_str() {
                    "domain" => {
                        new_row.push_field(domain);
                    },
                    "appsite_name" => {
                        if let Some(ref appsite_name) = categories.appsite_name {
                            new_row.push_field(appsite_name);
                        } else {
                            new_row.push_field("");
                        }
                    },
                    "categories_manual" => {
                        if let Some(ref manual_cat) = categories.categories_manual {
                            new_row.push_field(manual_cat);
                        } else {
                            new_row.push_field("");
                        }
                    },
                    "category_olfeo" => {
                        if let Some(ref olfeo_cat) = categories.category_olfeo {
                            new_row.push_field(olfeo_cat);
                        } else {
                            new_row.push_field("");
                        }
                    },
                    "prioritized_category" => {
                        if let Some(ref prioritized_cat) = categories.prioritized_category {
                            new_row.push_field(prioritized_cat);
                        } else {
                            new_row.push_field("");
                        }
                    },
                    _ if header.starts_with("llm_category_") => {
                        let level_str = header.trim_start_matches("llm_category_");
                        if let Ok(level) = level_str.parse::<usize>() {
                            if let Some(ref llm_cats) = categories.categories_llm {
                                if let Some(cat) = llm_cats.get(level - 1) {
                                    new_row.push_field(cat);
                                } else {
                                    new_row.push_field("");
                                }
                            } else {
                                new_row.push_field("");
                            }
                        } else {
                            new_row.push_field("");
                        }
                    },
                    _ => {
                        new_row.push_field("");
                    }
                }
            }

            match wtr.write_record(&new_row) {
                Err(e) => {
                    eprintln!("Error writing record for domain {}: {}", domain, e);
                    fails += 1;
                    continue;
                },
                Ok(_) => (),
            }
        }

        match wtr.flush() {
            Err(e) => {
                eprintln!("Error flushing CSV writer: {}", e);
                return Err(Box::new(e));
            },
            Ok(_) => (),
        } 

        println!("Output written to {}", self.filename.display());

        Ok(())
    }

    fn new(filename: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = filename.parent() {
            std::fs::create_dir_all(parent)?;
        }

        Ok(MyCSVOutput { 
            filename: filename.to_path_buf(),
            headers: HashMap::new(),
        })
    }

    fn create_output_header(&mut self, input_headers: &HashMap<String, usize>, levels_count: usize) {
        let mut headers = input_headers.clone();
        let mut offset = headers.len();

        for i in 0..levels_count {
            let key = format!("llm_category_{}", i + 1);
            headers.insert(key, offset + i);
        }

        offset += levels_count;
    
        headers.insert("prioritized_category".to_string(), offset);
        self.headers = headers.clone();
    }
}

impl MyCSVOutput {


    pub fn generate_header(&self) -> StringRecord {
        let mut new_header = StringRecord::new();

        let mut headers: Vec<(&String, &usize)> = self.headers.iter().collect();
        headers.sort_by_key(|(_header, idx)| *idx);

        for (header, _) in headers {
            new_header.push_field(header);
        }

        new_header
    }
}