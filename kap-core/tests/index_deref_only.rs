//! Regression tests for top-level `[...]` as index deref (T1.3 carryover).
//!
//! Per Kotlin `parser.kt:898-903` (processIndex → processLeftArgAdjustment),
//! a bare `[` at the start of a value expression is an INDEX DEREFERENCE
//! without a left value to dereference — and errors with
//! "Index dereference without argument". The port's previous behavior
//! was to parse `[...]` as a list literal (e.g. `[0]` → `(0)`,
//! `[]` → `()`), which silently diverged from the oracle.
//!
//! The list-literal syntax still works inside a paren group where there
//! is a left-arg context (e.g. `f[0]`, `(+)[0]` after a value, or after
//! the `=` in an assignment). The `parse_index_suffix` path handles
//! these cases correctly.

use kap_core::Engine;

fn err_msg(src: &str) -> String {
    let engine = Engine::new();
    match engine.eval_string(src) {
        Ok(v) => panic!("expected error for `{}`, got Ok({:?})", src, v),
        Err(e) => format!("{:?}", e),
    }
}

#[test]
fn only_brackets_zero() {
    // `[0]` at top level: oracle errors "Index dereference without argument".
    let msg = err_msg("[0]");
    assert!(
        msg.contains("Index dereference without argument"),
        "expected index-deref error, got: {}",
        msg
    );
}

#[test]
fn only_brackets_empty() {
    // `[]` at top level: same error.
    let msg = err_msg("[]");
    assert!(
        msg.contains("Index dereference without argument"),
        "expected index-deref error, got: {}",
        msg
    );
}

#[test]
fn empty_brackets_in_assignment() {
    // `a ← []` at top level: same error.
    let msg = err_msg("a ← []");
    assert!(
        msg.contains("Index dereference without argument"),
        "expected index-deref error, got: {}",
        msg
    );
}

#[test]
fn nonempty_brackets_in_assignment() {
    // `a ← [1; 2; 3]` at top level: same error (the brackets are still
    // at the start of a fresh value expression after `=`).
    let msg = err_msg("a ← [1; 2; 3]");
    assert!(
        msg.contains("Index dereference without argument"),
        "expected index-deref error, got: {}",
        msg
    );
}

#[test]
fn brackets_after_value_still_works_as_index() {
    // `(1 2 3)[0]` — the `[` comes AFTER a value, so it is an index
    // deref, not a list literal. The fix must not affect this case.
    let engine = Engine::new();
    let v = engine.eval_string("(1 2 3)[0]").expect("should parse and eval");
    assert_eq!(v.format_value(), "1");
}

#[test]
fn brackets_after_value_multi_dim() {
    // `(2 3 ⍴ ⍳6)[1;2]` — multi-axis index deref after a value.
    let engine = Engine::new();
    let v = engine.eval_string("(2 3 ⍴ ⍳6)[1;2]").expect("should parse and eval");
    assert_eq!(v.format_value(), "5");
}
