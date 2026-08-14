//! AST for Kap (Phase 2b).
//!
//! Mirrors `array/.../instr.kt` (`Instr` hierarchy) at a coarse grain. The Kotlin `Instr`
//! is a large sealed-ish hierarchy; for the Rust port we use a compact `Instr` enum that
//! captures the constructs the evaluator needs. Strategy §4.3.
//!
//! Reference: docs/reference.asciidoc (syntax sections).

use crate::KapNumber;
use crate::token::LiteralValue;

/// A parsed expression. This is what the evaluator (Phase 3) consumes.
#[derive(Debug, Clone)]
pub enum Instr {
    /// A literal scalar value (number, char, string, null).
    Literal(LiteralValue),
    /// A variable / function name reference. `namespace` is `Some` for `foo:bar`.
    Symbol { name: String, namespace: Option<String> },
    /// Monadic or dyadic function application: `f x` (monadic) or `a f b` (dyadic).
    /// `fn_expr` is the function (a Symbol or parenthesised expr); `left`/`right` are args.
    Apply {
        fn_expr: Box<Instr>,
        left: Option<Box<Instr>>,
        right: Box<Instr>,
    },
    /// Assignment: `target ← value`.
    Assign {
        target: Box<Instr>,
        value: Box<Instr>,
    },
    /// An array/vector literal: `[a;b;c]` (explicit) or a stranded vector `a b c`.
    Array { elements: Vec<Instr> },
    /// Empty array / nil.
    Empty,
}

impl Instr {
    /// Convenience: build a literal number Instr.
    pub fn number(n: KapNumber) -> Instr {
        Instr::Literal(LiteralValue::Number(n))
    }
    /// Convenience: build a symbol Instr.
    pub fn symbol(name: &str) -> Instr {
        Instr::Symbol { name: name.to_string(), namespace: None }
    }
}
