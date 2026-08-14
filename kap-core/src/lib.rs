//! kap-core: the Kap language engine (value model, tokeniser, parser, evaluator).
//!
//! Phase 0 skeleton. This crate is intentionally UI-agnostic: it knows nothing
//! about terminals or REPLs. See `RUST_REWRITE_STRATEGY.md` (repo root) for the
//! full plan and locked decisions.
//!
//! Locked decisions (strategy §9):
//!   D1 single-threaded `Rc<APLValue>`, immutable arrays.
//!   D2 `num-bigint`/`num-rational` for numbers (no GMP yet).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub mod number;
pub mod array;
pub mod token;
pub mod lexer;
pub mod lex_helpers;
pub mod ast;
pub mod parser;
pub mod evaluator;
pub mod session;

/// A persistent, REPL-like Kap evaluation context. State (variables, user
/// functions) survives across `eval` calls. See [`session::Session`].
pub use session::Session;

pub use number::KapNumber;
pub use array::{ArrayData, KapArray};

/// Shared-reference alias for Kap values. Locked per D1: single-threaded `Rc`.
/// If parallelism is added later (Phase 7), change this one line to `Arc` and
/// the rest of the crate follows.
pub type AplRef<T> = Rc<T>;

/// The root of all Kap values. Mirrors Kap's `APLValue`.
///
/// Phase 1: scalars (numbers/chars/strings), arrays, and the null value.
/// Phase 2b: `Deferred` — the LAZY-EVALUATION thunk. Kap is lazy: a function argument
/// that should not be evaluated immediately is represented as a `Deferred` value carrying
/// an unevaluated `Instr` plus the environment it closes over. The evaluator (Phase 3)
/// forces it on demand via `force()`. This keeps function arguments as trees in the
/// `Instr`/`APLValue` model rather than forcing them at parse time.
#[derive(Debug, Clone)]
pub enum APLValue {
    Number(KapNumber),
    Char(char),
    Str(String),
    Array(AplRef<KapArray>),
    Null,
    /// An unevaluated expression (lazy thunk). `instr` is the tree; `env` is the lexical
    /// environment captured at the point of deferral.
    Deferred { instr: AplRef<ast::Instr>, env: AplRef<Environment> },
    /// A user-defined function (lambda / tradfn). `params` are argument names; `body` is
    /// the unevaluated expression; `env` is the closure captured at definition time
    /// (used to build a child scope when the function is applied).
    UserFn { params: Vec<String>, body: AplRef<ast::Instr>, env: AplRef<Environment> },
}

impl APLValue {
    pub fn is_null(&self) -> bool {
        matches!(self, APLValue::Null)
    }

    /// Render a value for display (REPL / tests). Mirrors Kap's value printing.
    pub fn format_value(&self) -> String {
        match self {
            APLValue::Number(n) => n.format(true),
            APLValue::Char(c) => c.to_string(),
            APLValue::Str(s) => s.clone(),
            APLValue::Null => "null".to_string(),
            APLValue::Array(a) => {
                // simple 1-D vector render
                let parts: Vec<String> = a.elements().iter().map(|e| e.format_value()).collect();
                format!("[{}]", parts.join(" "))
            }
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
        }
    }
}

/// Lexical environment. Symbols live behind a `RefCell` so assignment can mutate the
/// shared `Rc<Environment>` in place (single-threaded, per D1). `parent` enables lexical
/// scoping: a lookup walks outward until it finds the name.
#[derive(Debug, Default, Clone)]
pub struct Environment {
    /// Symbols defined in this scope: key = (name, namespace). Values are shared refs.
    pub symbols: RefCell<HashMap<(String, Option<String>), AplRef<APLValue>>>,
    /// Parent scope for lexical lookup.
    pub parent: Option<AplRef<Environment>>,
}

/// Engine: holds namespaces, symbols, and the standard output sink.
/// Phase 0 stub — real fields (namespaces, symbol table, module registry) land
/// in Phase 3.
#[derive(Debug, Default)]
pub struct Engine {
    pub standard_output: Option<String>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Kap evaluation errors. Parse errors carry the source position where the
/// problem was detected (1-based line/column, matching the lexer's spans).
/// Runtime errors (undefined symbol, type mismatch, etc.) have no single
/// source position, so they use [`AplError::Runtime`] with just a message.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AplError {
    /// A problem detected during lexing/parsing at a known source position.
    #[error("parse error at {line}:{col}: {msg}")]
    Parse { line: usize, col: usize, msg: String },
    /// A runtime error (name resolution, type/valence, division, …) raised while
    /// evaluating a well-formed expression. Position is not tracked for these.
    #[error("error: {0}")]
    Runtime(String),
}

impl AplError {
    /// Build a runtime error from a message.
    pub fn runtime(msg: String) -> Self {
        AplError::Runtime(msg)
    }
}
