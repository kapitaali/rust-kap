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
    /// (used to build a child scope when the function is applied). `split` = the number
    /// of leading `params` that are bound to the *left* (dyadic) argument; the remainder
    /// are bound to the right argument. For a monadic function `split == 0`.
    UserFn {
        params: Vec<String>,
        split: usize,
        body: AplRef<ast::Instr>,
        env: AplRef<Environment>,
    },
    /// A user-defined *operator*: a function defined with function-operands, e.g.
    /// `∇ (x foo) a { 10 ⍞x a }`. `op_left`/`op_right` are the names bound to the
    /// function-operands supplied at the *call site* (`+foo 2` → op_left=`+`;
    /// `-foo+ 3` → op_left=`-`, op_right=`+`). `left_params`/`right_params` are the
    /// ordinary data arguments. `arity` = number of function-operands (1 or 2).
    UserOp {
        op_left: Option<String>,
        op_right: Option<String>,
        left_params: Vec<String>,
        right_params: Vec<String>,
        body: AplRef<ast::Instr>,
        env: AplRef<Environment>,
    },
}

impl APLValue {
    pub fn is_null(&self) -> bool {
        matches!(self, APLValue::Null)
    }

    /// Render a value for display (REPL / tests). Mirrors Kap's value printing.
    ///
    /// This is the **plain** renderer: strings are shown *without* surrounding
    /// double quotes. It is the canonical internal representation used by `⍕`
    /// (format) and by operator results (so `⍕"foo"` => `foo`, `"af" + 1 ¯1`
    /// => `be`). Keep this quote-free — see [`APLValue::format_display`] for the
    /// REPL-style renderer that wraps strings in double quotes.
    pub fn format_value(&self) -> String {
        match self {
            APLValue::Number(n) => n.format(true),
            APLValue::Char(c) => c.to_string(),
            APLValue::Str(s) => s.clone(),
            APLValue::Null => "null".to_string(),
            APLValue::Array(a) => {
                // Kap vectors render with parentheses, not brackets (brackets are
                // reserved for array indexing). Simple 1-D vector render; nested
                // arrays recurse and each level also uses parentheses.
                let parts: Vec<String> = a.elements().iter().map(|e| e.format_value()).collect();
                format!("({})", parts.join(" "))
            }
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
        }
    }

    /// Render a value for the **REPL / file output** path — Real Kap conformance.
    ///
    /// Unlike [`APLValue::format_value`], this wraps string values in *double
    /// quotes* and renders the null value as `⍬` (matching Real Kap's REPL:
    /// typing `"foo bar"` prints `"foo bar"`, and `⊣ io:println "x"` shows `⍬`).
    /// Arrays and numbers are rendered as in `format_value` (strings inside an
    /// array are also quoted, so a vector of strings shows `( "a" "b" )`).
    pub fn format_display(&self) -> String {
        match self {
            APLValue::Number(n) => n.format(true),
            // REPL / "pretty" form: a character is shown with an `@` prefix
            // (Real Kap: `↑"abc"` prints `@a`, ` @a ` prints `@a`). The internal
            // `format_value` (used by `⍕` and operator results) stays bare.
            APLValue::Char(c) => format!("@{}", c),
            APLValue::Str(s) => format!("\"{}\"", escape_string(s)),
            APLValue::Null => "⍬".to_string(),
            APLValue::Array(a) => {
                let elems = a.elements();
                // A 1-D vector of characters is a "string value" in Real Kap and
                // renders as a *quoted string* (`@a @b @c` -> "abc"), not as a
                // parenthesised `@a @b @c` list. Only the scalar Char gets `@`.
                if a.dimensions.len() == 1
                    && elems.iter().all(|e| matches!(e.as_ref(), APLValue::Char(_)))
                {
                    let s: String = elems
                        .iter()
                        .map(|e| match e.as_ref() {
                            APLValue::Char(c) => *c,
                            _ => unreachable!(),
                        })
                        .collect();
                    format!("\"{}\"", escape_string(&s))
                } else {
                    let parts: Vec<String> = elems.iter().map(|e| e.format_display()).collect();
                    format!("({})", parts.join(" "))
                }
            }
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
        }
    }
}

/// Escape a string for REPL display: `\"` and `\` are backslash-escaped, and
/// non-printable control characters are rendered as `\t`, `\n`, `\r`, or
/// `\xNN`. Mirrors Real Kap's string quoting in the REPL result line.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{ArrayData, KapArray};
    use std::rc::Rc;

    #[test]
    fn format_display_char_scalar_has_at_prefix() {
        // Real Kap REPL: char scalar shown with `@` prefix (not "pretty" bare).
        assert_eq!(APLValue::Char('a').format_display(), "@a");
        // Numeric / null unchanged by the display renderer.
        assert_eq!(APLValue::Number(KapNumber::Long(1)).format_display(), "1");
        assert_eq!(APLValue::Null.format_display(), "⍬");
    }

    #[test]
    fn format_display_char_vector_is_quoted_string() {
        // A 1-D char vector is a "string value": rendered as a quoted string,
        // not as a parenthesised `@a @b @c` list.
        let a = APLValue::Array(Rc::new(KapArray::new(
            vec![3],
            ArrayData::Char(vec!['a', 'b', 'c']),
        )));
        assert_eq!(a.format_display(), "\"abc\"");
    }

    #[test]
    fn format_value_char_stays_bare() {
        // The internal plain renderer (⍕, operator results) must NOT add `@`.
        assert_eq!(APLValue::Char('a').format_value(), "a");
    }
}
