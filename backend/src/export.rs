//! Exports: a query result as CSV, a dashboard as a zip of them.

use std::io::Write;

use chrono::{Datelike, Timelike};

use serde_json::Value;

/// A query result (`columns` with labels, `rows`) as CSV, for spreadsheets: the column labels on
/// the first line, RFC 4180 quoting, and a byte-order mark so Excel reads it as UTF-8.
pub fn csv(result: &Value) -> String {
    let mut out = String::from("\u{feff}");
    let empty = vec![];
    let columns = result["columns"].as_array().unwrap_or(&empty);
    let labels: Vec<String> =
        columns.iter().map(|c| cell(&Value::String(c["label"].as_str().unwrap_or("").into()))).collect();
    out.push_str(&labels.join(","));
    out.push_str("\r\n");
    for row in result["rows"].as_array().unwrap_or(&empty) {
        let cells: Vec<String> = row.as_array().unwrap_or(&empty).iter().map(cell).collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

fn cell(v: &Value) -> String {
    let text = match v {
        Value::Null => return String::new(),
        Value::Number(n) => return n.to_string(),
        Value::Bool(b) => return b.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // a spreadsheet runs text starting with these as a formula: keep it text
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{text}") } else { text };
    if text.contains([',', '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text }
}

/// A title as a file name: lower case letters, digits and underscores, at most 60 characters.
pub fn file_name(title: &str, fallback: &str) -> String {
    let mut name = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('_') {
            name.push('_');
        }
    }
    let name: String = name.trim_end_matches('_').chars().take(60).collect();
    if name.is_empty() { fallback.to_string() } else { name }
}

/// Files (path, text) in a zip archive.
pub fn zip(files: &[(String, String)]) -> zip::result::ZipResult<Vec<u8>> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let now = chrono::Utc::now().naive_utc();
    let (date, time) = (now.date(), now.time());
    let stamp = zip::DateTime::from_date_and_time(
        date.year() as u16,
        date.month() as u8,
        date.day() as u8,
        time.hour() as u8,
        time.minute() as u8,
        time.second() as u8,
    )
    .unwrap_or_default();
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(stamp);
    for (path, text) in files {
        w.start_file(path.as_str(), options)?;
        w.write_all(text.as_bytes())?;
    }
    Ok(w.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn result() -> Value {
        json!({
            "columns": [{"label": "Region"}, {"label": "Revenue, net"}, {"label": "Note"}],
            "rows": [["EMEA", 1200.5, "says \"hi\""], ["=SUM(A1)", null, "-bad"], ["AMER", -3, "line\nbreak"]],
        })
    }

    #[test]
    fn csv_has_labels_quotes_and_no_formulas() {
        let text = csv(&result());
        assert!(text.starts_with('\u{feff}'), "Excel reads UTF-8 with the mark");
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert_eq!(lines[0], "Region,\"Revenue, net\",Note");
        assert_eq!(lines[1], "EMEA,1200.5,\"says \"\"hi\"\"\"");
        // text that a spreadsheet would run as a formula is kept as text
        assert_eq!(lines[2], "'=SUM(A1),,'-bad");
        assert_eq!(lines[3], "AMER,-3,\"line\nbreak\"");
    }

    #[test]
    fn names_are_safe_file_names() {
        assert_eq!(file_name("Daily revenue (EUR) / store", "x"), "daily_revenue_eur_store");
        assert_eq!(file_name("???", "fallback"), "fallback");
        assert_eq!(file_name(&"a".repeat(100), "x").len(), 60);
    }

    #[test]
    fn zips_hold_the_files() {
        let bytes = zip(&[("a.csv".into(), "x,y\r\n".into()), ("Detail/b.csv".into(), "z\r\n".into())]).unwrap();
        let mut archive = ::zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        let year = archive.by_index(0).unwrap().last_modified().unwrap().year();
        assert!(year >= 2026, "dated now, not 1980: {year}");
        let mut b = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("Detail/b.csv").unwrap(), &mut b).unwrap();
        assert_eq!(b, "z\r\n");
    }
}
