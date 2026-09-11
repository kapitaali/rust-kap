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
//! - `{S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0` → value with a suspended return (an
//!   escape crossing `¨` suspends: Kotlin errors here — detached stack —
//!   which the harness and stale-collapse reproduce as errors)
//!
//! `comp`-collapse propagation (`{S ⇐ → ⋄ comp {...}¨⍳10} 0` → `9` in the
//! oracle) works via `SuspendedReturn`: the port's eager `¨` suspends a
//! `→`-return raised by an element instead of propagating it; `comp`-collapse
//! re-raises inside the live frame so the block catches it, while an
//! uncollapsed leak errors at the boundary (Kotlin's detached-stack error).

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
fn escape_collapses_through_comp() {
    // The sweep's comp case (oracle `9`): `comp` collapses the `¨` result
    // inside the live frame, so the suspended return re-raises there and the
    // block catches it.
    assert_eq!(ok("{S ⇐ → ⋄ comp {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0"), "9");
}

#[test]
fn escape_through_each_suspends_until_collapse() {
    // The sweep's non-comp case: the port's `¨` is eager, so the return is
    // SUSPENDED as a value (Kotlin errors here — detached stack — because its
    // `¨` forces elements outside the frame). Forcing past frame exit errors:
    // collapse the leaked array in a fresh statement and the stale return
    // surfaces like Kotlin's `Return outside of expected frame`.
    let v = ok("{S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0");
    assert!(v.contains("[return]"), "suspended marker should leak, got: {}", v);
    let e = err("x ← {S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0 ⋄ comp x");
    assert!(e.contains("Return"), "stale collapse should raise, got: {}", e);
}
