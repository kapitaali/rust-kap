//! CSV reader (§csv cluster): port of Kotlin `csv-reader.kt` (213 lines) as used
//! by `io:readCsv` (builtins/io_functions.kt:393-492).
//!
//! Faithfulness notes:
//! - `nextLine` splits on `\n` only; a trailing final newline yields no extra
//!   row (EOF with empty buffer returns null, io.kt:169). `\r` is content.
//! - Quoted-field `""` escape checks for `"` literally (csv-reader.kt:122),
//!   even with a custom `quoteChar`; backslash escapes any next char.
//! - Unquoted fields: leading whitespace skipped iff `trim`; trailing held
//!   whitespace dropped iff `trim`. Quoted content is never trimmed.
//! - Short rows read as `""`; missing cells convert through the same converter.
//! - With `parseNumbers`, `CsvHeuristicConverter` = `parseStringToNumber`,
//!   else every cell stays a string (`emptyConverter`).

use crate::number::KapNumber;

#[derive(Clone, Debug)]
pub struct CsvFlags {
    pub separator: char,
    pub quote: char,
    pub trim: bool,
    pub parse_numbers: bool,
    pub col_headers: bool,
    pub row_headers: bool,
}

impl Default for CsvFlags {
    fn default() -> Self {
        CsvFlags {
            separator: ',',
            quote: '"',
            trim: true,
            parse_numbers: false,
            col_headers: false,
            row_headers: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CsvCell {
    Str(String),
    Num(KapNumber),
}

pub struct CsvTable {
    pub nrows: usize,
    pub ncols: usize,
    pub cells: Vec<CsvCell>,
    pub row_labels: Option<Vec<String>>,
    pub col_labels: Option<Vec<String>>,
}

/// Split text into lines the way `CharacterProvider.nextLine` does.
fn text_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // EOF with an empty buffer returns null: drop one trailing empty piece.
    if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

fn is_csv_whitespace(ch: char, sep: char) -> bool {
    ch != sep && ch.is_whitespace()
}

/// Port of `CsvReader.readRows`: returns raw string fields per row.
pub fn read_rows(text: &str, flags: &CsvFlags) -> Result<Vec<Vec<String>>, String> {
    let lines = text_lines(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut li = 0usize;
    while li < lines.len() {
        let mut line = lines[li];
        li += 1;
        let mut fields: Vec<String> = Vec::new();
        let mut pos = 0usize;
        let chars: Vec<char> = line.chars().collect();
        // NOTE: `line` is rebound on multi-line quoted fields; work with a
        // refreshed char vec each time via a small closure-free loop below.
        let mut cur: Vec<char> = chars;
        loop {
            if flags.trim {
                while pos < cur.len() && is_csv_whitespace(cur[pos], flags.separator) {
                    pos += 1;
                }
            }
            if pos >= cur.len() {
                break;
            }
            let ch = cur[pos];
            pos += 1;
            let field = if ch == flags.quote {
                // readQuotedField (may consume further lines on embedded \n).
                let mut buf = String::new();
                loop {
                    while pos >= cur.len() {
                        if li >= lines.len() {
                            return Err("End of file in the middle of string".to_string());
                        }
                        line = lines[li];
                        li += 1;
                        pos = 0;
                        buf.push('\n');
                        cur = line.chars().collect();
                    }
                    let c = cur[pos];
                    pos += 1;
                    if c == flags.quote {
                        if pos >= cur.len() || cur[pos] != '"' {
                            break;
                        } else {
                            buf.push('"');
                            pos += 1;
                        }
                    } else if c == '\\' {
                        if pos >= cur.len() {
                            return Err("Unterminated string".to_string());
                        }
                        buf.push(cur[pos]);
                        pos += 1;
                    } else {
                        buf.push(c);
                    }
                }
                buf
            } else if ch == flags.separator {
                pos -= 1;
                String::new()
            } else {
                // readUnquotedField(initial).
                let mut buf = String::new();
                buf.push(ch);
                let mut held = String::new();
                while pos < cur.len() {
                    let c = cur[pos];
                    if c == flags.separator {
                        if !flags.trim {
                            buf.push_str(&held);
                        }
                        break;
                    } else if is_csv_whitespace(c, flags.separator) {
                        held.push(c);
                    } else {
                        buf.push_str(&held);
                        held.clear();
                        buf.push(c);
                    }
                    pos += 1;
                }
                buf
            };
            fields.push(field);
            if flags.trim {
                while pos < cur.len() && is_csv_whitespace(cur[pos], flags.separator) {
                    pos += 1;
                }
            }
            if pos < cur.len() {
                let c2 = cur[pos];
                pos += 1;
                if c2 != flags.separator {
                    return Err("Syntax error in CSV file".to_string());
                }
            }
        }
        rows.push(fields);
    }
    Ok(rows)
}

fn convert(s: &str, flags: &CsvFlags) -> CsvCell {
    if flags.parse_numbers {
        if let Some(n) = KapNumber::parse_kap_number_string(s) {
            return CsvCell::Num(n);
        }
    }
    CsvCell::Str(s.to_string())
}

/// Port of `CsvReader.read` (csv-reader.kt:25-86).
pub fn build_table(rows: &[Vec<String>], flags: &CsvFlags) -> CsvTable {
    if rows.is_empty() {
        return CsvTable { nrows: 0, ncols: 0, cells: Vec::new(), row_labels: None, col_labels: None };
    }
    let row_off = if flags.col_headers { 1 } else { 0 };
    let col_off = if flags.row_headers { 1 } else { 0 };
    let nrows = rows.len() - row_off;
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if width == 0 {
        return CsvTable { nrows, ncols: 0, cells: Vec::new(), row_labels: None, col_labels: None };
    }
    let ncols = width - col_off;
    let mut cells = Vec::with_capacity(nrows * ncols);
    for ri in 0..nrows {
        let row = &rows[ri + row_off];
        for ci in 0..ncols {
            let s = if ci + col_off >= row.len() { "" } else { &row[ci + col_off] };
            cells.push(convert(s, flags));
        }
    }
    let col_labels = if flags.col_headers {
        let row = &rows[0];
        let mut out = Vec::with_capacity(ncols);
        for i in 0..ncols {
            out.push(if i + col_off < row.len() { row[i + col_off].clone() } else { String::new() });
        }
        Some(out)
    } else {
        None
    };
    let row_labels = if flags.row_headers {
        let mut out = Vec::with_capacity(nrows);
        for i in 0..nrows {
            let row = &rows[i + row_off];
            out.push(if row.is_empty() { String::new() } else { row[0].clone() });
        }
        Some(out)
    } else {
        None
    };
    CsvTable { nrows, ncols, cells, row_labels, col_labels }
}
