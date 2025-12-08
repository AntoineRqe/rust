use indexmap::IndexMap;
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use crate::my_traits::Infos;
use crate::CatVisionData;
use itertools::Itertools;

#[derive(Debug)]
pub struct HTMLGenerator{
    filename: PathBuf,
    columns: HashMap<String, usize>,
}

impl crate::my_traits::Output for HTMLGenerator {

    fn clone_box(&self) -> Box<dyn crate::my_traits::Output> {
        Box::new(Self {
            filename: self.filename.clone(),
            columns: self.columns.clone(),
        })
    }

    fn write(&mut self, data: &dyn std::any::Any, infos: &Infos) -> Result<(), Box<dyn std::error::Error>> {
        let data = data.downcast_ref::<IndexMap<String, CatVisionData>>().ok_or("Failed to downcast data to IndexMap<String, Vec<String>>")?;
        generate_html_table(self.columns.clone(), data, infos, &self.filename)?;
        println!("HTML output written to {}", self.filename.display());
        Ok(())
    }

    fn new(filename: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = filename.parent() {
            std::fs::create_dir_all(parent)?;
        }

        Ok(HTMLGenerator {
            filename: filename.to_path_buf(),
            columns: HashMap::new(),
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
        self.columns = headers.clone();
    }
}

fn render_cell(html: &mut String, value: &str) {
    if value.contains("*RED*") {
        let clean = value.replace("*RED*", "");
        html.push_str(&format!("<td><span class='red'>{}</span></td>", clean));
    } else {
        html.push_str(&format!("<td>{}</td>", value));
    }
}

pub fn generate_html_table(
    columns: HashMap<String, usize>,
    data: &IndexMap<String, CatVisionData>,
    infos: &Infos,
    output_path: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    if data.is_empty() {
        return Err("Data is empty".into());
    }

    // Escape HTML formatting
    let header_html = infos.header.replace('\n', "<br>").replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;");
    let footer_html = infos.footer.replace('\n', "<br>").replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;");

    // Start HTML
    let mut html = String::from(
        r#"<!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
        "#,
    );

    html.push_str(&format!("<title>{}</title>", infos.title));

    html.push_str(
        r#"
        <style>
            table { width: 100%; border-collapse: collapse; margin: 20px 0; }
            th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }
            th { background-color: #f2f2f2; }
            .red { color: red; }
            .stats { font-style: italic; color: #666; margin-bottom: 20px; white-space: pre-line; }
            .header, .footer { margin: 20px 0; white-space: pre-line; }
        </style>
        </head>
        <body>
    "#,
    );

    html.push_str(&format!("<h1>{}</h1>", infos.title));

    // Header section
    html.push_str(&format!("<div class=\"header\">{}</div>", header_html));

    // HTML table start
    html.push_str("<table><thead><tr>");

    for (col_name, _) in columns.iter().sorted_by_key(|(_, idx)| *idx) {
        html.push_str(&format!("<th>{}</th>", col_name));
    }

    html.push_str("</tr></thead><tbody>");

    // Rows
    for (domain, categories) in data.iter().sorted_by_key(|(row, _)| *row) {
        html.push_str("<tr>");

        for (col_name, _) in columns.iter().sorted_by_key(|(_, idx)| *idx) {
            match col_name.as_str() {
                "domain" => render_cell(&mut html, domain),

                "appsite_name" => render_cell(
                    &mut html,
                    categories.appsite_name.as_deref().unwrap_or("")
                ),

                "categories_manual" => render_cell(
                    &mut html,
                    categories.categories_manual.as_deref().unwrap_or("")
                ),

                "category_olfeo" => render_cell(
                    &mut html,
                    categories.category_olfeo.as_deref().unwrap_or("")
                ),

                "prioritized_category" => render_cell(
                    &mut html,
                    categories.prioritized_category.as_deref().unwrap_or("")
                ),

                other if other.starts_with("llm_category_") => {
                    let level = other.trim_start_matches("llm_category_").parse::<usize>().unwrap_or(0);
                    let cell = categories
                        .categories_llm
                        .as_ref()
                        .and_then(|v| v.get(level - 1))
                        .map(|s| s.as_str())
                        .unwrap_or("");

                    render_cell(&mut html, cell);
                }

                _ => {
                    render_cell(&mut html, "");
                }
            }
        }

        html.push_str("</tr>");
    }

    // End table
    html.push_str("</tbody></table>");

    // Footer
    html.push_str(&format!("<div class=\"footer\">{}</div>", footer_html));

    // End HTML
    html.push_str("</body></html>");

    // Write file
    std::fs::write(output_path, html)?;

    Ok(())
}