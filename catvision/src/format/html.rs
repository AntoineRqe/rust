use indexmap::IndexMap;
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use crate::my_traits::Infos;
use crate::CatVisionData;
use itertools::Itertools;

/// HTML output generator.
#[derive(Debug)]
pub struct HTMLGenerator {
    /// Output file path.
    pub filename: PathBuf,
    /// Mapping of column names to their indices.
    pub columns: HashMap<String, usize>,
}

impl crate::my_traits::Output for HTMLGenerator {
    /// Clone the output object.
    fn clone_box(&self) -> Box<dyn crate::my_traits::Output> {
        Box::new(Self {
            filename: self.filename.clone(),
            columns: self.columns.clone(),
        })
    }

    /// Generate HTML output from structured data.
    ///
    /// # Arguments
    ///
    /// * `data` - Structured data as `IndexMap<String, CatVisionData>`.
    /// * `infos` - Metadata including header, footer, and title.
    ///
    /// # Errors
    ///
    /// Returns an error if data cannot be downcast, or HTML generation/writing fails.
    fn write(
        &mut self,
        data: &dyn std::any::Any,
        infos: &Infos,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let data = data
            .downcast_ref::<IndexMap<String, CatVisionData>>()
            .ok_or("Failed to downcast data to IndexMap<String, CatVisionData>")?;

        generate_html_table(self.columns.clone(), data, infos, &self.filename)?;
        println!("HTML output written to {}", self.filename.display());
        Ok(())
    }

    /// Create a new `HTMLGenerator` instance.
    ///
    /// # Arguments
    ///
    /// * `filename` - Output file path.
    ///
    /// # Errors
    ///
    /// Returns an error if parent directories cannot be created.
    fn new(filename: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = filename.parent() {
            std::fs::create_dir_all(parent)?;
        }

        Ok(HTMLGenerator {
            filename: filename.to_path_buf(),
            columns: HashMap::new(),
        })
    }

    /// Create output header mapping for HTML table, including optional LLM category columns.
    ///
    /// # Arguments
    ///
    /// * `input_headers` - Existing input header mapping.
    /// * `levels_count` - Number of LLM category levels to include.
    fn create_output_header(
        &mut self,
        input_headers: &HashMap<String, usize>,
        levels_count: usize,
    ) {
        let mut headers = input_headers.clone();
        let mut offset = headers.len();

        // Add LLM category columns
        for i in 0..levels_count {
            let key = format!("llm_category_{}", i + 1);
            headers.insert(key, offset + i);
        }

        offset += levels_count;
        // Add prioritized category column
        headers.insert("prioritized_category".to_string(), offset);
        self.columns = headers.clone();
    }
}

/// Render a single table cell in HTML.
///
/// Cells containing `*RED*` are rendered in red.
///
/// # Arguments
///
/// * `html` - HTML string buffer to append to.
/// * `value` - Cell content.
fn render_cell(html: &mut String, value: &str) {
    if value.contains("*RED*") {
        let clean = value.replace("*RED*", "");
        html.push_str(&format!("<td><span class='red'>{}</span></td>", clean));
    } else {
        html.push_str(&format!("<td>{}</td>", value));
    }
}

/// Generate an HTML table from structured data.
///
/// # Arguments
///
/// * `columns` - Mapping of column names to indices.
/// * `data` - Data to render as an HTML table.
/// * `infos` - Metadata including title, header, and footer.
/// * `output_path` - Path to the output HTML file.
///
/// # Errors
///
/// Returns an error if data is empty or file writing fails.
pub fn generate_html_table(
    columns: HashMap<String, usize>,
    data: &IndexMap<String, CatVisionData>,
    infos: &Infos,
    output_path: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    if data.is_empty() {
        return Err("Data is empty".into());
    }

    // Escape header/footer HTML
    let header_html = infos.header.replace('\n', "<br>").replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;");
    let footer_html = infos.footer.replace('\n', "<br>").replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;");

    // Start HTML document
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
    html.push_str(&format!("<div class=\"header\">{}</div>", header_html));

    // Table start
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
                "appsite_name_by_olfeo" => render_cell(&mut html, categories.appsite_name_by_olfeo.as_deref().unwrap_or("")),
                "appsite_name_by_gemini" => render_cell(&mut html, categories.appsite_name_by_gemini.as_deref().unwrap_or("")),
                "categories_manual" => render_cell(&mut html, categories.categories_manual.as_deref().unwrap_or("")),
                "category_by_olfeo" => render_cell(&mut html, categories.category_olfeo.as_deref().unwrap_or("")),
                other if other.starts_with("llm_category_") => {
                    let level = other.trim_start_matches("llm_category_").parse::<usize>().unwrap_or(0);
                    let cell = categories
                        .categories_llm
                        .as_ref()
                        .and_then(|v| v.get(level - 1))
                        .map(|s| *s)
                        .unwrap_or("");
                    render_cell(&mut html, cell);
                }
                _ => render_cell(&mut html, ""),
            }
        }
        html.push_str("</tr>");
    }

    // End table and footer
    html.push_str("</tbody></table>");
    html.push_str(&format!("<div class=\"footer\">{}</div>", footer_html));
    html.push_str("</body></html>");

    // Write to file
    std::fs::write(output_path, html)?;

    Ok(())
}
