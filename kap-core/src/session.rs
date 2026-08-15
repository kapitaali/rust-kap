//! Stateful Kap evaluation session (Mode 2).
//!
//! `Session` keeps one `Environment` alive across calls, so variables and
//! user-defined functions assigned in one `eval` are visible in the next —
//! exactly like the Kap REPL. For one-shot stateless evaluation, use
//! [`crate::Engine::eval_string`] directly (Mode 1).

use crate::{APLValue, AplError, AplRef, Engine, Environment};
use std::rc::Rc;

/// A persistent Kap evaluation context. State lives in the shared
/// `Rc<Environment>`, so it survives across `eval` calls.
///
/// ```no_run
/// use kap_core::Session;
/// let s = Session::new();
/// let _ = s.eval("x ← 5");
/// let v = s.eval("x + 1").unwrap(); // 6
/// ```
#[derive(Debug)]
pub struct Session {
    engine: Engine,
    env: AplRef<Environment>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Create a new session with an empty environment.
    pub fn new() -> Self {
        Session {
            engine: Engine::new(),
            env: Rc::new(Environment::default()),
        }
    }

    /// Evaluate `src` in the session's persistent environment. Returns the value
    /// of the last statement. Variables assigned here remain defined for
    /// subsequent `eval` calls (REPL-like persistence).
    pub fn eval(&self, src: &str) -> Result<AplRef<APLValue>, AplError> {
        self.engine.eval_string_in_env(src, &self.env)
    }

    /// Convenience: evaluate `src` and return its formatted (REPL-style) string.
    pub fn eval_to_string(&self, src: &str) -> Result<String, AplError> {
        Ok(self.eval(src)?.format_value())
    }

    /// Number of symbols currently defined in the session's top-level scope.
    pub fn len(&self) -> usize {
        self.env.symbols.borrow().len()
    }

    /// Whether the session defines no symbols yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_persists_variables() {
        let s = Session::new();
        s.eval("x ← 5").unwrap();
        s.eval("y ← 10").unwrap();
        assert_eq!(s.eval("x + y").unwrap().format_value(), "15");
        // assignment that reads a prior variable
        assert_eq!(s.eval("z ← x × y").unwrap().format_value(), "50");
        // z is now visible in the next call
        assert_eq!(s.eval("z + 1").unwrap().format_value(), "51");
    }

    #[test]
    fn session_persists_lambda_and_sees_globals() {
        let s = Session::new();
        s.eval("double ← λ(x) x × 2").unwrap();
        assert_eq!(s.eval("double 21").unwrap().format_value(), "42");
        s.eval("k ← 3").unwrap();
        s.eval("addk ← λ(x) x + k").unwrap();
        assert_eq!(s.eval("addk 10").unwrap().format_value(), "13");
    }

    #[test]
    fn stateless_engine_eval_string_does_not_persist() {
        // Mode 1: each call gets a fresh environment.
        let e = Engine::new();
        let _ = e.eval_string("x ← 99").unwrap();
        let r = e.eval_string("x + 1");
        assert!(r.is_err(), "stateless eval must not see prior vars");
    }

    #[test]
    fn mode1_eval_to_string_formats() {
        let e = Engine::new();
        assert_eq!(e.eval_to_string("2 + 3").unwrap(), "5");
        // fresh env means a prior assignment is invisible
        let _ = e.eval_string("a ← 7").unwrap();
        assert!(e.eval_to_string("a + 1").is_err());
    }

    #[test]
    fn parse_errors_carry_real_position() {
        // A parse error must report the real line/col, not 0:0.
        let e = Engine::new();
        match e.eval_string("a ← 3\nb ← 4\nc +") {
            Err(AplError::Parse { line, col, .. }) => assert_eq!((line, col), (3, 4)),
            other => panic!("expected Parse error, got {:?}", other),
        }
    }

    #[test]
    fn runtime_errors_separate_from_parse() {
        // Runtime errors (e.g. unknown function) are their own variant and carry
        // a clean message; they must not be reported as parse errors.
        let s = Session::new();
        match s.eval("foo 1 2") {
            Err(AplError::Runtime(msg)) => assert!(msg.contains("foo")),
            other => panic!("expected Runtime error, got {:?}", other),
        }
    }
}
