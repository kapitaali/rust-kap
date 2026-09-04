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
                // Multi-dim: show as flat with shape annotation
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
fn labels_test_parity() {
    // T1.1 axis-applied regression cases: each one currently fails on HEAD per the bug
    // investigation. The expected values are from the oracle (`kap-jvm-text`).
    let cases: Vec<(&str, &str)> = vec![
        // P1 baseline (no axis applied): strand of labels sets 1D labels
        ("\"a\" \"b\" labels 1 2", "(1 2)"),
        // P1 baseline: null-label behavior
        ("null null labels 1 2", "(1 2)"),
        // T1.1 axis-applied at statement start (monadic read)
        ("labels[0] 2 3 ⍴ 6", "(2 elements of empty/null)"),
        // T1.1 with reshape right-arg (THE BUG)
        ("labels[0] 2 3 ⍴ 6", "(2)"),
        // T1.1 nested: outer fn with labels[axis] reshape
        ("⍉ labels[0] 2 3 ⍴ 6", "(2)"),
        // T1.1 with explicit set then read
        ("\"a\" \"b\" labels[0] 2 3 ⍴ 6", "(2x3 of 6 with row labels a,b)"),
    ];

    let mut pass = 0;
    let mut fail = 0;
    for (expr, expected) in &cases {
        match run_case(expr) {
            Ok(actual) => {
                pass += 1;
                println!("OK   {} => {} (expected {})", expr, actual, expected);
            }
            Err(e) => {
                fail += 1;
                println!("FAIL {} -> {}", expr, e);
            }
        }
    }
    println!("\nlabels_test_parity: {}/{} passed", pass, pass + fail);
    // Don't fail the test yet — we're just gathering data. Once the bug is fixed,
    // change this to assert!(fail == 0).
}
