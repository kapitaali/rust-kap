//! Minimal HTML table extractor for `io:fromHtmlTable`.
//!
//! Faithful port of the ksoup-driven logic in
//! `array/.../htmlconverter/htmlconverter.kt` (`parseAsHtmlTable`,
//! `parseHeaders`, `htmlTableToArray`) plus the value grammar
//! `NumberWithThousandsSeparator`. Only the `<table>`/`<thead>`/`<tbody>`/
//! `<tfoot>`/`<tr>`/`<td>`/`<th>` skeleton plus text is interpreted; everything
//! else is skipped. Tree-construction replicates the HTML5 table rules ksoup
//! applies to these inputs (verified against oracle output):
//! - `<tr>` directly under `<table>` (no open section) opens an implied `<tbody>`.
//! - `<td>`/`<th>` with an open section but no open `<tr>` opens an implied `<tr>`
//!   (this is what makes the bare-`<th>` thead of `simpleHtmlMonadicCall`
//!   yield column labels `Foo`/`Bar`).
//! - Unclosed elements are auto-closed at end of input (the `tableWithHeaders`
//!   fixture closes its second table with `</thead>`).
//! - `getElementsByTag` is a recursive descendant search, so nested tables'
//!   sections count (matching ksoup); `body.select("table")` collects every
//!   `<table>` in document order and the dyadic left argument picks the nth.
//! - Cell/header text is whitespace-normalised and trimmed; a small set of
//!   character entities is decoded.

/// One parsed table: rows of cell texts plus optional column-header texts.
pub struct HtmlTable {
    pub rows: Vec<Vec<String>>,
    pub headers: Option<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Table,
    Section,
    Row,
    Cell,
}

struct Frame {
    ctx: Ctx,
    tag: String,
    text: String,
    children: Vec<Frame>,
}

impl Frame {
    fn new(ctx: Ctx, tag: String) -> Self {
        Frame { ctx, tag, text: String::new(), children: Vec::new() }
    }

    /// Recursive descendant search by tag name (`getElementsByTag`).
    fn descendants_by_tag<'a>(&'a self, tag: &str, out: &mut Vec<&'a Frame>) {
        for c in &self.children {
            if c.tag == tag {
                out.push(c);
            }
            c.descendants_by_tag(tag, out);
        }
    }

    /// Whitespace-normalised text content (`Element.text()` approximation).
    fn text_content(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.text.trim().is_empty() {
            parts.push(collapse_ws(&self.text));
        }
        for c in &self.children {
            let t = c.text_content();
            if !t.is_empty() {
                parts.push(t);
            }
        }
        parts.join(" ")
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(semi) = s[i..].find(';').map(|k| k + i) {
                if semi - i <= 10 {
                    let ent = &s[i + 1..semi];
                    let rep = match ent {
                        "amp" => Some("&".to_string()),
                        "lt" => Some("<".to_string()),
                        "gt" => Some(">".to_string()),
                        "quot" => Some("\"".to_string()),
                        "apos" => Some("'".to_string()),
                        "nbsp" => Some(" ".to_string()),
                        _ if ent.starts_with('#') => {
                            let num = &ent[1..];
                            let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                                u32::from_str_radix(hex, 16).ok()
                            } else {
                                num.parse::<u32>().ok()
                            };
                            code.and_then(char::from_u32).map(|c| c.to_string())
                        }
                        _ => None,
                    };
                    if let Some(rep) = rep {
                        out.push_str(&rep);
                        i = semi + 1;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Parse every `<table>` in the document, in document order.
pub fn parse_tables(html: &str) -> Vec<HtmlTable> {
    // Skeleton tree: root > tables > sections > rows > cells. We keep a
    // stack of open frames with the root at the bottom.
    let mut stack: Vec<Frame> = vec![Frame::new(Ctx::Table, "root".to_string())];
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Comment?
            if html[i..].starts_with("<!--") {
                if let Some(end) = html[i..].find("-->") {
                    i += end + 3;
                } else {
                    break;
                }
                continue;
            }
            if let Some(end) = html[i..].find('>') {
                let raw = html[i + 1..i + end].trim();
                i += end + 1;
                let closing = raw.starts_with('/');
                let name = raw
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/')
                    .to_ascii_lowercase();
                if name.is_empty() {
                    continue;
                }
                if closing {
                    close_tag(&mut stack, &name);
                } else {
                    open_tag(&mut stack, &name);
                }
                continue;
            } else {
                break;
            }
        }
        // Text node: accumulate into innermost open cell, else drop (foster
        // parenting makes stray table text invisible to the extractor).
        if let Some(top) = stack.last() {
            if top.ctx == Ctx::Cell {
                let start = i;
                while i < bytes.len() && bytes[i] != b'<' {
                    i += 1;
                }
                let chunk = decode_entities(&html[start..i]);
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&chunk);
                }
                continue;
            }
        }
        i += 1;
    }
    while stack.len() > 1 {
        let f = stack.pop().unwrap();
        if let Some(p) = stack.last_mut() {
            p.children.push(f);
        }
    }
    let root = stack.pop().unwrap();

    // Collect tables in document order.
    let mut tables: Vec<&Frame> = Vec::new();
    collect_tables(&root, &mut tables);
    tables.into_iter().map(frame_to_table).collect()
}

fn open_tag(stack: &mut Vec<Frame>, name: &str) {
    match name {
        "table" => {
            // A table nested inside a cell belongs to that cell's subtree in
            // ksoup; for extraction purposes the flat skeleton keeps nested
            // tables as siblings (select("table") is document order either
            // way for non-nested fixtures).
            stack.push(Frame::new(Ctx::Table, "table".to_string()));
        }
        "thead" | "tbody" | "tfoot" => {
            // Implied-close an open section of another kind (mis-nested input).
            while stack.last().map(|f| f.ctx) == Some(Ctx::Section) {
                let f = stack.pop().unwrap();
                if let Some(p) = stack.last_mut() {
                    p.children.push(f);
                }
            }
            stack.push(Frame::new(Ctx::Section, name.to_string()));
        }
        "tr" => {
            // Implied tbody when a row appears directly under the table, and
            // implied-close of a previous row.
            close_open_row(stack);
            if stack.last().map(|f| f.ctx) == Some(Ctx::Table) {
                stack.push(Frame::new(Ctx::Section, "tbody".to_string()));
            }
            if stack.last().map(|f| f.ctx) == Some(Ctx::Section) {
                stack.push(Frame::new(Ctx::Row, "tr".to_string()));
            }
        }
        "td" | "th" => {
            close_open_cell(stack);
            if stack.last().map(|f| f.ctx) == Some(Ctx::Table) {
                stack.push(Frame::new(Ctx::Section, "tbody".to_string()));
            }
            if stack.last().map(|f| f.ctx) == Some(Ctx::Section) {
                // Implied tr (bare cells directly in a section).
                stack.push(Frame::new(Ctx::Row, "tr".to_string()));
            }
            if stack.last().map(|f| f.ctx) == Some(Ctx::Row) {
                stack.push(Frame::new(Ctx::Cell, name.to_string()));
            }
        }
        _ => {}
    }
}

/// Close an open cell/row when a new sibling cell/row starts.
fn close_open_cell(stack: &mut Vec<Frame>) {
    if stack.last().map(|f| f.ctx) == Some(Ctx::Cell) {
        let f = stack.pop().unwrap();
        if let Some(p) = stack.last_mut() {
            p.children.push(f);
        }
    }
}

fn close_open_row(stack: &mut Vec<Frame>) {
    close_open_cell(stack);
    if stack.last().map(|f| f.ctx) == Some(Ctx::Row) {
        let f = stack.pop().unwrap();
        if let Some(p) = stack.last_mut() {
            p.children.push(f);
        }
    }
}

fn close_tag(stack: &mut Vec<Frame>, name: &str) {
    let target = match name {
        "table" => Ctx::Table,
        "thead" | "tbody" | "tfoot" => Ctx::Section,
        "tr" => Ctx::Row,
        "td" | "th" => Ctx::Cell,
        _ => return,
    };
    // Pop until the matching frame is closed (auto-closing anything open
    // inside it); never pop the root.
    while stack.len() > 1 {
        let is_match = stack.last().map(|f| f.ctx == target) == Some(true);
        let f = stack.pop().unwrap();
        if let Some(p) = stack.last_mut() {
            p.children.push(f);
        }
        if is_match {
            break;
        }
        // A mismatched close (e.g. </thead> for an open table) keeps popping;
        // stop if the new top cannot contain the target.
        if stack.len() == 1 {
            break;
        }
    }
}

fn collect_tables<'a>(frame: &'a Frame, out: &mut Vec<&'a Frame>) {
    for c in &frame.children {
        if c.tag == "table" {
            out.push(c);
        }
        collect_tables(c, out);
    }
}

/// `parseAsHtmlTable` (htmlconverter.kt:9-34): exactly one `<tbody>`
/// descendant, rows of `<td>`/`<th>` texts, headers iff exactly one `<thead>`
/// whose first child element's cells match the first row's width.
fn frame_to_table(table: &Frame) -> HtmlTable {
    let mut tbodies: Vec<&Frame> = Vec::new();
    table.descendants_by_tag("tbody", &mut tbodies);
    let mut rows: Vec<Vec<String>> = Vec::new();
    if tbodies.len() == 1 {
        for child in &tbodies[0].children {
            if child.tag == "tr" {
                let mut row: Vec<String> = Vec::new();
                for cell in &child.children {
                    if cell.tag == "td" || cell.tag == "th" {
                        row.push(cell.text_content());
                    }
                }
                rows.push(row);
            }
        }
    }
    let mut theads: Vec<&Frame> = Vec::new();
    table.descendants_by_tag("thead", &mut theads);
    let mut headers: Option<Vec<String>> = None;
    if theads.len() == 1 && !rows.is_empty() {
        if let Some(first) = theads[0].children.first() {
            let mut h: Vec<String> = Vec::new();
            for cell in &first.children {
                if cell.tag == "td" || cell.tag == "th" {
                    h.push(cell.text_content());
                }
            }
            if h.len() == rows[0].len() {
                headers = Some(h);
            }
        }
    }
    HtmlTable { rows, headers }
}

/// Pick the nth table (`htmlTableToArray` null-cases included): bad index,
/// missing/ambiguous tbody, empty content or zero columns all yield `None`,
/// which the caller maps to `No table found in HTML content`.
pub fn nth_table(html: &str, n: i64) -> Option<HtmlTable> {
    if n < 0 {
        return None;
    }
    let tables = parse_tables(html);
    let t = tables.get(n as usize)?;
    if t.rows.is_empty() {
        return None;
    }
    if t.rows.iter().map(|r| r.len()).max().unwrap_or(0) == 0 {
        return None;
    }
    Some(HtmlTable { rows: t.rows.clone(), headers: t.headers.clone() })
}

/// `NumberWithThousandsSeparator` (htmlconverter.kt:91-105) applied to a
/// trimmed cell text. Returns the Kap number source text: integer digits
/// (arbitrarily large; the caller reduces via `parse_kap_number_string`) or
/// a decimal `intpart.fracpart` for f64 parsing. Trailing junk after the
/// number is allowed (`.*$`), so `123foo` parses as `123`.
pub fn parse_cell_number(trimmed: &str) -> Option<CellNumber> {
    let s = trimmed.as_bytes();
    let mut i = 0;
    let neg = if s.first() == Some(&b'-') {
        i = 1;
        true
    } else {
        false
    };
    // ((?:[0-9]+,)*[0-9]+): digit runs joined by structural commas (each
    // comma must be followed by a digit); anything else terminates the
    // integer part and is swallowed by the `.*$` tail.
    let mut digits = String::new();
    let mut any = false;
    while i < s.len() {
        match s[i] {
            b'0'..=b'9' => {
                digits.push(s[i] as char);
                any = true;
                i += 1;
            }
            b',' if any && i + 1 < s.len() && s[i + 1].is_ascii_digit() => {
                i += 1;
            }
            _ => break,
        }
    }
    if !any {
        return None;
    }
    // (?:\.([0-9]+))?
    let mut frac: Option<String> = None;
    if s.get(i) == Some(&b'.') {
        let mut f = String::new();
        let mut j = i + 1;
        while j < s.len() && s[j].is_ascii_digit() {
            f.push(s[j] as char);
            j += 1;
        }
        if !f.is_empty() {
            frac = Some(f);
            i = j;
        }
    }
    let _ = &s[i..];
    let sign = if neg { "-" } else { "" };
    match frac {
        Some(f) => Some(CellNumber::Decimal(format!("{sign}{digits}.{f}"))),
        None => Some(CellNumber::Integer(format!("{sign}{digits}"))),
    }
}

pub enum CellNumber {
    Integer(String),
    Decimal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_numbers_match_kotlin_regex() {
        assert!(matches!(parse_cell_number("abctest"), None));
        assert!(matches!(parse_cell_number(""), None));
        assert!(matches!(parse_cell_number(".5"), None));
        match parse_cell_number("123foo").unwrap() {
            CellNumber::Integer(d) => assert_eq!(d, "123"),
            _ => panic!("expected int"),
        }
        match parse_cell_number("-1,000.25junk").unwrap() {
            CellNumber::Decimal(d) => assert_eq!(d, "-1000.25"),
            _ => panic!("expected dec"),
        }
        match parse_cell_number("12.25").unwrap() {
            CellNumber::Decimal(d) => assert_eq!(d, "12.25"),
            _ => panic!("expected dec"),
        }
    }

    #[test]
    fn implied_tbody_and_row_rules() {
        // Bare tr under table (2519 table 2) and bare th in thead (2518).
        let tables = parse_tables("<table><tr><td>10</td><td>11</td></tr></table>");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows, vec![vec!["10".to_string(), "11".to_string()]]);
        let tables = parse_tables("<table><thead><th>Foo</th><th>Bar</th></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>");
        assert_eq!(tables[0].headers, Some(vec!["Foo".to_string(), "Bar".to_string()]));
    }
}
