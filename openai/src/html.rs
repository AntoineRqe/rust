use indexmap::IndexMap;
use std::fs::write;
use std::error::Error;
use std::path::PathBuf;
use crate::my_traits::Infos;

#[derive(Debug)]
pub struct HTMLGenerator{
    filename: PathBuf,
}

impl crate::my_traits::Output for HTMLGenerator {

    fn write(&mut self, data: &dyn std::any::Any, infos: &Infos) -> Result<(), Box<dyn std::error::Error>> {
        let data = data.downcast_ref::<IndexMap<String, Vec<String>>>().ok_or("Failed to downcast data to IndexMap<String, Vec<String>>")?;
        generate_html_table(data, infos, &self.filename)?;
        println!("HTML output written to {}", self.filename.display());
        Ok(())
    }

    fn new(filename: &PathBuf) -> Self {
        HTMLGenerator {
            filename: filename.to_path_buf(),
        }
    }
}

pub fn generate_html_table(
    data: &IndexMap<String, Vec<String>>,
    infos: &Infos,
    output_path: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    if data.is_empty() {
        return Err("Data is empty".into());
    }

    let max_columns = data.values().map(|v| v.len()).max().unwrap_or(0);

    // Remplace les caractères spéciaux pour HTML
    let header_html = infos.header.replace('\n', "<br>").replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;");
    let footer_html = infos.footer.replace('\n', "<br>").replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;");

    // Début du HTML avec le titre de la page
    let mut html = String::from(
        r#"<!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            "#,
    );
    html.push_str(&format!("<title>{}</title>\n", infos.title));
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
            <h1>"#,
    );
    html.push_str(&infos.title);
    html.push_str("</h1>\n");

    // Ajoute l'en-tête
    html.push_str(&format!("<div class=\"header\">{}</div>\n", header_html));

    // Début du tableau
    html.push_str("<table><thead><tr><th>Domain</th><th>Expected Category</th>");

    // Ajoute les colonnes LLM
    for i in 1..max_columns - 1 {
        html.push_str(&format!("<th>Category LLM {}</th>", i));
    }

    html.push_str(&format!("<th>Prioritized Category</th>"));

    html.push_str("</tr></thead><tbody>");

    // Ajoute les lignes de données
    for (row, columns) in data {
        html.push_str(&format!("<tr><td>{}</td>", row));
        for column in columns {
            let cell = if column.contains("*RED*") {
                format!("<td><span class='red'>{}</span></td>", column.replace("*RED*", ""))
            } else {
                format!("<td>{}</td>", column)
            };
            html.push_str(&cell);
        }
        // Remplit les colonnes manquantes
        for _ in columns.len()..max_columns {
            html.push_str("<td></td>");
        }
        html.push_str("</tr>");
    }

    // Fin du tableau
    html.push_str("</tbody></table>");

    // Ajoute le pied de page
    html.push_str(&format!("<div class=\"footer\">{}</div>\n", footer_html));

    // Fin du HTML
    html.push_str("</body></html>");

    // Écrit le fichier
    write(output_path, html)?;
    Ok(())
}