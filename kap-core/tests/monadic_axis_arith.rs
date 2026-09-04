//! Monadic axis-applied arithmetic: `+[k] x`, `-[k] x`, `×[k] x`, `÷[k] x`, `*[k] x`.
//!
//! Kotlin `MathCombineAPLFunction.eval1Arg` (math_functions.kt:428) silently drops
//! the axis and applies the monadic form. Oracle-verified:
//!
//! | expr            | oracle      | port         | meaning     |
//! |-----------------|-------------|--------------|-------------|
//! | `+[0] 3`        | `3`         | `3`          | identity    |
//! | `-[1] 5`        | `¯5`        | `¯5`         | negate      |
//! | `×[2] ¯7`       | `¯1`        | `¯1`         | signum      |
//! | `÷[3] 4`        | `1r4`       | `1r4`        | reciprocal  |
//! | `*[0] 2`        | `e^2`       | `e^2`        | exponential |
//! | `+[0] (2 3 ⍴ ⍳6)` | shape `[2 3]` | shape `[2 3]` | identity    |
//!
//! Regression test for the bug fixed in T1.1 carryover (PROGRESS-20260904.md
//! Part 4 d1 audit: "+[axis] monadic (~1) — reverted; needs ResizedArrayImpls work").
//! The simpler "axis is silently dropped" oracle behavior lets us fix it via
//! re-entry into the existing monadic dispatch instead of building the full
//! resize/disclose machinery.

use std::rc::Rc;

use kap_core::APLValue;
use kap_core::Engine;

fn run(expr: &str) -> Result<String, String> {
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

fn eval(expr: &str) -> Rc<APLValue> {
    Engine::new().eval_string(expr).expect(expr)
}

#[test]
fn monadic_axis_arith_drops_axis() {
    // Scalar cases — oracle-verified via `printf 'expr\n' | kap-jvm-text`.
    assert_eq!(run("+[0] 3").unwrap(), "3"); // identity
    assert_eq!(run("+[1] 3").unwrap(), "3"); // axis ignored
    assert_eq!(run("+[5] 3").unwrap(), "3"); // axis ignored, even out of range

    // -[k] x = negate
    let s = run("-[1] 5").unwrap();
    assert!(s == "¯5" || s == "-5", "got {:?}", s);

    // ×[k] x = signum
    assert_eq!(run("×[2] 7").unwrap(), "1");
    let s = run("×[2] ¯7").unwrap();
    assert!(s == "¯1" || s == "-1", "got {:?}", s);

    // ÷[k] x = reciprocal
    let s = run("÷[3] 4").unwrap();
    assert!(s == "1r4" || s == "0.25" || s == "1/4", "got {:?}", s);

    // *[k] x = exponential
    let s = run("*[0] 0").unwrap();
    assert!(!s.is_empty(), "got {:?}", s);
    assert!(s.contains('1'), "exp(0) should be 1, got {:?}", s);
}

#[test]
fn monadic_axis_arith_preserves_array_shape() {
    // +[axis] on a rank-2 array must preserve shape (axis silently dropped,
    // not broadcast to rank-3 like ResizedArrayImpls.makeResizedArray).
    // `⍴` returns a rank-1 vector with the dimensions as elements.
    let v = eval("⍴ +[1] 2 3 ⍴ ⍳6");
    match &*v {
        APLValue::Array(a) => assert_eq!(a.dimensions, vec![2]),
        _ => panic!("expected array, got {:?}", v),
    }
    let dims = run("⍴ +[1] 2 3 ⍴ ⍳6").unwrap();
    assert_eq!(dims, "(2 3)", "shape should be [2, 3]");

    // Sanity: -[axis] preserves too
    let dims = run("⍴ -[0] 2 3 ⍴ ⍳6").unwrap();
    assert_eq!(dims, "(2 3)");
}

#[test]
fn monadic_axis_arith_values_match_plain_monadic() {
    // Value-equivalence: `+[k] x` must equal `+ x` (the plain monadic form).
    let with_axis = run("+[0] (2 3 ⍴ ⍳6)").unwrap();
    let plain = run("+ (2 3 ⍴ ⍳6)").unwrap();
    assert_eq!(with_axis, plain, "+[k] must equal plain monadic +");

    let with_axis = run("-[1] (2 3 ⍴ ⍳6)").unwrap();
    let plain = run("- (2 3 ⍴ ⍳6)").unwrap();
    assert_eq!(with_axis, plain, "-[k] must equal plain monadic -");

    let with_axis = run("×[2] (2 3 ⍴ ⍳6)").unwrap();
    let plain = run("× (2 3 ⍴ ⍳6)").unwrap();
    assert_eq!(with_axis, plain, "×[k] must equal plain monadic ×");
}

#[test]
fn monadic_axis_arith_no_regression_dyadic() {
    // The new monadic bypass must NOT swallow dyadic scalar+scalar (which has
    // its own short-circuit at evaluator.rs:1616).
    assert_eq!(run("2 +[0] 3").unwrap(), "5");
    assert_eq!(run("10 -[0] 3").unwrap(), "7");
    assert_eq!(run("2 ×[0] 5").unwrap(), "10");
}