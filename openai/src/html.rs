use indexmap::IndexMap;
use std::fs::write;
use std::error::Error;

pub fn generate_html_table(
    data: &IndexMap<String, Vec<String>>,
    output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let mut html = String::from(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>IndexMap Table Display</title>
    <style>
        table {
            width: 100%;
            border-collapse: collapse;
            margin: 20px 0;
        }
        th, td {
            border: 1px solid #ddd;
            padding: 8px;
            text-align: left;
        }
        th {
            background-color: #f2f2f2;
        }
        .red {
            color: red;
        }
    </style>
</head>
<body>
    <h1>IndexMap Table</h1>
    <table>
        <thead>
            <tr>
                <th>Domain</th>
"#
    );

    // Determine the maximum number of columns for the header
    let max_columns = data.values().map(|v| v.len()).max().unwrap_or(0);
    html.push_str(&format!("                <th>Expected Category</th>\n"));

    for i in 0..(max_columns - 1) {
        html.push_str(&format!("                <th>Category LLM {}</th>\n", i + 1));
    }

    html.push_str(
        r#"
            </tr>
        </thead>
        <tbody>
"#
    );

    for (row, columns) in data {
        html.push_str(&format!("            <tr>\n                <td>{}</td>\n", row));
        for column in columns {
            let processed_column = if column.contains("*RED*") {
                format!(
                    "<span class='red'>{}</span>",
                    column.replace("*RED*", "")
                )
            } else {
                column.clone()
            };
            html.push_str(&format!("                <td>{}</td>\n", processed_column));
        }
        // Fill empty cells if the current row has fewer columns than max_columns
        for _ in columns.len()..max_columns {
            html.push_str("                <td></td>\n");
        }
        html.push_str("            </tr>\n");
    }

    html.push_str(
        r#"
        </tbody>
    </table>
</body>
</html>
"#
    );

    write(output_path, html)?;
    Ok(())
}
