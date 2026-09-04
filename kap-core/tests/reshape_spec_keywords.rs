use kap_core::Engine;
use kap_core::APLValue;

fn run_case(expr: &str) -> Result<String, String> {
    let engine = Engine::new();
    match engine.eval_string(expr) {
        Ok(v) => Ok(format_value(&v)),
        Err(e) => Err(format!("{:?}", e)),
    }
}

fn format_value(v: &APLValue) -> String {
    match v {
        APLValue::Array(a) => {
            if a.dimensions.len() > 1 {
                let elems: Vec<String> = a.elements().iter().map(|e| format_value(e)).collect();
                format!("[shape={:?} {}]", a.dimensions, elems.join(" "))
            } else {
                let elems: Vec<String> = a.elements().iter().map(|e| format_value(e)).collect();
                format!("({})", elems.join(" "))
            }
        }
        APLValue::Number(n) => format!("{}", n),
        APLValue::Str(s) => format!("\"{}\"", s),
        APLValue::Char(c) => format!("'{}'", c),
        APLValue::Null => "⍬".to_string(),
        APLValue::Symbol { name, .. } => format!("`{}`", name),
        _ => format!("{:?}", v),
    }
}

#[test]
fn reshape_spec_keywords() {
    // P3 reshape spec keywords (:match / :fill / :truncate / :recycle)
    // Per Kotlin ReshapeTest.kt:85-205 and reshape.kt:375-393.
    //
    // Oracle-verified via `printf 'expr\n' | kap-jvm-text`:
    //   2 :match ⍴ ⍳4  → 2x2 of 0..3 (computed = 4/2 = 2)
    //   2 ¯1 ⍴ ⍳4      → 2x2 of 0..3 (literal -1 = match)
    //   2 :fill ⍴ ⍳3   → 2x2 of 0..3,0 (padded with defaultValue=0)
    //   2 :truncate ⍴ ⍳5 → 2x2 of 0..3 (excess dropped)
    //   2 :recycle ⍴ ⍳3 → 2x2 of 0..2,0 (recycled one more)
    //   3 :match ⍴ 1+⍳9 → 3x3 of 1..9 (computed = 9/3 = 3)
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("2 :match ⍴ ⍳4",        vec![2, 2]),
        ("2 ¯1 ⍴ ⍳4",            vec![2, 2]),
        ("2 :fill ⍴ ⍳3",         vec![2, 2]),
        ("2 :truncate ⍴ ⍳5",     vec![2, 2]),
        ("2 :recycle ⍴ ⍳3",      vec![2, 2]),
        ("3 :match ⍴ 1+⍳9",      vec![3, 3]),
        // The "Only a single dimension is allowed to be marked as computed" case:
        ("¯1 ¯1 ⍴ ⍳4",           vec![]), // should ERROR
    ];

    let mut pass = 0;
    let mut fail = 0;
    for (expr, expected_dims) in &cases {
        match run_case(expr) {
            Ok(actual) => {
                if actual.contains(&format!("{:?}", expected_dims)) {
                    pass += 1;
                    println!("OK   {} => {} ✓", expr, actual);
                } else {
                    fail += 1;
                    println!("FAIL {} => {} (expected dims {:?})", expr, actual, expected_dims);
                }
            }
            Err(e) => {
                if expected_dims.is_empty() {
                    pass += 1;
                    println!("OK   {} => ERROR (expected) {}", expr, e);
                } else {
                    fail += 1;
                    println!("FAIL {} => ERROR {}", expr, e);
                }
            }
        }
    }
    println!("\nreshape_spec_keywords: {}/{} passed", pass, pass + fail);
    assert_eq!(fail, 0, "reshape spec keywords regressed");
}
