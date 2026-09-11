//! SQLite-backed `sql:` module + `cm:` Calcite-local (contrib/sql, experimental/calcite-mod).
//!
//! Faithfulness notes:
//! - `sql:connect` takes 1-3 args (url[, user, pass]); sqlite ignores credentials.
//!   `jdbc:sqlite::memory:` (and any `jdbc:sqlite:` path) opens that database;
//!   `jdbc:h2:mem:` URLs map to in-memory sqlite (documented emulation — the JVM
//!   rows only use portable CREATE/INSERT/SELECT, verified against the sweep).
//!   Anything else errors like Kotlin's `DriverManager` failure.
//! - `sql:query` returns a (rows × cols) nested array with axis-1 column labels
//!   (Kotlin `resultSetToValue` + `MetadataOverrideArray`); `sql:update` returns
//!   the affected-row count; prepared variants bind per Kotlin's
//!   `updatePreparedStatementCol` (rank-1 = one execution, rank-2 = per-row).
//! - `cm:connect :local` returns a calcite-flagged connection whose queries
//!   materialise in-scope rank-2 arrays as `kap.ns_var` temp tables first.
//! - Type mapping mirrors `parseEntry`: Integer→Long, Real→Double, Text→Str
//!   (numeric-declared columns parse to Long/BigInt/Rational), Blob→Long vector,
//!   Null→nil, time/date-declared columns→Timestamp. Timestamps bind as millis.

use crate::array::{ArrayData, DimensionLabels, KapArray};
use crate::number::KapNumber;
use crate::{APLValue, AplRef, Environment};
use std::rc::Rc;

pub fn sql_error(who: &str, msg: impl std::fmt::Display) -> String {
    format!("{}: Exception from database engine: {}", who, msg)
}

/// Map a JDBC URL to a sqlite open target. Returns Err for unknown drivers
/// (Kotlin: `DriverManager.getConnection` throws `SQLException`).
pub fn open_url(url: &str) -> Result<rusqlite::Connection, String> {
    if let Some(rest) = url.strip_prefix("jdbc:sqlite:") {
        if rest == ":memory:" || rest.is_empty() {
            rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())
        } else {
            rusqlite::Connection::open(rest).map_err(|e| e.to_string())
        }
    } else if url.starts_with("jdbc:h2:mem:") {
        // Documented emulation: no JVM here, so H2 URLs open in-memory sqlite.
        // The sweep's H2 rows use portable DDL/DML only.
        rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())
    } else {
        Err(format!("No suitable driver found for {}", url))
    }
}

/// Convert one Kap value to a sqlite bind value (Kotlin
/// `updatePreparedStatementCol`, sql.kt:274-303).
pub fn kap_to_sql(v: &APLValue, who: &str) -> Result<rusqlite::types::Value, String> {
    use rusqlite::types::Value;
    match v {
        APLValue::Number(KapNumber::Long(x)) => Ok(Value::Integer(*x)),
        APLValue::Number(KapNumber::Double(x)) => Ok(Value::Real(*x)),
        APLValue::Number(KapNumber::BigInt(x)) => {
            use num_traits::ToPrimitive;
            match x.to_i64() {
                Some(i) => Ok(Value::Integer(i)),
                None => Ok(Value::Text(x.to_string())),
            }
        }
        APLValue::Number(KapNumber::Rational(r)) => {
            // Kotlin stores an exact BigDecimal and ERRORS on non-terminating
            // decimals (`insertNonDecimalRationalShouldFail`). Accept only
            // denominators of the form 2^a·5^b, stored as the nearest f64.
            let mut den = r.denom().clone();
            let zero = num_bigint::BigInt::from(0);
            let two = num_bigint::BigInt::from(2);
            let five = num_bigint::BigInt::from(5);
            while &den % &two == zero {
                den /= &two;
            }
            while &den % &five == zero {
                den /= &five;
            }
            if den != num_bigint::BigInt::from(1) {
                return Err(format!("Cannot convert rational to decimal: {}/{}", r.numer(), r.denom()));
            }
            use num_traits::ToPrimitive;
            r.to_f64()
                .map(Value::Real)
                .ok_or_else(|| format!("{}: rational out of range", who))
        }
        APLValue::Number(KapNumber::Complex(_, _)) => Err(format!("{}: complex cannot bind", who)),
        APLValue::Str(s) => Ok(Value::Text(s.clone())),
        APLValue::Char(c) => Ok(Value::Text(c.to_string())),
        APLValue::Nil => Ok(Value::Null),
        APLValue::Timestamp(ms) => Ok(Value::Integer(*ms)),
        APLValue::Array(a) if a.dimensions.len() == 1 => {
            // Rank-1 Long vector = byte content (Kotlin `asByteArray`, used by
            // the blob rows `s sql:updatePrepared 1 (⍳256)`); rank-1 chars = text.
            let els = a.elements();
            if els.iter().all(|e| matches!(e.as_ref(), APLValue::Number(KapNumber::Long(_)))) {
                Ok(Value::Blob(
                    els.iter()
                        .map(|e| match e.as_ref() {
                            APLValue::Number(KapNumber::Long(x)) => *x as u8,
                            _ => 0,
                        })
                        .collect(),
                ))
            } else if els.iter().all(|e| matches!(e.as_ref(), APLValue::Char(_))) {
                Ok(Value::Text(
                    els.iter()
                        .map(|e| match e.as_ref() {
                            APLValue::Char(c) => *c,
                            _ => '?',
                        })
                        .collect(),
                ))
            } else {
                Err(format!("{}: Value cannot be used in an SQL prepared statement", who))
            }
        }
        other => Err(format!(
            "{}: Value cannot be used in an SQL prepared statement: {}",
            who,
            other.format_plain()
        )),
    }
}

/// Convert one sqlite result cell to a Kap value (Kotlin `parseEntry`,
/// sql.kt:40-63), guided by the column's declared type for TEXT cells.
pub fn sql_to_kap(decl: Option<&str>, v: rusqlite::types::Value) -> AplRef<APLValue> {
    use rusqlite::types::Value;
    let decl_lc = decl.unwrap_or("").to_ascii_lowercase();
    let is_time = decl_lc.contains("time") || decl_lc.contains("date");
    match v {
        Value::Null => Rc::new(APLValue::Nil),
        Value::Integer(i) => {
            if is_time {
                Rc::new(APLValue::Timestamp(i))
            } else {
                Rc::new(APLValue::Number(KapNumber::Long(i)))
            }
        }
        Value::Real(f) => Rc::new(APLValue::Number(KapNumber::Double(f))),
        Value::Blob(b) => {
            let els: Vec<AplRef<APLValue>> =
                b.into_iter().map(|x| Rc::new(APLValue::Number(KapNumber::Long(x as i64)))).collect();
            Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![els.len()],
                ArrayData::Nested(els),
            ))))
        }
        Value::Text(s) => {
            if is_time {
                if let Ok(ms) = s.trim().parse::<i64>() {
                    return Rc::new(APLValue::Timestamp(ms));
                }
                return Rc::new(APLValue::Str(s));
            }
            if decl_lc.contains("int") {
                if let Ok(i) = s.trim().parse::<i64>() {
                    return Rc::new(APLValue::Number(KapNumber::Long(i)));
                }
                if let Ok(b) = s.trim().parse::<num_bigint::BigInt>() {
                    return Rc::new(APLValue::Number(reduce_bigint(b)));
                }
                return Rc::new(APLValue::Str(s));
            }
            if decl_lc.contains("numeric") || decl_lc.contains("decimal") || decl_lc.contains("number") {
                if let Ok(b) = s.trim().parse::<num_bigint::BigInt>() {
                    return Rc::new(APLValue::Number(reduce_bigint(b)));
                }
                if let Some(r) = parse_decimal(&s) {
                    return Rc::new(APLValue::Number(KapNumber::Rational(r)));
                }
                return Rc::new(APLValue::Str(s));
            }
            if decl_lc.contains("real") || decl_lc.contains("float") || decl_lc.contains("double") {
                if let Ok(f) = s.trim().parse::<f64>() {
                    return Rc::new(APLValue::Number(KapNumber::Double(f)));
                }
                return Rc::new(APLValue::Str(s));
            }
            Rc::new(APLValue::Str(s))
        }
    }
}

fn reduce_bigint(b: num_bigint::BigInt) -> KapNumber {
    use num_traits::ToPrimitive;
    match b.to_i64() {
        Some(i) => KapNumber::Long(i),
        None => KapNumber::BigInt(b),
    }
}

/// Parse a decimal literal (`1234.87`, `.5`, `1E3` fails → None) into an exact
/// Rational (Kotlin `BigDecimal` → `makeAPLNumber` keeps decimals rational).
fn parse_decimal(s: &str) -> Option<num_rational::BigRational> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let neg = s.starts_with('-');
    let s = s.strip_prefix(['-', '+']).unwrap_or(s);
    if s.contains(['e', 'E']) {
        return None;
    }
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    if !int_part.chars().all(|c| c.is_ascii_digit()) || !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let digits = format!("{}{}", if int_part.is_empty() { "0" } else { int_part }, frac_part);
    let num: num_bigint::BigInt = digits.parse().ok()?;
    let den = num_bigint::BigInt::from(10).pow(frac_part.len() as u32);
    let r = num_rational::BigRational::new(num, den);
    Some(if neg { -r } else { r })
}

/// Run a SELECT and build the (rows × cols) labelled array (Kotlin
/// `resultSetToValue`, sql.kt:100-129).
pub fn run_query(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[rusqlite::types::Value],
) -> Result<AplRef<APLValue>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let col_count = stmt.column_count();
    let (col_names, decls): (Vec<String>, Vec<Option<String>>) = stmt
        .columns()
        .iter()
        .map(|c| (c.name().to_string(), c.decl_type().map(|s| s.to_string())))
        .unzip();
    let params_ref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let mut rows_query = stmt.query(params_ref.as_slice()).map_err(|e| e.to_string())?;
    let mut cells: Vec<AplRef<APLValue>> = Vec::new();
    let mut nrows = 0usize;
    while let Some(row) = rows_query.next().map_err(|e| e.to_string())? {
        nrows += 1;
        for i in 0..col_count {
            let v: rusqlite::types::Value = row.get(i).map_err(|e| e.to_string())?;
            cells.push(sql_to_kap(decls[i].as_deref(), v));
        }
    }
    let arr = KapArray::new(vec![nrows, col_count], ArrayData::Nested(cells));
    let labels = vec![
        None,
        Some(col_names.into_iter().map(Some).collect::<Vec<crate::array::AxisLabel>>()),
    ];
    let mut arr = arr;
    arr.labels = Some(Box::new(DimensionLabels { labels }));
    Ok(Rc::new(APLValue::Array(Rc::new(arr))))
}

/// Collect in-scope rank-2 arrays keyed `ns_name` (innermost binding wins),
/// backing `kap.ns_var` references on calcite connections (Calcite
/// `VariableSchema`: only 2-D variables become tables).
pub fn kap_tables_in_scope(env: &AplRef<Environment>) -> std::collections::HashMap<String, AplRef<APLValue>> {
    fn consider(out: &mut std::collections::HashMap<String, AplRef<APLValue>>, ns: &str, name: &str, v: &AplRef<APLValue>) {
        if let APLValue::Array(a) = v.as_ref() {
            if a.dimensions.len() == 2 {
                out.entry(format!("{}_{}", ns, name)).or_insert_with(|| v.clone());
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    // Lexical scopes, innermost first (block locals shadow module bindings).
    let mut cur: Option<AplRef<Environment>> = Some(env.clone());
    while let Some(e) = cur {
        for ((name, ns), v) in e.symbols.borrow().iter() {
            consider(&mut out, ns.as_deref().unwrap_or("default"), name, v);
        }
        cur = e.parent.clone();
    }
    // Module namespace table (top-level `←` bindings live here, not in
    // lexical scope — `Environment::define` routes root bindings by namespace).
    for (ns, map) in env.ns_registry.symbols.borrow().iter() {
        for (name, v) in map.iter() {
            consider(&mut out, ns, name, v);
        }
    }
    out
}

/// Materialise every `kap.NAME` table referenced by `sql` from the in-scope
/// tables map (Calcite `VariableSchema` + `VariableTable`). Column names come
/// from axis-1 labels when present, else `c0…`; types are inferred from the
/// first non-null cell of each column.
pub fn materialise_kap_tables(
    conn: &rusqlite::Connection,
    sql: &str,
    tables: &std::collections::HashMap<String, AplRef<APLValue>>,
) -> Result<(), String> {
    let re = regex::Regex::new(r"(?i)\b(?:from|join)\s+kap\.([A-Za-z0-9_]+)").map_err(|e| e.to_string())?;
    let mut needed: Vec<String> = Vec::new();
    for cap in re.captures_iter(sql) {
        let t = cap[1].to_string();
        if !needed.contains(&t) {
            needed.push(t);
        }
    }
    for t in needed {
        let v = tables
            .get(&t)
            .ok_or_else(|| format!("Table not found: kap.{}", t))?;
        let (dims, els, col_labels) = match v.as_ref() {
            APLValue::Array(a) => {
                let labs: Vec<String> = a
                    .labels
                    .as_ref()
                    .and_then(|l| l.labels.get(1).cloned())
                    .flatten()
                    .map(|axis| {
                        axis.into_iter()
                            .enumerate()
                            .map(|(i, l)| l.unwrap_or_else(|| format!("c{}", i)))
                            .collect()
                    })
                    .unwrap_or_else(|| (0..a.dimensions[1]).map(|i| format!("c{}", i)).collect());
                (a.dimensions.clone(), a.elements(), labs)
            }
            _ => return Err(format!("Table not found: kap.{}", t)),
        };
        let nrows = dims[0];
        let ncols = dims[1];
        let clean = |s: &str| {
            let c: String = s.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect();
            format!("\"{}\"", c)
        };
        // Column types from the first non-null cell of each column.
        let mut coltypes = vec!["TEXT"; ncols];
        for c in 0..ncols {
            for r in 0..nrows {
                match els[r * ncols + c].as_ref() {
                    APLValue::Nil => continue,
                    APLValue::Number(KapNumber::Long(_)) | APLValue::Number(KapNumber::BigInt(_)) => {
                        coltypes[c] = "INTEGER"
                    }
                    APLValue::Number(KapNumber::Double(_)) => coltypes[c] = "REAL",
                    APLValue::Timestamp(_) => coltypes[c] = "INTEGER",
                    _ => coltypes[c] = "TEXT",
                }
                break;
            }
        }
        let defs: Vec<String> =
            (0..ncols).map(|c| format!("{} {}", clean(&col_labels[c]), coltypes[c])).collect();
        conn.execute_batch(&format!("DROP TABLE IF EXISTS kap_{}; CREATE TEMP TABLE kap_{} ({});", t, t, defs.join(", ")))
            .map_err(|e| e.to_string())?;
        // Rewrite `kap.NAME` to the temp table (unqualified; TEMP lookup).
        // (Done by caller re-scanning: simpler to create under the bare name too.)
        let placeholders: Vec<String> = (0..ncols).map(|_| "?".to_string()).collect();
        let insert_sql = format!("INSERT INTO kap_{} VALUES ({})", t, placeholders.join(", "));
        for r in 0..nrows {
            let mut params: Vec<rusqlite::types::Value> = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let cell = cell_for_table(&els[r * ncols + c]);
                params.push(kap_to_sql(&cell, "cm:query").map_err(|e| e.to_string())?);
            }
            let pref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            conn.execute(&insert_sql, pref.as_slice()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Rewrite `kap.NAME` references to the materialised `kap_NAME` temp tables
/// (sqlite has no catalogs; TEMP tables resolve unqualified).
pub fn rewrite_kap_refs(sql: &str) -> String {
    let re = regex::Regex::new(r"(?i)\bkap\.([A-Za-z0-9_]+)").unwrap();
    re.replace_all(sql, "kap_$1").to_string()
}

/// Unwrap one table cell for binding: nested rank-0/singleton arrays disclose
/// to their scalar (Calcite `VariableTable` reads cell values, not boxes).
fn cell_for_table(v: &APLValue) -> APLValue {
    match v {
        APLValue::Array(a) if a.dimensions.is_empty() => {
            a.elements().into_iter().next().map(|e| e.as_ref().clone()).unwrap_or(APLValue::Nil)
        }
        other => other.clone(),
    }
}
