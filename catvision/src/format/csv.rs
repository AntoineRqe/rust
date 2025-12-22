use std::{collections::HashMap, fs::File, path::PathBuf};
use crate::{category::check_category_validity, my_traits::{Infos, Input, Output}, statistics::Statistics, utils::trim_domain_by_llm};
use csv::{Reader, StringRecord};
use indexmap::IndexMap;
use itertools::Itertools;
use crate::CatVisionData;
use std::any::Any;
use crate::category::main_domain_for;

/// CSV input handler.
#[derive(Debug)]
pub struct MyCSVInput {
    /// Path to the CSV file.
    pub filename: PathBuf,
    /// Mapping of header names to their column indices.
    pub headers: HashMap<String, usize>,
}

impl Input for MyCSVInput {
    /// Clone the input object.
    fn clone_box(&self) -> Box<dyn Input> {
        Box::new(Self {
            filename: self.filename.clone(),
            headers: self.headers.clone(),
        })
    }

    /// Get a reference to self as Any.
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Get a mutable reference to self as Any.
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// Parse the CSV file and return structured data.
    ///
    /// # Arguments
    ///
    /// * `stats` - Mutable reference to statistics object to track processing stats.
    /// * `dict` - Optional mapping used to enrich data with `appsite_name_by_gemini`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CSV cannot be opened, or if required headers are missing.
    fn parse(
        &mut self,
        stats: &mut Statistics,
        dict: Option<&HashMap<String, String>>,
    ) -> Result<Box<dyn Any>, Box<dyn std::error::Error>> {
        let file = File::open(&self.filename).map_err(|e| {
            eprintln!("Error opening file {}: {}", self.filename.display(), e);
            e
        })?;

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(b';')
            .from_reader(file);

        let mut input_headers = self.parse_header(&mut rdr)?;

        if dict.is_some() {
            input_headers.insert("appsite_name_by_gemini".to_string(), input_headers.len());
        }

        self.headers = input_headers;
        let mut res: IndexMap<String, CatVisionData> = IndexMap::new();

        for record in rdr.records() {
            let record = record?;
            let mut new_data = CatVisionData::new(None, None, None, None, None);
            let domain = record.get(*self.headers.get("domain").unwrap()).unwrap().trim();

            if let Some(dict) = dict {
                let (appsite_name_by_gemini, _) = trim_domain_by_llm(dict, domain);
                new_data.appsite_name_by_gemini = appsite_name_by_gemini;
            }

            if let Some(idx) = self.headers.get("appsite_name_by_olfeo") {
                let appsite_name = record.get(*idx).unwrap().trim();
                if !appsite_name.is_empty() {
                    new_data.appsite_name_by_olfeo = Some(appsite_name.to_string());
                }
            }

            if let Some(idx) = self.headers.get("category_by_olfeo") {
                let olfeo_category = record.get(*idx).unwrap().trim();
                if !olfeo_category.is_empty() {
                    new_data.category_olfeo = main_domain_for(olfeo_category);
                }
            }

            if let Some(idx) = self.headers.get("categories_manual") {
                let expected_category = record.get(*idx).unwrap().trim();
                if !expected_category.is_empty() {
                    new_data.categories_manual = check_category_validity(expected_category);
                }
            }

            if let Some(idx) = self.headers.get("old_category") {
                let olfeo_cat = record.get(*idx).unwrap().trim();
                if !olfeo_cat.is_empty() {
                    new_data.category_olfeo = check_category_validity(olfeo_cat);
                    if let Some(ref expected_category) = new_data.categories_manual {
                        if expected_category.contains(olfeo_cat) {
                            stats.increment_olfeo_match_count();
                        }
                    }
                }
            }

            res.insert(domain.to_string(), new_data);
        }

        Ok(Box::new(res))
    }

    /// Create a new `MyCSVInput` instance.
    ///
    /// # Arguments
    ///
    /// * `filename` - Path to the CSV file to parse.
    fn new(filename: &PathBuf) -> Self {
        MyCSVInput {
            filename: filename.to_path_buf(),
            headers: HashMap::new(),
        }
    }
}

impl MyCSVInput {
    /// Parse the CSV header and return a mapping of header names to column indices.
    ///
    /// # Arguments
    ///
    /// * `rdr` - CSV reader instance.
    ///
    /// # Errors
    ///
    /// Returns an error if required headers (e.g., `"domain"`) are missing.
    pub(crate) fn parse_header(
        &mut self,
        rdr: &mut Reader<File>,
    ) -> Result<HashMap<String, usize>, Box<dyn std::error::Error>> {
        let headers: Vec<String> = rdr.headers()?.iter().map(|s| s.trim().to_string()).collect();
        let mut header_map = HashMap::new();

        for (index, header) in headers.iter().enumerate() {
            header_map.insert(header.clone(), index);
        }

        let required_headers = vec!["domain"];
        for req_header in required_headers {
            if !header_map.contains_key(req_header) {
                return Err(format!("Required header '{}' not found in CSV file", req_header).into());
            }
        }

        Ok(header_map)
    }
}

/// CSV output handler.
#[derive(Debug)]
pub struct MyCSVOutput {
    /// Path to the CSV file.
    pub filename: PathBuf,
    /// Mapping of header names to column indices.
    pub headers: HashMap<String, usize>,
}

impl Output for MyCSVOutput {
    /// Clone the output object.
    fn clone_box(&self) -> Box<dyn Output> {
        Box::new(Self {
            filename: self.filename.clone(),
            headers: self.headers.clone(),
        })
    }

    /// Write structured data to the CSV file.
    ///
    /// # Arguments
    ///
    /// * `data` - Data as `IndexMap<String, CatVisionData>`.
    /// * `_infos` - Metadata info (unused here).
    ///
    /// # Errors
    ///
    /// Returns an error if the CSV cannot be written or flushed.
    fn write(&mut self, data: &dyn Any, _infos: &Infos) -> Result<(), Box<dyn std::error::Error>> {
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(b';')
            .from_path(&self.filename)?;

        let headers = self.generate_header();
        wtr.write_record(&headers)?;

        let mut fails = 0;
        let data = data
            .downcast_ref::<IndexMap<String, CatVisionData>>()
            .ok_or("Failed to downcast data to IndexMap<String, CatVisionData>")?;

        for (domain, categories) in data {
            if fails > 10 {
                eprintln!("Too many failures while writing CSV output. Aborting.");
                break;
            }

            let mut new_row = StringRecord::new();

            for (header, _) in self.headers.iter().sorted_by_key(|(_header, idx)| *idx) {
                match header.as_str() {
                    "domain" => new_row.push_field(domain),
                    "appsite_name_by_olfeo" => new_row.push_field(categories.appsite_name_by_olfeo.as_deref().unwrap_or("")),
                    "appsite_name_by_gemini" => new_row.push_field(categories.appsite_name_by_gemini.as_deref().unwrap_or("")),
                    "categories_manual" => new_row.push_field(categories.categories_manual.as_deref().unwrap_or("")),
                    "category_olfeo" => new_row.push_field(categories.category_olfeo.as_deref().unwrap_or("")),
                    _ if header.starts_with("llm_category_") => {
                        let level_str = header.trim_start_matches("llm_category_");
                        if let Ok(level) = level_str.parse::<usize>() {
                            let field = categories.categories_llm.as_ref()
                                .and_then(|llm| llm.get(level - 1))
                                .map(|s| *s)
                                .unwrap_or("");
                            new_row.push_field(field);
                        } else {
                            new_row.push_field("");
                        }
                    }
                    _ => new_row.push_field(""),
                }
            }

            if let Err(e) = wtr.write_record(&new_row) {
                eprintln!("Error writing record for domain {}: {}", domain, e);
                fails += 1;
            }
        }

        wtr.flush().map_err(|e| {
            eprintln!("Error flushing CSV writer: {}", e);
            e
        })?;

        println!("Output written to {}", self.filename.display());
        Ok(())
    }

    /// Create a new `MyCSVOutput` instance.
    ///
    /// # Arguments
    ///
    /// * `filename` - Path to output CSV file.
    ///
    /// # Errors
    ///
    /// Returns an error if parent directories cannot be created.
    fn new(filename: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = filename.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(MyCSVOutput { filename: filename.to_path_buf(), headers: HashMap::new() })
    }

    /// Create output headers including LLM category columns.
    ///
    /// # Arguments
    ///
    /// * `input_headers` - Mapping of input CSV headers.
    /// * `levels_count` - Number of LLM category levels to add.
    fn create_output_header(&mut self, input_headers: &HashMap<String, usize>, levels_count: usize) {
        let mut headers = input_headers.clone();
        let offset = headers.len();
        for i in 0..levels_count {
            headers.insert(format!("llm_category_{}", i + 1), offset + i);
        }
        self.headers = headers;
    }
}

impl MyCSVOutput {
    /// Generate the CSV header record based on the current headers mapping.
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
