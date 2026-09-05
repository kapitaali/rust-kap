//! Tests for the `→` return escape with bind-time target capture.
//!
//! Kotlin's `ReturnFunction.make` (div_functions.kt:528) captures the nearest
//! return-target environment when `→` is instantiated as a function value
//! (`S ⇐ →`, `λ→`); calling it throws `ReturnValue(value, returnEnvironment)`
//! and each call frame only catches a signal aimed at its own environment
//! (engine.kt `withStackFrame`), rethrowing the rest. The port tags
//! `AplError::Return` with the target env id and frames compare before catching.
//!
//! All expectations below are oracle-verified (`kap-jvm-text`):
//! - `{ S ⇐ → ◊ 100 + { S ⍵+20 ◊ ⍵+1 } 10 } 0` → `30` (escape skips the inner dfn)
//! - `{ →5 } 0` → `5` (direct return, bare block is its own frame)
//! - `{ S ⇐ → ⋄ S 30 } 0` → `30`, `{ f ⇐ λ→ ⋄ f 30 } 0` → `30`
//! - `S ⇐ →` → error `→: Call to return without a function call` (bind time)
//! - `λ→` → error `Call to return without a function call` (no `→: ` prefix)
//! - `→5` → error `→: Call to return without a function call`
//! - `{S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0` → error `Return outside of expected frame`
//!   (an escape crossing `¨` dies: Kotlin runs each in a detached stack)
//!
//! NOT covered: `comp`-collapse propagation (`{S ⇐ → ⋄ comp {...}¨⍳10} 0` → `9`
//! in the oracle) needs lazy `¨` (Kotlin's each forces elements on demand, so
//! `⍕`/index/`≢`/`comp` see values while statement print errors). The port's
//! `¨` is eager — recorded for the roadmap, not attempted here.

use kap_core::Engine;

fn run(expr: &str) -> Result<String, String> {
    let engine = Engine::new();
    engine
        .eval_string(expr)
        .map(|v| v.as_ref().format_value())
        .map_err(|e| format!("{:?}", e))
}

fn ok(expr: &str) -> String {
    run(expr).unwrap_or_else(|e| panic!("expected value for {:?}, got error: {}", expr, e))
}

fn err(expr: &str) -> String {
    run(expr).unwrap_err()
}

#[test]
fn escape_skips_inner_dfn() {
    // The sweep's S⇐→ case: S(30) exits the OUTER fn, `100 +` never completes.
    assert_eq!(ok("{ S ⇐ → ◊ 100 + { S ⍵+20 ◊ ⍵+1 } 10 } 0"), "30");
}

#[test]
fn direct_return_in_bare_block() {
    assert_eq!(ok("{ →5 } 0"), "5");
}

#[test]
fn escape_called_directly() {
    assert_eq!(ok("{ S ⇐ → ⋄ S 30 } 0"), "30");
}

#[test]
fn lambda_arrow_captures_escape() {
    assert_eq!(ok("{ f ⇐ λ→ ⋄ f 30 } 0"), "30");
}

#[test]
fn bind_time_error_without_function() {
    // Oracle errors AT BIND TIME, with the `→: ` prefix.
    assert!(
        err("S ⇐ →").contains("Call to return without a function call"),
        "got: {}",
        err("S ⇐ →")
    );
}

#[test]
fn lambda_arrow_error_without_function() {
    // Oracle: `λ→` at top level, WITHOUT the `→: ` prefix.
    let e = err("λ→");
    assert!(e.contains("Call to return without a function call"), "got: {}", e);
    assert!(!e.contains("→:"), "must not have the → prefix, got: {}", e);
}

#[test]
fn top_level_return_errors() {
    // At engine level an uncaught `→` escapes as the raw `Return` signal
    // (the CLI converts it to "→: Call to return without a function call",
    // verified against the oracle via the REPL probe, not here).
    let e = err("→5");
    assert!(e.starts_with("Return("), "got: {}", e);
}

#[test]
fn escape_through_each_errors() {
    // The sweep's non-comp case: both sides error identically.
    assert!(
        err("{S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0").contains("Return outside of expected frame"),
        "got: {}",
        err("{S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0")
    );
}
