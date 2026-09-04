//! Regression tests for `∧[axis]` / `∨[axis]` sort-along-axis (T1.1 carryover).
//!
//! Oracle-verified behavior (Kotlin `sortKapArray`, sort.kt:192, with axis
//! validated by `ensureValidAxis`):
//! - `∧[axis] x` sorts ascending along the chosen axis
//! - `∨[axis] x` sorts descending along the chosen axis
//! - Axis out of range → "Axis N is not valid. Expected: <rank>"
//! - Dyadic with axis specifier → "Axis N is not valid. Expected: <left_rank>"
//!   (Kotlin `AndOrToken` dyadic path is bitwise; it does not accept an axis)

use kap_core::{Engine, APLValue};

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

#[test]
fn sort_axis_ascending_rank1() {
    // `∧[0] 3 1 2` → sorted ascending along axis 0
    assert_eq!(run("∧[0] 3 1 2").unwrap(), "(1 2 3)");
}

#[test]
fn sort_axis_descending_rank1() {
    // `∨[0] 3 1 2` → sorted descending along axis 0
    assert_eq!(run("∨[0] 3 1 2").unwrap(), "(3 2 1)");
}

#[test]
fn sort_axis_rank1_axis1_invalid() {
    // Rank-1 array has only axis 0; axis 1 is not valid.
    let out = run("∧[1] 3 1 2").unwrap_err();
    assert!(
        out.contains("Axis 1 is not valid"),
        "expected axis-1 error, got: {out}"
    );
    assert!(out.contains("Expected: 1"), "got: {out}");
}

#[test]
fn sort_axis_rank2_first_axis() {
    // `∨[0] 2 3⍴⍳6` → sort along axis 0 (rows reordered)
    // ⍳6 = 0 1 2 3 4 5; 2 3⍴⍳6 = [[0 1 2] [3 4 5]]
    // After sort descending: [[3 4 5] [0 1 2]] (port's format_value uses
    // [shape=[2,3] ...] for multi-dim, but the data layout is row-major
    // so the element order is 3 4 5 0 1 2).
    let out = run("∨[0] 2 3⍴⍳6").unwrap();
    assert!(out.contains("3 4 5 0 1 2"), "got: {out}");
}

#[test]
fn sort_axis_rank2_last_axis() {
    // `∨[1] 2 3⍴⍳6` → sort along axis 1 (each row reversed)
    // [[0 1 2] [3 4 5]] → [[2 1 0] [5 4 3]]
    let out = run("∨[1] 2 3⍴⍳6").unwrap();
    assert!(out.contains("2 1 0 5 4 3"), "got: {out}");
}

#[test]
fn sort_axis_rank2_ascending_first_axis() {
    // `∧[0] 2 3⍴⍳6` → rows already ascending: [[0 1 2] [3 4 5]]
    let out = run("∧[0] 2 3⍴⍳6").unwrap();
    assert!(out.contains("0 1 2 3 4 5"), "got: {out}");
}

#[test]
fn sort_axis_rank3_outer_axis() {
    // `∨[0] 2 2 3⍴⍳12` → reverse along outer axis
    // ⍳12 = 0..11, shape (2 2 3) = [[[0 1 2][3 4 5]] [[6 7 8][9 10 11]]]
    // After ∨[0] reverse: [[[6 7 8][9 10 11]] [[0 1 2][3 4 5]]]
    let out = run("∨[0] 2 2 3⍴⍳12").unwrap();
    assert!(out.contains("6 7 8 9 10 11 0 1 2 3 4 5"), "got: {out}");
}

#[test]
fn sort_axis_rank3_inner_axis() {
    // `∨[2] 2 2 3⍴⍳12` → reverse along inner axis
    // [[[0 1 2][3 4 5]] [[6 7 8][9 10 11]]]
    // After ∨[2] reverse: [[[2 1 0][5 4 3]] [[8 7 6][11 10 9]]]
    let out = run("∨[2] 2 2 3⍴⍳12").unwrap();
    assert!(out.contains("2 1 0 5 4 3 8 7 6 11 10 9"), "got: {out}");
}

#[test]
fn sort_axis_no_spec_still_works() {
    // `∧ x` and `∨ x` (no axis spec) must still work as before.
    assert_eq!(run("∧ 3 1 2").unwrap(), "(1 2 3)");
    assert_eq!(run("∨ 3 1 2").unwrap(), "(3 2 1)");
    // Also for higher-rank: data layout matches the expected ordering
    // (rows reordered to descending).
    let out = run("∧ 2 3⍴⍳6").unwrap();
    assert!(out.contains("0 1 2 3 4 5"), "got: {out}");
    let out = run("∨ 2 3⍴⍳6").unwrap();
    assert!(out.contains("3 4 5 0 1 2"), "got: {out}");
}

#[test]
fn sort_axis_dyadic_scalar_left_rejected() {
    // `2 ∧[0] 3 1 2`: dyadic with axis; left is a scalar (rank 0).
    // Oracle: "Axis 0 is not valid. Expected: 0" (because `ensureValidAxis`
    // runs on the LEFT arg's dims).
    let out = run("2 ∧[0] 3 1 2").unwrap_err();
    assert!(
        out.contains("Axis 0 is not valid"),
        "expected axis-0 error, got: {out}"
    );
    assert!(out.contains("Expected: 0"), "got: {out}");
}

#[test]
fn sort_axis_dyadic_vector_left_rejected() {
    // `(1 2) ∧[0] 3 1`: dyadic with axis; left is rank 1 → Expected: 1.
    let out = run("(1 2) ∧[0] 3 1").unwrap_err();
    assert!(
        out.contains("Axis 0 is not valid"),
        "expected axis-0 error, got: {out}"
    );
    assert!(out.contains("Expected: 1"), "got: {out}");
}

#[test]
fn sort_axis_axis_out_of_range_rank3() {
    // 3-rank array: valid axes are 0, 1, 2. Axis 3 is not valid.
    let out = run("∧[3] 2 2 3⍴⍳12").unwrap_err();
    assert!(
        out.contains("Axis 3 is not valid"),
        "expected axis-3 error, got: {out}"
    );
    assert!(out.contains("Expected: 3"), "got: {out}");
}

#[test]
fn sort_axis_matches_plain_monadic_axis0() {
    // `∧[0] x == ∧ x` for any array (axis-0 sort = first-axis sort = default).
    let out_axis = run("∧[0] 2 3⍴⍳6").unwrap();
    let out_plain = run("∧ 2 3⍴⍳6").unwrap();
    assert_eq!(out_axis, out_plain);
    let out_axis2 = run("∨[0] 2 3⍴⍳6").unwrap();
    let out_plain2 = run("∨ 2 3⍴⍳6").unwrap();
    assert_eq!(out_axis2, out_plain2);
}
