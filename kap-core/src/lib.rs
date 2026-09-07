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
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

pub mod number;
pub mod array;
pub mod token;
pub mod lexer;
pub mod lex_helpers;
pub mod ast;
pub mod parser;
pub mod evaluator;
pub mod encoder;
pub mod map;
pub mod session;

/// A persistent, REPL-like Kap evaluation context. State (variables, user
/// functions) survives across `eval` calls. See [`session::Session`].
pub use session::Session;

pub use number::KapNumber;
pub use array::{ArrayData, KapArray};
pub use map::KapMap;

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
    /// A `;`-separated *list* value (Kotlin `APLList` / `LiteralAPLList`).
    /// Distinct from `Array` (space-stranded): `typeof (1;2;3)` → `kap:list`,
    /// `typeof (1 2 3)` → `kap:array`. Destructuring assignment `(a;b;c)←RHS`
    /// requires the RHS to be a list, not an array.
    List(AplRef<KapArray>),
    /// The APL **empty-array** value (Kotlin `APLNullValue : APLArray`, source
    /// spelling `⍬`). Rank-1, zero elements. Displays as `⍬`. This is NOT the
    /// nil singleton: `⍬≡null` → `0`, `⍴⍬` → `(0)` but `⍴null` → `⍬`.
    Null,
    /// The APL **nil** singleton (Kotlin `APLNilValue : APLSingleValue`, source
    /// spelling the `null` keyword). A rank-0 scalar with its own class
    /// (`typeof null` → `kap:null`). Displays as `null` (bare, and inside
    /// strands: `1 null 2` → `(1 null 2)`). Dyadic `+ - × ÷ ⌊ ⌈` treat it as a
    /// missing operand (identity / other-side / error per function); every
    /// other scalar function errors on it.
    Nil,
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
    /// A captured **return escape** (Kotlin `ReturnFunctionImpl`): the value of
    /// `→` in function position (`S ⇐ →`, `λ→` inside a fn). `target` is the id
    /// of the nearest enclosing return-target frame at capture time. Applying it
    /// raises `AplError::Return(value, target)`, which propagates outward until
    /// the frame with that id catches it. `None` = captured with no enclosing
    /// function (only constructible transiently; binding/applying reports
    /// "Call to return without a function call" like the oracle).
    Escape {
        target: Option<usize>,
    },
    /// A non-binding macro function (`:nfunction` arg in `defsyntax`, Kotlin
    /// `DeclaredNonBoundFunction` — "ignores its arguments"). `body` is the
    /// unevaluated argument source. Application evaluates `body` in the
    /// CALLER's context with no new scope and no `⍵`/`⍺` bindings, so ambient
    /// arguments show through (`foo { x+⍵ }` with outer `⍵=3` yields `x+3`).
    NonBoundFn {
        body: AplRef<ast::Instr>,
    },
    /// A first-class **symbol** value (Kap `APLSymbol` wrapping a `Symbol`).
    /// Created by the `'foo` literal and `int:intern`; read by `int:symbolName`.
    /// `namespace` is `None` for the default namespace, `Some("keyword")` for the
    /// `:foo` keyword form. Distinct from `Str` (symbols are interned, comparable
    /// by name+namespace, and render as `ns:name`).
    Symbol {
        name: String,
        namespace: Option<String>,
    },
    /// A native Kap hashmap (`APLMap`). Immutable; keyed by value-equal
    /// (type-discriminating). Created by `map:with`, read by `map:get`, etc.
    /// See `map.rs` / `builtins/map.kt`.
    Map(KapMap),
}

impl APLValue {
    pub fn is_null(&self) -> bool {
        matches!(self, APLValue::Null)
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, APLValue::Nil)
    }

    /// Kap class name for the `typeof` builtin (Kotlin `SystemClass` names:
    /// `INTEGER`, `FLOAT`, `COMPLEX`, `RATIONAL`, `CHAR`, `ARRAY`, `SYMBOL`,
    /// `LAMBDA_FN`, `LIST`, `MAP`, …). Returns the bare class name; the `typeof`
    /// Kap class name for the `typeof` builtin (Kotlin `SystemClass` names, lowercase,
    /// e.g. `integer`, `float`, `array`, `symbol`, `char`, `string`, `list`, `map`).
    pub fn class_name(&self) -> &'static str {
        match self {
            APLValue::Number(n) => match n {
                KapNumber::Long(_) => "integer",
                KapNumber::Double(_) => "float",
                KapNumber::BigInt(_) => "integer",
                KapNumber::Rational(_) => "rational",
                KapNumber::Complex(_, _) => "complex",
            },
            APLValue::Char(_) => "char",
            // Strings are arrays in Kap (Kotlin APLString : APLArray()).
            // `typeof "x"` → kap:array.
            APLValue::Str(_) => "array",
            APLValue::Array(_) => "array",
            APLValue::List(_) => "list",
            APLValue::Null => "null",
            APLValue::Nil => "null",
            APLValue::Deferred { .. } => "deferred",
            // Kotlin `SystemClass.LAMBDA_FN = SystemClass("function")`
            // (objects.kt:32): `typeof` of any function value is `kap:function`
            // (oracle: `typeof(y)` on an operator operand → `kap:function`).
            APLValue::UserFn { .. } => "function",
            APLValue::UserOp { .. } => "operator",
            // A captured return escape reports as a function (it only ever
            // appears where a function value is expected: `S ⇐ →`, `λ→`).
            APLValue::Escape { .. } => "function",
            // A non-binding macro function is still a function value.
            APLValue::NonBoundFn { .. } => "function",
            APLValue::Symbol { .. } => "symbol",
            APLValue::Map(_) => "map",
        }
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
            APLValue::Number(n) => n.format(false), // PLAIN style: ASCII minus (matches oracle ⍕)
            APLValue::Char(c) => c.to_string(),
            APLValue::Str(s) => s.clone(),
            APLValue::Null => "⍬".to_string(),
            APLValue::Nil => "null".to_string(),
            APLValue::Array(a) => {
                // Kap vectors render with parentheses, not brackets (brackets are
                // reserved for array indexing). Simple 1-D vector render; nested
                // arrays recurse and each level also uses parentheses.
                let parts: Vec<String> = a.elements().iter().map(|e| e.format_value()).collect();
                format!("({})", parts.join(" "))
            }
            APLValue::List(a) => {
                // A `;`-separated list. Kotlin's internal toString uses `(1 2 3)`
                // notation (same as arrays) — `⟨⟩` is only for conform-display.
                let parts: Vec<String> = a.elements().iter().map(|e| e.format_value()).collect();
                format!("({})", parts.join(" "))
            }
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
            APLValue::Escape { .. } => "<function>".to_string(),
            APLValue::NonBoundFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
            APLValue::Map(m) => format!("map[size={}]", m.len()),
        }
    }

    /// Render a value by **recursively flattening to scalar leaves and concatenating**
    /// with no separators and no parentheses. This is Kap's `formatted(FormatStyle.PLAIN)`
    /// used by **monadic `⍕`**: `⍕ 1 2 3 → "123"`, `⍕(2 2⍴⍳4) → "0123"`,
    /// `⍕"ab" → "ab"`, `⍕⊂1 2 3 → "123"` (a box is flattened, not parenthesised).
    /// Strings contribute their characters unquoted. This deliberately differs from
    /// [`APLValue::format_value`] (which wraps arrays in `( … )` with spaces).
    pub fn format_plain(&self) -> String {
        match self {
            APLValue::Number(n) => n.format(true),
            APLValue::Char(c) => c.to_string(),
            APLValue::Str(s) => s.clone(),
            APLValue::Null => String::new(),
            APLValue::Nil => "null".to_string(),
            APLValue::Array(a) => {
                a.elements().iter().map(|e| e.format_plain()).collect()
            }
            APLValue::List(a) => {
                a.elements().iter().map(|e| e.format_plain()).collect()
            }
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
            APLValue::Escape { .. } => "<function>".to_string(),
            APLValue::NonBoundFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
            APLValue::Map(m) => format!("map[size={}]", m.len()),
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
            APLValue::Number(n) => n.format(false), // PLAIN: ASCII '-' (matches oracle REPL)
            // REPL / "pretty" form: a character is shown with an `@` prefix
            // (Real Kap: `↑"abc"` prints `@a`, ` @a ` prints `@a`). The internal
            // `format_value` (used by `⍕` and operator results) stays bare.
            APLValue::Char(c) => format!("@{}", c),
            APLValue::Str(s) => format!("\"{}\"", escape_string(s)),
            APLValue::Null => "⍬".to_string(),
            APLValue::Nil => "null".to_string(),
            APLValue::Array(a) => {
                let elems = a.elements();
                // An empty array displays as `⍬` (Real Kap: `↓⍬`, `⍬ ∩ ⍳10`,
                // `⊃⍬` all print `⍬`, and `⍬` itself is now a real empty
                // rank-1 array rather than Null).
                if elems.is_empty() && a.dimensions.len() <= 1 {
                    return "⍬".to_string();
                }
                // A 1-D vector of characters is a "string value" in Real Kap and
                // renders as a *quoted string* (`@a @b @c` -> "abc"), not as a
                // parenthesised `@a @b @c` list. Only the scalar Char gets `@`.
                if !elems.is_empty()
                    && a.dimensions.len() == 1
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
            APLValue::Escape { .. } => "<function>".to_string(),
            APLValue::NonBoundFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
            APLValue::List(a) => {
                let parts: Vec<String> = a.elements().iter().map(|e| e.format_display()).collect();
                format!("⟨{}⟩", parts.join(" "))
            }
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
            APLValue::Map(m) => format!("map[size={}]", m.len()),
        }
    }

    /// Render a value in **conform mode** — oracle-compatible display.
    ///
    /// This matches Real Kap's `formatted(FormatStyle.PLAIN)` for scalars and
    /// `encloseInBox` for arrays. Used by `--conform-display` for manual
    /// comparison with the oracle. Differences from `format_display`:
    /// - Negative numbers use ASCII `-` (not `¯`).
    /// - 1-D vectors use `⟨⟩` (not `()`).
    /// - Multi-dim arrays use box frames (┌→──┐ etc.).
    /// - Empty arrays use `⍬` or `┌⊖┐` (not `()`).
    pub fn format_conform(&self) -> String {
        match self {
            APLValue::Number(n) => n.format(false), // readable=false → ASCII minus
            APLValue::Char(c) => c.to_string(),
            APLValue::Str(s) => s.clone(),
            APLValue::Null => "⍬".to_string(),
            APLValue::Nil => "null".to_string(),
            APLValue::Array(a) => Self::format_conform_array(a, false),
            APLValue::List(a) => Self::format_conform_array(a, true),
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
            APLValue::Escape { .. } => "<function>".to_string(),
            APLValue::NonBoundFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
            APLValue::Map(m) => format!("map[size={}]", m.len()),
        }
    }

    /// Wrap a rendered value in a box frame (for enclosed arrays).
    fn enclose_in_box_frame(content: &str) -> String {
        let lines: Vec<&str> = content.split('\n').collect();
        let max_width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let mut result = String::new();
        result.push('┌');
        for _ in 0..max_width {
            result.push('─');
        }
        result.push('┐');
        for line in &lines {
            result.push('\n');
            result.push('│');
            result.push_str(line);
            let pad = max_width - line.chars().count();
            for _ in 0..pad {
                result.push(' ');
            }
            result.push('│');
        }
        result.push('\n');
        result.push('└');
        for _ in 0..max_width {
            result.push('─');
        }
        result.push('┘');
        result
    }

    /// Render an array in conform (oracle-compatible) mode.
    ///
    /// Handles rank-0 (scalar), rank-1 (`⟨⟩`), and rank-2 (box frames).
    /// Higher-rank arrays fall back to a compact debug representation.
    /// `is_list` = true for `List` values (enclosed arrays from `⊂`), which
    /// use box frames even for rank-1.
    fn format_conform_array(a: &KapArray, is_list: bool) -> String {
        let dims = &a.dimensions;
        let rank = dims.len();

        // Empty array (any dimension is 0)
        if dims.iter().any(|&d| d == 0) {
            return if rank <= 1 {
                "⍬".to_string()
            } else {
                "┌⊖┐".to_string()
            };
        }

        // Rank 0 (scalar array)
        if rank == 0 {
            let elems = a.elements();
            let elem = elems.first().map(|e| e.as_ref().clone());
            return match elem {
                Some(e) if matches!(&e, APLValue::Array(inner) if inner.dimensions.len() > 0)
                    || matches!(&e, APLValue::List(inner) if inner.dimensions.len() > 0) =>
                {
                    Self::enclose_in_box_frame(&e.format_conform())
                }
                Some(e) => e.format_conform(),
                None => String::new(),
            };
        }

        // Rank 1: ⟨elem1 elem2 ...⟩ for arrays, box frame for lists
        if rank == 1 {
            let parts: Vec<String> = a
                .elements()
                .iter()
                .map(|e| e.format_conform())
                .collect();
            if is_list {
                // Enclosed 1-D vector: box frame
                let content_width = parts.join(" ").chars().count();
                let mut result = String::new();
                result.push('┌');
                for _ in 0..content_width {
                    result.push('─');
                }
                result.push('┐');
                result.push('\n');
                result.push('│');
                result.push_str(&parts.join(" "));
                result.push('│');
                result.push('\n');
                result.push('└');
                for _ in 0..content_width {
                    result.push('─');
                }
                result.push('┘');
                return result;
            }
            return format!("⟨{}⟩", parts.join(" "));
        }

        // Rank 2: box-frame rendering
        if rank == 2 {
            let rows = dims[0];
            let cols = dims[1];

            // Compute per-column widths for cell alignment
            let mut col_widths = vec![0usize; cols];
            for c in 0..cols {
                for r in 0..rows {
                    let idx = r * cols + c;
                    let w = a.elements()[idx].format_conform().chars().count();
                    if w > col_widths[c] {
                        col_widths[c] = w;
                    }
                }
            }

            // Build row strings with right-justified cells
            let mut row_strs = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut cells = Vec::with_capacity(cols);
                for c in 0..cols {
                    let idx = r * cols + c;
                    let val = a.elements()[idx].format_conform();
                    let pad = col_widths[c] - val.chars().count();
                    let mut cell = String::new();
                    for _ in 0..pad {
                        cell.push(' ');
                    }
                    cell.push_str(&val);
                    cells.push(cell);
                }
                row_strs.push(cells.join(" "));
            }

            let content_width = col_widths.iter().sum::<usize>() + cols - 1;

            let mut result = String::new();
            result.push('┌');
            result.push('→');
            for _ in 1..content_width {
                result.push('─');
            }
            result.push('┐');
            result.push('\n');
            for row in &row_strs {
                result.push('│');
                result.push_str(row);
                result.push('│');
                result.push('\n');
            }
            result.push('└');
            for _ in 0..content_width {
                result.push('─');
            }
            result.push('┘');
            return result;
        }

        // Rank 3+: compact fallback (conformance tests rarely check these)
        format!("<{:?} array>", dims)
    }

    // --- Array-shape accessors ---
    // A `Str` is a rank-1 array (vector of its chars) in Kap, exactly matching
    // Kotlin `APLBmpString.dimensions = dimensionsOfSize(content.length)`. These
    // accessors let array primitives (rho/tally/reverse/transpose) treat `Str`
    // uniformly with `Array` without restructuring the value (which would touch
    // every `Str(...)` construction site). Scalars (Number/Char/Null) are rank 0.

    /// Dimensions of the value. `Str` => `[len]`; scalars => `[]`; arrays => their dims.
    /// A `List` is a RANK-0 scalar (Kotlin `APLList : APLSingleValue`,
    /// `dimensions = emptyDimensions()`), so it reports `[]` even though its backing
    /// `KapArray` is stored rank-1 (see `Instr::List` eval arm in evaluator.rs).
    pub fn dimensions(&self) -> Vec<usize> {
        match self {
            APLValue::Array(a) => a.dimensions.clone(),
            APLValue::List(_) => vec![],
            APLValue::Str(s) => vec![s.chars().count()],
            _ => vec![],
        }
    }

    /// Rank of the value (`dimensions().len()`).
    pub fn rank(&self) -> usize {
        match self {
            APLValue::Array(a) => a.dimensions.len(),
            APLValue::List(_) => 0,
            APLValue::Str(s) => {
                if s.is_empty() {
                    0
                } else {
                    1
                }
            }
            _ => 0,
        }
    }

    /// Total element count. `Str` => char count; scalars => 1; arrays => product of dims.
    pub fn element_count(&self) -> usize {
        match self {
            APLValue::Array(a) => a.element_count(),
            APLValue::List(a) => a.element_count(),
            APLValue::Str(s) => s.chars().count(),
            _ => 1,
        }
    }

    /// Element at flat index `i`. For `Str`, returns the i-th character as a `Char`.
    pub fn value_at(&self, i: usize) -> APLValue {
        match self {
            APLValue::Array(a) => a.elements().get(i).map(|e| e.as_ref().clone()).unwrap_or(APLValue::Null),
            APLValue::List(a) => a.elements().get(i).map(|e| e.as_ref().clone()).unwrap_or(APLValue::Null),
            APLValue::Str(s) => s
                .chars()
                .nth(i)
                .map(APLValue::Char)
                .unwrap_or(APLValue::Null),
            other => other.clone(),
        }
    }

    /// All elements as `APLValue`s, in flat (row-major) order. Unlike
    /// `KapArray::elements`, this works on any `APLValue` (scalars, strings) and is
    /// the right call when you need to iterate a whole value more than once — callers
    /// should hoist this ONCE rather than calling `value_at(i)` in a loop, because
    /// `KapArray::elements` rebuilds the whole `Vec` on every call.
    pub fn elements(&self) -> Vec<AplRef<APLValue>> {
        match self {
            APLValue::Array(a) => a.elements(),
            APLValue::List(a) => a.elements(),
            APLValue::Str(s) => s.chars().map(|c| Rc::new(APLValue::Char(c))).collect(),
            APLValue::Null => vec![],
            other => vec![Rc::new(other.clone())],
        }
    }

    /// Cross-kind total-order comparison, mirroring Kotlin `compareTotalOrdering`
    /// (with `typeDiscrimination = true`, as all of Kap's sorting/grade/match/`cmp`
    /// paths use td=true). Two numeric values (any mix of Long/BigInt/Rational/
    /// Double/Complex) compare numerically with type discrimination; two chars compare
    /// by codepoint; two strings/arrays compare by rank then element-wise
    /// (`compareAPLArrays`); two lists compare element-wise; otherwise distinct types
    /// order by Kap's `typeSortOrder` (number < char < symbol < array < list < null).
    /// Non-orderable pairs (incl. complex) → `None`.
    pub fn total_cmp(&self, other: &APLValue) -> Option<Ordering> {
        use std::cmp::Ordering::*;
        use APLValue::*;
        // Number vs Number → Kotlin compareTotalOrdering (type discrimination, never errors
        // on complex; a complex with im≠0 sorts by type position after Double).
        if let (Number(a), Number(b)) = (self, other) {
            return Some(KapNumber::number_ordering(a, b));
        }
        match (self, other) {
            (Char(a), Char(b)) => Some(a.cmp(b)),
            // String or array vs string or array → rank-first, then element-wise
            // (Kotlin compareAPLArrays: lower rank is "less"; equal rank-1 →
            // lexicographic by element; higher rank → dimension-vector then elements).
            (Str(_), Str(_))
            | (Str(_), Array(_))
            | (Array(_), Str(_))
            | (Array(_), Array(_)) => {
                let ea = self.elements();
                let eb = other.elements();
                let ra = self.rank();
                let rb = other.rank();
                if ra != rb {
                    return Some(ra.cmp(&rb));
                }
                if ra == 1 && rb == 1 {
                    let n = ea.len().min(eb.len());
                    for i in 0..n {
                        let o = ea[i].total_cmp(&eb[i])?;
                        if o != Equal {
                            return Some(o);
                        }
                    }
                    return Some(ea.len().cmp(&eb.len()));
                }
                let dcmp = self.dimensions().cmp(&other.dimensions());
                if dcmp != Equal {
                    return Some(dcmp);
                }
                let n = ea.len().min(eb.len());
                for i in 0..n {
                    let o = ea[i].total_cmp(&eb[i])?;
                    if o != Equal {
                        return Some(o);
                    }
                }
                Some(ea.len().cmp(&eb.len()))
            }
            // List vs List → element-wise (Kotlin APLList.compareSameType).
            (List(_), List(_)) => {
                let ea = self.elements();
                let eb = other.elements();
                let n = ea.len().min(eb.len());
                for i in 0..n {
                    let o = ea[i].total_cmp(&eb[i])?;
                    if o != Equal {
                        return Some(o);
                    }
                }
                Some(ea.len().cmp(&eb.len()))
            }
            (Null, Null) => Some(Equal),
            (Nil, Nil) => Some(Equal),
            (Symbol { name: n1, namespace: ns1 }, Symbol { name: n2, namespace: ns2 }) => {
                Some(ns1.cmp(ns2).then(n1.cmp(n2)))
            }
            // Distinct types → by Kap type sort order (typeSortOrder in types.kt):
            // Long/BigInt/Rational/Double/Complex=0..4, Char=5, Symbol=6, Array=7,
            // Map=8, List=9, Timestamp=10, Nil=11. Numbers are handled above; here we
            // assign a position per remaining kind and order by the difference.
            _ => {
                let pos = |v: &APLValue| -> Option<usize> {
                    match v {
                        Number(_) => Some(0),
                        Char(_) => Some(5),
                        Symbol { .. } => Some(6),
                        Str(_) | Array(_) => Some(7),
                        List(_) => Some(9),
                        Null | Nil => Some(11),
                        _ => None,
                    }
                };
                let pa = pos(self)?;
                let pb = pos(other)?;
                Some(pa.cmp(&pb))
            }
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


/// Shared namespace registry. Lives behind an `Rc` so every `Environment` (root and all
/// child lexical scopes) sees the same set of named namespaces and the current namespace.
/// Mirrors Kotlin's `Engine.namespaces`/`currentNamespace` (the module-level symbol table),
/// distinct from the per-scope lexical `Environment.symbols` table (which holds dfn params
/// like `⍵`/`⍺` and block locals).
#[derive(Debug, Default, Clone)]
pub struct NamespaceRegistry {
    /// The "current" namespace name. `None` = the implicit default namespace.
    pub current: RefCell<Option<String>>,
    /// `name -> (symbolName -> value)` for module-scope bindings, keyed by bare name within
    /// each namespace. (Kotlin: `Namespace.localSymbols`.)
    pub symbols: RefCell<HashMap<String, HashMap<String, AplRef<APLValue>>>>,
    /// `name -> set of exported symbol names` (populated by `declare(:export …)`).
    pub exports: RefCell<HashMap<String, HashSet<String>>>,
    /// `name -> list of imported namespace names` (populated by `import(…)`).
    pub imports: RefCell<HashMap<String, Vec<String>>>,
    /// `(ns, name)` pairs that are READ-ONLY. Populated two ways:
    /// - natively at Engine construction for the quad constants `⎕A ⎕a ⎕d` (Kotlin
    ///   registers these as engine constants; oracle: fresh-session `⎕A` works and
    ///   `⎕A ← 5` → "Assignment to constant variable: kap:⎕A");
    /// - by `declare(:const …)` at eval time (B6 / code_analysis_03).
    pub constants: RefCell<HashSet<(String, String)>>,
}

impl NamespaceRegistry {
    /// The default namespace name used when `current` is `None`.
    pub fn default_ns() -> String {
        "default".to_string()
    }
    /// Resolve the *effective* current namespace name (never `None`).
    pub fn current_ns(&self) -> String {
        self.current.borrow().clone().unwrap_or_else(Self::default_ns)
    }
    /// Fetch (or lazily create) the symbol map for a namespace.
    pub fn ns_symbols(&self, ns: &str) -> HashMap<String, AplRef<APLValue>> {
        self.symbols
            .borrow()
            .get(ns)
            .cloned()
            .unwrap_or_default()
    }
    pub fn ns_define(&self, ns: &str, name: &str, val: AplRef<APLValue>) {
        self.symbols
            .borrow_mut()
            .entry(ns.to_string())
            .or_default()
            .insert(name.to_string(), val);
    }
    /// Mark `(ns, name)` as read-only (Kotlin `Namespace.addConstant`).
    /// Oracle behaviour (2026-08-24): `declare(:const q)` BINDS `q ← null`
    /// in the namespace (`⊢ null` on the declare line; a later `q` errors
    /// "Variable not assigned" only because null is unprintable there —
    /// assignment to it still fails with "Assignment to constant variable").
    pub fn declare_const(&self, ns: &str, name: &str) {
        self.constants
            .borrow_mut()
            .insert((ns.to_string(), name.to_string()));
        // Bind the name to Null if not already bound, so the symbol exists.
        {
            let mut syms = self.symbols.borrow_mut();
            let m = syms.entry(ns.to_string()).or_default();
            m.entry(name.to_string()).or_insert_with(|| Rc::new(APLValue::Null));
        }
    }
    /// Whether `(ns, name)` is a read-only constant.
    pub fn is_constant(&self, ns: &str, name: &str) -> bool {
        self.constants
            .borrow()
            .contains(&(ns.to_string(), name.to_string()))
    }
    pub fn ns_lookup(&self, ns: &str, name: &str) -> Option<AplRef<APLValue>> {
        self.symbols
            .borrow()
            .get(ns)
            .and_then(|m| m.get(name))
            .cloned()
    }
    pub fn declare_export(&self, ns: &str, name: &str) {
        self.exports
            .borrow_mut()
            .entry(ns.to_string())
            .or_default()
            .insert(name.to_string());
    }
    pub fn is_exported(&self, ns: &str, name: &str) -> bool {
        self.exports
            .borrow()
            .get(ns)
            .map(|s| s.contains(name))
            .unwrap_or(false)
    }
    /// Namespaces imported by `ns` (via `import("…")`, in order). Used by the
    /// parser's macro-visibility check alongside [`is_exported`].
    pub fn imports_of(&self, ns: &str) -> Vec<String> {
        self.imports.borrow().get(ns).cloned().unwrap_or_default()
    }
    pub fn add_import(&self, ns: &str, imported: &str) {
        self.imports
            .borrow_mut()
            .entry(ns.to_string())
            .or_default()
            .push(imported.to_string());
    }
    /// Resolve a *bare* (namespace-less) name in `current_ns`, honoring `import` fallbacks
    /// then the default namespace (Kotlin `defaultNamespaceFallback`).
    pub fn resolve_bare(&self, name: &str) -> Option<AplRef<APLValue>> {
        let cur = self.current_ns();
        if let Some(v) = self.ns_lookup(&cur, name) {
            return Some(v);
        }
        // Walk imported namespaces (in import order).
        if let Some(imps) = self.imports.borrow().get(&cur).cloned() {
            for imp in imps {
                if let Some(v) = self.ns_lookup(&imp, name) {
                    return Some(v);
                }
            }
        }
        // Fall back to the default namespace.
        if cur != Self::default_ns() {
            if let Some(v) = self.ns_lookup(&Self::default_ns(), name) {
                return Some(v);
            }
        }
        // Fall back to the `kap` namespace (the standard-library namespace). Kap's
        // `base-functions.kap`/`structure.kap`/`math*.kap` define their symbols in the
        // `kap` namespace via `namespace("kap")`; the user session can reference those
        // symbols bare (Kotlin's `kap` namespace is the default user-visible one). This
        // mirrors the `default` fallback above — it only triggers when `kap` is not
        // already the current/imported namespace.
        if cur != "kap" {
            if let Some(v) = self.ns_lookup("kap", name) {
                return Some(v);
            }
        }
        None
    }
    /// Collect names bound to `UserFn` values across all namespaces (used by the parser's
    /// `function_names()` so a user-defined function is recognized as applicable after a
    /// separate `∇`/`⇐` statement).
    pub fn collect_function_names(&self, out: &mut Vec<String>) {
        for m in self.symbols.borrow().values() {
            for (name, val) in m.iter() {
                if matches!(val.as_ref(), APLValue::UserFn { .. }) && !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
    }
    /// Collect names bound to `UserOp` values across all namespaces (parser `operator_names`).
    pub fn collect_operator_names(&self, out: &mut Vec<String>) {
        for m in self.symbols.borrow().values() {
            for (name, val) in m.iter() {
                if matches!(val.as_ref(), APLValue::UserOp { .. }) && !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
    }
    /// Collect names bound to TWO-operand `UserOp` values (`op_right.is_some()`,
    /// i.e. `∇ (x foo y) …`). Seeds the parser's `known_ops2` so a value token
    /// after the operator parses as Kotlin's `ValueCall` operand (op.kt:193-207)
    /// instead of the data argument.
    pub fn collect_operator_names_2arg(&self, out: &mut Vec<String>) {
        for m in self.symbols.borrow().values() {
            for (name, val) in m.iter() {
                if matches!(
                    val.as_ref(),
                    APLValue::UserOp {
                        op_right: Some(_),
                        ..
                    }
                ) && !out.contains(name)
                {
                    out.push(name.clone());
                }
            }
        }
    }
}

/// Lexical environment. Symbols live behind a `RefCell` so assignment can mutate the
/// shared `Rc<Environment>` in place (single-threaded, per D1). `parent` enables lexical
/// scoping: a lookup walks outward until it finds the name. `ns_registry` is the
/// module-level namespace table, shared (via `Rc`) across all scopes.
#[derive(Debug, Default, Clone)]
pub struct Environment {
    /// Unique scope id. Tags `AplError::Return` signals with their target frame
    /// (Kotlin `ReturnValue` carries its `returnEnvironment`). `0` = never
    /// assigned (a `Default`-built value not yet passed through `child`/`new_root`).
    pub id: usize,
    /// Lexical (block-scope) bindings: key = (name, namespace). Values are shared refs.
    /// Holds dfn params (`⍵`/`⍺`), block locals, and operator operands — NOT module symbols.
    pub symbols: RefCell<HashMap<(String, Option<String>), AplRef<APLValue>>>,
    /// Names defined via `⇐` (function definition), NOT `←` (value assignment).
    /// `function_names()` returns only these, so `←`-bound lambdas (which store
    /// a `UserFn` but are VALUES) don't get treated as applicable functions.
    /// See PROBLEM.md (A2).
    pub function_defs: RefCell<HashSet<String>>,
    /// Declared-but-not-yet-assigned function locals (Kap `declare(:local …)`).
    /// A marked name SHADOWS outer bindings: reading it before assignment errors
    /// (`Variable not assigned: ns:name`), and assigning it binds in the marking
    /// scope and clears the mark. Key = (name, namespace), like `symbols`.
    pub unassigned_locals: RefCell<HashSet<(String, Option<String>)>>,
    /// Parent scope for lexical lookup.
    pub parent: Option<AplRef<Environment>>,
    /// Shared namespace registry (module-level symbol table + import/export metadata).
    pub ns_registry: Rc<NamespaceRegistry>,
    /// The namespace this scope is *anchored* to (first non-None up the parent chain).
    /// Set ONLY on the closure-wrapper scopes created for `⇐`/`∇` definitions made
    /// while a `use(...)`d file's `namespace("…")` directive is in effect. A function
    /// body's BARE symbol references must resolve against the DEFINING file's namespace
    /// (Kotlin: a namespace directive scopes to the defining file), not whatever
    /// namespace happens to be current when the function is later CALLED — `use()`
    /// restores the caller's current namespace on return, so without this anchor,
    /// exported fns cannot see their non-exported siblings ("unknown function: helper").
    pub home_ns: RefCell<Option<String>>,
    /// True for the per-file wrapper scope created by `eval_string_in_env_tolerant`.
    /// Makes `define`/`assign` treat module-scope bare bindings exactly like the root
    /// (route them into the namespace table) even though this scope has a parent.
    pub acts_as_root: std::cell::Cell<bool>,
    /// True for the child scope created by `apply_user_fn` / `eval_block` — marks this
    /// scope as a return target for `→`. Mirrors Kotlin's `Environment.isReturnTarget`.
    /// `AplError::Return` propagates up until it finds a scope with this flag set,
    /// then the value is returned from that scope instead of propagating further.
    pub is_return_target: std::cell::Cell<bool>,
    /// Dynamic depth of deferred function/operator-body execution (`apply_user_fn` /
    /// `apply_user_op` body eval). PLAN §2.8b: Kotlin checks const ONLY at
    /// instruction-BUILD time (`deriveLvalueReader`); runtime `setVar` is unchecked.
    /// While > 0, `check_not_constant` is a no-op and def-time hooks stay silent, so
    /// `updateableConstValue` (declare-after-def + call → `2`) keeps working.
    pub fn_body_depth: std::cell::Cell<usize>,
}

/// Process-wide counter issuing unique [`Environment`] ids. Ids tag
/// `AplError::Return` signals with their target frame (Kotlin `ReturnValue`
/// carries its `returnEnvironment`; each call frame only catches its own).
static ENV_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Issue a fresh environment id (`0` is reserved for `Default`-built values,
/// which always receive a real id in `child`/`new_root`).
pub fn next_env_id() -> usize {
    ENV_ID_COUNTER.fetch_add(1, AtomicOrdering::Relaxed) as usize
}

impl Environment {
    /// Build a child lexical scope that inherits the same shared namespace registry.
    pub fn child(parent: &Rc<Environment>) -> Rc<Environment> {
        Rc::new(Environment {
            id: next_env_id(),
            symbols: RefCell::new(HashMap::new()),
            function_defs: parent.function_defs.clone(),
            unassigned_locals: RefCell::new(HashSet::new()),
            parent: Some(parent.clone()),
            ns_registry: parent.ns_registry.clone(),
            home_ns: RefCell::new(None),
            acts_as_root: std::cell::Cell::new(false),
            is_return_target: std::cell::Cell::new(false),
            fn_body_depth: std::cell::Cell::new(0),
        })
    }

    /// Build a fresh ROOT environment whose namespace registry is pre-seeded with the
    /// native quad constants (`⎕A`/`⎕a`/`⎕d`). B5 / code_analysis_03 / ROADMAP P6:
    /// Real Kap registers these as engine-level read-only constants — a FRESH session
    /// (no stdlib) already has `⎕A → "ABCDEFGHIJKLMNOPQRSTUVWXYZ"` and
    /// `⎕A ← 5` errors "Assignment to constant variable: kap:⎕A" (oracle-verified).
    pub fn new_root() -> Rc<Environment> {
        let mut root = Environment::default();
        root.id = next_env_id();
        let env = Rc::new(root);
        {
            let reg = &env.ns_registry;
            for (name, val) in [
                ("⎕A", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
                ("⎕a", "abcdefghijklmnopqrstuvwxyz"),
                ("⎕d", "0123456789"),
            ] {
                reg.declare_const("kap", name);
                reg.ns_define("kap", name, Rc::new(APLValue::Str(val.to_string())));
            }
            // Kap's `null` keyword (Kotlin `NilToken` → `APLNilValue`, the nil
            // singleton — NOT `⍬`, which is the empty array `APLNullValue`).
            // `fhelp.kap` / `standard-lib.kap` reference it (`fhelpFn ← null`).
            // Bind it as a constant to Nil so the symbol resolves like the
            // oracle (`null` → `null`, `⍬` → `⍬`, `⍬≡null` → `0`).
            reg.declare_const("default", "null");
            reg.ns_define("default", "null", Rc::new(APLValue::Nil));
        }
        env
    }

    /// Is this the root (module-level) scope — or a per-file wrapper acting as one?
    /// Root scopes hold module bindings; child scopes hold lexical (block-local) ones.
    pub fn is_root(&self) -> bool {
        self.parent.is_none() || self.acts_as_root.get()
    }

    /// Whether `name` is bound *lexically* (in this scope chain), before consulting the
    /// namespace table. Used to route `←` (assign vs. define) and to keep `⍵`/`⍺` lexical.
    pub fn lexical_contains(&self, name: &str) -> bool {
        let key = (name.to_string(), None);
        let mut cur: Option<&Environment> = Some(self);
        while let Some(e) = cur {
            if e.symbols.borrow().contains_key(&key) {
                return true;
            }
            cur = e.parent.as_ref().map(|p| p.as_ref());
        }
        false
    }
}

/// Engine: holds namespaces, symbols, and the standard output sink.
/// Phase 0 stub — real fields (namespaces, symbol table, module registry) land
/// in Phase 3.
#[derive(Debug, Default)]
pub struct Engine {
    pub standard_output: Option<String>,
    /// Basenames of files currently being loaded via `use(...)`. Guards against
    /// recursive `use` (e.g. `base-functions.kap` calls `use("base-functions.kap")`
    /// at its top). Real Kap has no such guard and StackOverflows; we skip an
    /// already-in-flight include instead so the intended single load succeeds.
    pub include_stack: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
    /// Explicitly-configured standard-library search directories (e.g. set from a
    /// `--lib-path` CLI flag). Consulted first by `use(...)` when resolving a file
    /// by basename — the port's analog of kap-jvm-text's `--lib-path`.
    pub lib_paths: std::rc::Rc<std::cell::RefCell<Vec<std::path::PathBuf>>>,
    /// Registered `defsyntax` / `defsyntaxsub` macros (session-global, like Kotlin's
    /// `engine.customSyntaxSubRules`). `defsyntax` inside a `use(...)` file registers
    /// here so the macro is visible to statements parsed *after* that file loads. Keyed
    /// by bare trigger name (namespace stripped) — sufficient for the stdlib's usage.
    pub macros:
        std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, crate::ast::SyntaxMacro>>>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Prepend one or more standard-library directories (in priority order) to the
    /// `use(...)` search path. Mirrors kap-jvm-text `--lib-path=path` / `-p path`.
    pub fn set_lib_paths<S: AsRef<std::path::Path>>(&self, paths: &[S]) {
        let mut v = self.lib_paths.borrow_mut();
        for p in paths {
            v.push(p.as_ref().to_path_buf());
        }
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
    /// Control-flow return signal raised by the `→` (branch/return) primitive.
    /// Carries the id of the frame it returns from (Kotlin `ReturnValue` carries
    /// its `returnEnvironment`): a frame only catches a signal targeted at its own
    /// env id, all others re-raise it outward. `None` = raised with no enclosing
    /// function — it propagates to the top level, which converts it to a "Call to
    /// return without a function call" runtime error.
    #[error("return: {0:?}")]
    Return(AplRef<APLValue>, Option<usize>),
    /// Control-flow throw signal raised by `throw` (Kotlin `ThrowFunction` →
    /// `TagCatch(ThrowableTag(key, data))`, div_functions.kt:254-267). Carried
    /// separately from `Runtime` so `catch` (CatchOperator, engine.kt:412) can
    /// match the KEY against its handler table; anything uncaught renders as
    /// the oracle text `throw: <data>` at the top level. Monadic `throw x` has
    /// key `Symbol{error, kap}` (Kotlin `internSymbol("error", coreNamespace)`).
    #[error("throw: {1:?}")]
    Thrown(AplRef<APLValue>, AplRef<APLValue>),
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
