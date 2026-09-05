//! Tests for `⍬` (Null) handling in the math/reduce/catenate/enclose/reshape
//! builtins. These are the T2.2-expansion fixes from PROGRESS-20260903c.md:
//! each entry here was oracle-verified against `kap-jvm-text` and is a real
//! port divergence with the Kotlin reference (not just a display
//! rendering difference).
//!
//! Per Kotlin's `APLEmptyArray` (types.kt:1660), `⍬` is a rank-1 size-0
//! array, NOT a unit value. The port's `APLValue::Null` is unit-shaped,
//! so each builtin that dispatches on `b.rank()` or `b.dimensions()` has
//! been patched at its entry point to either:
//!   1. **propagate Null** through math/comparison (e.g. `+⍬ → ⍬`)
//!   2. **return the function's identity** for reduce-over-empty
//!      (e.g. `+/⍬ → 0`, `×/⍬ → 1`)
//!   3. **materialize Null as a rank-1 [0] array** for shape/reshape/
//!      reduce so subsequent rank-based dispatch works
//!
//! The `format_value` (used by `eval_to_string`) renders `Null` as
//! `"(null)"` (its internal token) but `format_display` and
//! `format_conform` use `⍬`. The tests below use a custom `show`
//! that matches `format_conform` so the values line up with the oracle
//! transcripts.

use kap_core::Engine;
use kap_core::APLValue;
use kap_core::KapNumber;

fn show(v: &APLValue) -> String {
    match v {
        APLValue::Null => "⍬".to_string(),
        APLValue::Nil => "null".to_string(),
        APLValue::Array(a) => {
            // `⍬` is now a real rank-1 empty array (not Null): render it as
            // `⍬`, matching `format_conform`/`format_display`.
            if a.element_count() == 0 && a.dimensions.len() <= 1 {
                return "⍬".to_string();
            }
            let elems: Vec<String> = a.elements().iter().map(|e| show(e)).collect();
            if a.dimensions.len() > 1 {
                format!("[{}]", elems.join(" "))
            } else {
                format!("({})", elems.join(" "))
            }
        }
        APLValue::Number(n) => n.format(false),
        APLValue::Str(s) => s.clone(),
        APLValue::Char(c) => c.to_string(),
        _ => format!("{:?}", v),
    }
}

fn run(engine: &Engine, expr: &str) -> Result<String, String> {
    engine
        .eval_string(expr)
        .map(|v| show(v.as_ref()))
        .map_err(|e| format!("{:?}", e))
}

#[test]
fn monadic_math_propagates_null() {
    // Oracle-verified: monadic math on `⍬` returns `⍬` (Kotlin propagates
    // the APLNullValue through the operation, not an error).
    let e = Engine::new();
    for op in ["+", "-", "×", "÷", "|", "⌈", "⌊"] {
        let r = run(&e, &format!("{}⍬", op)).unwrap();
        assert_eq!(r, "⍬", "expected {}⍬ → ⍬, got {:?}", op, r);
    }
}

#[test]
fn dyadic_math_propagates_null() {
    // Oracle-verified: `⍬ <op> X` and `X <op> ⍬` return `⍬` (Kotlin
    // combine2Arg `fnOther` branch handles APLNilValue operands).
    let e = Engine::new();
    for op in ["+", "-", "×", "÷"] {
        for (a, b) in [("⍬", "1"), ("1", "⍬"), ("⍬", "⍬")] {
            let r = run(&e, &format!("{} {} {}", a, op, b)).unwrap();
            assert_eq!(r, "⍬", "expected {} {} {} → ⍬, got {:?}", a, op, b, r);
        }
    }
}

#[test]
fn comparison_propagates_null() {
    // Oracle-verified: `⍬ = ⍬` → `⍬` (not a runtime error and not 1,
    // which would be the case if `=` returned by deep equality).
    let e = Engine::new();
    assert_eq!(run(&e, "⍬=⍬").unwrap(), "⍬");
    // `⍬ ≡ ⍬` → 1 (deep equality: both are the same singleton).
    assert_eq!(run(&e, "⍬≡⍬").unwrap(), "1");
}

#[test]
fn reduce_over_empty_uses_identity() {
    // Oracle-verified: reduce over `⍬` returns the algebraic identity
    // (Kotlin `reduce.kt:82-83` → `fn.identityValue()`).
    let e = Engine::new();
    assert_eq!(run(&e, "+/⍬").unwrap(), "0", "+/⍬ should be 0 (additive identity)");
    assert_eq!(run(&e, "+⌿⍬").unwrap(), "0", "+⌿⍬ should be 0");
    assert_eq!(run(&e, "×/⍬").unwrap(), "1", "×/⍬ should be 1 (multiplicative identity)");
    // `-` is also additive identity in Kotlin (subtractive identity is 0).
    assert_eq!(run(&e, "-/⍬").unwrap(), "0", "-/⍬ should be 0");
}

#[test]
fn reshape_with_null_shape_discloses_first() {
    // Oracle-verified: `⍬⍴X` returns the first element of arrayify(X),
    // with primitives passing through and arrays getting enclosed
    // (Kotlin `reshape.kt:249-257` → `EnclosedAPLValue.make(valueAt(0))`).
    let e = Engine::new();
    assert_eq!(run(&e, "⍬⍴5").unwrap(), "5", "primitive scalar passes through");
    assert_eq!(run(&e, "⍬⍴1 2 3").unwrap(), "1", "first element of vector");
    // `⍬⍴⍬` → 0 (empty array prototype, then enclosed but primitive).
    assert_eq!(run(&e, "⍬⍴⍬").unwrap(), "0");
}

#[test]
fn catenate_with_null_promotes_scalar() {
    // Oracle-verified: `⍬,1` → `⟨1⟩` (1-element vector), not bare `1`.
    // The port's old code returned the scalar unchanged.
    let e = Engine::new();
    assert_eq!(run(&e, "⍬,1").unwrap(), "(1)");
    assert_eq!(run(&e, "1,⍬").unwrap(), "(1)");
    // `⍬,1 2 3` → `1 2 3` (the partner is a vector; no extra wrap needed).
    assert_eq!(run(&e, "⍬,1 2 3").unwrap(), "(1 2 3)");
    // `⍬,⍬` → `⍬` (Kotlin: both empty arrays absorb each other into a
    // single `⍬`).
    assert_eq!(run(&e, "⍬,⍬").unwrap(), "⍬");
}

#[test]
fn enclose_null_keeps_box() {
    // Oracle-verified: `⊂⍬` → `┌─┐` (a 0-D box containing `⍬`).
    // The port's old code special-cased `Null` to pass through unchanged.
    let e = Engine::new();
    // The data is a 0-D nested array; the rendering differs from the
    // oracle's box-frames `┌─┐` but the structure is correct (rank 0,
    // one element which is `⍬`).
    let r = run(&e, "⊂⍬").unwrap();
    assert_eq!(r, "(⍬)", "enclose ⍬ should be a rank-0 box; got {:?}", r);
}

#[test]
fn shape_of_null_is_one_element_zero() {
    // Oracle-verified: `⍴⍬` → `⟨0⟩` (1-D shape vector of length 0).
    let e = Engine::new();
    let r = run(&e, "⍴⍬").unwrap();
    assert_eq!(r, "(0)", "shape of ⍬ is a 1-element [0] vector; got {:?}", r);
}

// --- `null` (nil singleton) vs `⍬` (empty array), 2026-09-05 ---
//
// Kotlin ground truth: `⍬` → `APLNullValue` (rank-1 empty array),
// the `null` keyword → `NilToken` → `APLNilValue` (rank-0 singleton).
// Every case below was verified against `kap-jvm-text` before coding.

#[test]
fn nil_is_distinct_from_empty() {
    let e = Engine::new();
    assert_eq!(run(&e, "null").unwrap(), "null");
    assert_eq!(run(&e, "⍬").unwrap(), "⍬");
    assert_eq!(run(&e, "⍬≡null").unwrap(), "0");
    assert_eq!(run(&e, "null≡null").unwrap(), "1");
    assert_eq!(run(&e, "⍬≡⍬").unwrap(), "1");
    assert_eq!(run(&e, "≢⍬").unwrap(), "0");
    assert_eq!(run(&e, "≢null").unwrap(), "1");
    assert_eq!(run(&e, "⍴null").unwrap(), "⍬");
    assert_eq!(run(&e, "1 null 2").unwrap(), "(1 null 2)");
}

#[test]
fn nil_arithmetic_identity() {
    // `+`: nil+nil → 0; one-sided nil passes the other side through.
    // `×`: nil×nil → 1; one-sided nil passes through.
    // `-`/`÷`: nil on the right passes numbers/arrays through.
    let e = Engine::new();
    for (expr, expected) in [
        ("null+null", "0"),
        ("null+5", "5"),
        ("5+null", "5"),
        ("null×null", "1"),
        ("null×5", "5"),
        ("5×null", "5"),
        ("5-null", "5"),
        ("5÷null", "5"),
        ("(1 2 3)+null", "(1 2 3)"),
        ("null+(1 2 3)", "(1 2 3)"),
        ("(1 2 3)-null", "(1 2 3)"),
        ("(1 2 3)×null", "(1 2 3)"),
        ("(1 2 3)÷null", "(1 2 3)"),
        ("\"ab\"+null", "ab"),
        ("null+\"ab\"", "ab"),
        ("@a+null", "a"),
        ("null+@a", "a"),
        ("(1 null 2)+(10 20 30)", "(11 20 32)"),
    ] {
        assert_eq!(run(&e, expr).unwrap(), expected, "{}", expr);
    }
}

#[test]
fn nil_arithmetic_errors() {
    // `-`/`÷` with nil on the left, nil beside Char/Str under `-`,
    // and monadic math on nil all error (oracle-verified).
    let e = Engine::new();
    for expr in [
        "null-5",
        "null-null",
        "\"ab\"-null",
        "null-\"ab\"",
        "@a-null",
        "null-@a",
        "null÷5",
        "null÷null",
        "+null",
        "-null",
        "×null",
        "2⋆null",
        "null⋆2",
        "5|null",
    ] {
        assert!(run(&e, expr).is_err(), "expected {} to error", expr);
    }
}

#[test]
fn nil_minmax_identity() {
    // `⌊`/`⌈`: one-sided nil returns the other side; both-nil errors.
    let e = Engine::new();
    for (expr, expected) in [
        ("null⌊5", "5"),
        ("5⌊null", "5"),
        ("null⌈5", "5"),
        ("5⌈null", "5"),
    ] {
        assert_eq!(run(&e, expr).unwrap(), expected, "{}", expr);
    }
    assert!(run(&e, "null⌊null").is_err());
    assert!(run(&e, "null⌈null").is_err());
}

#[test]
fn nil_equality_and_ordering() {
    // `=`/`≠` are value-like on nil; ordering throws.
    let e = Engine::new();
    for (expr, expected) in [
        ("null=null", "1"),
        ("null≠null", "0"),
        ("null=5", "0"),
        ("5=null", "0"),
        ("null≠5", "1"),
    ] {
        assert_eq!(run(&e, expr).unwrap(), expected, "{}", expr);
    }
    for expr in ["null<5", "5<null", "null<null"] {
        assert!(run(&e, expr).is_err(), "expected {} to error", expr);
    }
}

#[test]
fn nil_other_builtins() {
    // Enclose/disclose/ravel/tally with nil (all oracle-verified).
    let e = Engine::new();
    assert_eq!(run(&e, "⊂null").unwrap(), "null");
    assert_eq!(run(&e, "⊃null").unwrap(), "null");
    assert_eq!(run(&e, ",null").unwrap(), "(null)");
    assert_eq!(run(&e, "≢null").unwrap(), "1");
}
