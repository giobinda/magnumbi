//! Spreadsheets and CSV files as data: an uploaded file (or a Google Sheet, downloaded as Excel)
//! becomes tables, each a CSV file DuckDB reads in the workspace's own folder (connectors/duckdb.rs).
//! A CSV is one table; an Excel or OpenDocument workbook is a table per sheet.

use std::io::Cursor;

use calamine::{Data, Reader};

/// One table of a file: its name (a model's `table`) and its rows as CSV, header first.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    pub csv: Vec<u8>,
}

const KINDS: &str = "Magnum BI reads .csv, .xlsx, .xls and .ods files";

/// The tables of a file called `name`. `max_unpacked`: how much a workbook may unpack to (a small
/// file can unpack to gigabytes: a zip bomb).
pub fn read(name: &str, bytes: &[u8], max_unpacked: u64) -> Result<Vec<Table>, String> {
    let (stem, ext) = name.rsplit_once('.').map_or((name, ""), |(s, e)| (s, e));
    match ext.to_ascii_lowercase().as_str() {
        "csv" | "tsv" => Ok(vec![Table { name: table_name(stem), csv: bytes.to_vec() }]),
        "xlsx" | "xlsm" | "ods" => {
            unpacked_at_most(bytes, max_unpacked)?;
            workbook(bytes)
        }
        "xls" => workbook(bytes),
        _ => Err(format!("{KINDS}: {name:?} isn't one")),
    }
}

/// A Google Sheet's id, from its link (or the id itself): only docs.google.com/spreadsheets/d/<id>.
pub fn sheet_id(link: &str) -> Option<String> {
    let link = link.trim();
    let id = match link.split_once("docs.google.com/spreadsheets/d/") {
        Some(("" | "https://" | "http://", after)) => after.split(['/', '?', '#']).next()?,
        Some(_) => return None,
        None => link,
    };
    let ok = (25..=100).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then(|| id.to_string())
}

/// A zip-based workbook that unpacks to more than `max` is refused, before it's read.
fn unpacked_at_most(bytes: &[u8], max: u64) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| unreadable())?;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        total = total.saturating_add(zip.by_index_raw(i).map_err(|_| unreadable())?.size());
        if total > max {
            return Err(format!("this workbook is too big once unpacked (over {} MB)", max >> 20));
        }
    }
    Ok(())
}

fn unreadable() -> String {
    "this file can't be read as a spreadsheet: is it really one?".into()
}

/// Every sheet with data, as a table.
fn workbook(bytes: &[u8]) -> Result<Vec<Table>, String> {
    let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|_| unreadable())?;
    let mut tables: Vec<Table> = vec![];
    for sheet in book.sheet_names() {
        let range = book.worksheet_range(&sheet).map_err(|_| unreadable())?;
        if range.is_empty() {
            continue;
        }
        let mut csv = String::new();
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(|c| quoted(&cell(c))).collect();
            csv.push_str(&cells.join(","));
            csv.push('\n');
        }
        // unique once cleaned ("Q1" and "q1" are q1 and q1_2)
        let base = table_name(&sheet);
        let mut name = base.clone();
        for n in 2.. {
            if !tables.iter().any(|t| t.name == name) {
                break;
            }
            name = format!("{base}_{n}");
        }
        tables.push(Table { name, csv: csv.into_bytes() });
    }
    Ok(tables)
}

/// A cell as CSV text: dates as ISO dates (with the time when there is one), numbers as numbers.
fn cell(c: &Data) -> String {
    match c {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Int(n) => n.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) if d.is_datetime() => match d.as_datetime() {
            Some(t) if t.time() == chrono::NaiveTime::MIN => t.format("%Y-%m-%d").to_string(),
            Some(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
            None => d.as_f64().to_string(),
        },
        Data::DateTime(d) => d.as_f64().to_string(),
    }
}

/// Quoted when it holds a comma, a quote or a line break.
fn quoted(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// A table's name from a file's or sheet's: lowercase letters, digits and _, never starting
/// with a digit ("Sales 2026 (EU)" is sales_2026_eu).
fn table_name(s: &str) -> String {
    let mut out = String::new();
    for ch in s.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    match out.chars().next() {
        None => "sheet".into(),
        Some(c) if c.is_ascii_digit() => format!("t_{out}"),
        Some(_) => out,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A minimal .xlsx: `sheets` of rows (a cell starting with '=' is a number, 'd:' a date, the
    /// rest text), with the parts Excel and calamine expect.
    pub(crate) fn xlsx(sheets: &[(&str, &[&[&str]])]) -> Vec<u8> {
        let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
        let mut out = std::io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        let o = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, text: String| {
            z.start_file(name, o).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        };
        let n = sheets.len();
        let overrides: String = (1..=n)
            .map(|i| format!(r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#))
            .collect();
        put(
            "[Content_Types].xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>{overrides}</Types>"#
            ),
        );
        put("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into());
        let list: String = (1..=n)
            .map(|i| format!(r#"<sheet name="{}" sheetId="{i}" r:id="rId{i}"/>"#, esc(sheets[i - 1].0)))
            .collect();
        put(
            "xl/workbook.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{list}</sheets></workbook>"#
            ),
        );
        let rels: String = (1..=n)
            .map(|i| format!(r#"<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>"#))
            .collect();
        put(
            "xl/_rels/workbook.xml.rels",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}<Relationship Id="rS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#
            ),
        );
        // style 1: a date (built-in format 14)
        put("xl/styles.xml", r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font/></fonts><fills count="1"><fill/></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/></cellXfs></styleSheet>"#.into());
        for (i, (_, rows)) in sheets.iter().enumerate() {
            let body: String = rows
                .iter()
                .enumerate()
                .map(|(r, cells)| {
                    let cells: String = cells
                        .iter()
                        .enumerate()
                        .map(|(c, v)| {
                            let at = format!("{}{}", (b'A' + c as u8) as char, r + 1);
                            if let Some(n) = v.strip_prefix('=') {
                                format!(r#"<c r="{at}"><v>{n}</v></c>"#)
                            } else if let Some(d) = v.strip_prefix("d:") {
                                format!(r#"<c r="{at}" s="1"><v>{d}</v></c>"#)
                            } else {
                                format!(r#"<c r="{at}" t="inlineStr"><is><t>{}</t></is></c>"#, esc(v))
                            }
                        })
                        .collect();
                    format!(r#"<row r="{}">{cells}</row>"#, r + 1)
                })
                .collect();
            put(
                &format!("xl/worksheets/sheet{}.xml", i + 1),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{body}</sheetData></worksheet>"#
                ),
            );
        }
        z.finish().unwrap();
        out.into_inner()
    }

    fn csv_of(t: &Table) -> String {
        String::from_utf8(t.csv.clone()).unwrap()
    }

    #[test]
    fn a_google_sheet_is_found_from_its_link() {
        let id = "1AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abc";
        for link in [
            format!("https://docs.google.com/spreadsheets/d/{id}/edit#gid=0"),
            format!("https://docs.google.com/spreadsheets/d/{id}"),
            format!("docs.google.com/spreadsheets/d/{id}/edit?usp=sharing"),
            format!("  https://docs.google.com/spreadsheets/d/{id}/edit  "),
            id.to_string(),
        ] {
            assert_eq!(sheet_id(&link).as_deref(), Some(id), "{link}");
        }
        for not in [
            format!("https://evil.example/spreadsheets/d/{id}"),
            format!("https://docs.google.com.evil.example/spreadsheets/d/{id}"),
            format!("https://docs.google.com/document/d/{id}/edit"),
            "https://docs.google.com/spreadsheets/d/../../x/edit".to_string(),
            "short".to_string(),
            format!("{id} x"),
            String::new(),
        ] {
            assert_eq!(sheet_id(&not), None, "{not}");
        }
    }

    #[test]
    fn a_csv_is_one_table_named_after_its_file() {
        let t = read("Sales 2026 (EU).csv", b"region,amount\nEMEA,10\n", 1 << 20).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "sales_2026_eu");
        assert_eq!(csv_of(&t[0]), "region,amount\nEMEA,10\n");
    }

    #[test]
    fn a_workbook_is_a_table_per_sheet() {
        let book = xlsx(&[
            (
                "Orders",
                &[
                    &["order", "day", "amount", "note"],
                    &["A-1", "d:46000", "=12.5", "big, urgent"],
                    &["A-2", "d:46001", "=3", "say \"hi\""],
                ],
            ),
            ("Customers & more", &[&["name"], &["Acme"]]),
            ("Empty", &[]),
        ]);
        let t = read("book.xlsx", &book, 1 << 20).unwrap();
        // an empty sheet is no table
        assert_eq!(t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["orders", "customers_more"]);
        // dates as ISO dates, numbers as numbers, text quoted where needed
        assert_eq!(
            csv_of(&t[0]),
            "order,day,amount,note\nA-1,2025-12-09,12.5,\"big, urgent\"\nA-2,2025-12-10,3,\"say \"\"hi\"\"\"\n"
        );
        assert_eq!(csv_of(&t[1]), "name\nAcme\n");
    }

    #[test]
    fn sheets_with_the_same_name_once_cleaned_stay_apart() {
        let book = xlsx(&[("Q1", &[&["a"], &["1"]]), ("q1", &[&["b"], &["2"]]), ("2026", &[&["c"], &["3"]])]);
        let t = read("book.xlsx", &book, 1 << 20).unwrap();
        assert_eq!(t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["q1", "q1_2", "t_2026"]);
    }

    #[test]
    fn what_isnt_a_spreadsheet_is_refused() {
        for name in ["notes.txt", "photo.png", "noextension", "data.parquet"] {
            let e = read(name, b"x", 1 << 20).unwrap_err();
            assert!(e.contains(".csv") && e.contains(".xlsx"), "{name}: {e}");
        }
        // named .xlsx but not one
        let e = read("fake.xlsx", b"not a zip", 1 << 20).unwrap_err();
        assert!(e.contains("can't be read"), "{e}");
    }

    #[test]
    fn a_workbook_that_unpacks_to_too_much_is_refused() {
        // a small file that unpacks to 50 MB: a zip bomb, as far as we're concerned
        let mut out = std::io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        z.start_file("xl/worksheets/sheet1.xml", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(&vec![b' '; 50 << 20]).unwrap();
        z.finish().unwrap();
        let bytes = out.into_inner();
        assert!(bytes.len() < 1 << 20, "{}", bytes.len());
        let e = read("bomb.xlsx", &bytes, 10 << 20).unwrap_err();
        assert!(e.contains("too big"), "{e}");
    }
}
