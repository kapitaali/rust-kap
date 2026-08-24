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

pub mod number;
pub mod array;
pub mod token;
pub mod lexer;
pub mod lex_helpers;
pub mod ast;
pub mod parser;
pub mod evaluator;
pub mod encoder;
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
    /// A first-class **symbol** value (Kap `APLSymbol` wrapping a `Symbol`).
    /// Created by the `'foo` literal and `int:intern`; read by `int:symbolName`.
    /// `namespace` is `None` for the default namespace, `Some("keyword")` for the
    /// `:foo` keyword form. Distinct from `Str` (symbols are interned, comparable
    /// by name+namespace, and render as `ns:name`).
    Symbol {
        name: String,
        namespace: Option<String>,
    },
}

impl APLValue {
    pub fn is_null(&self) -> bool {
        matches!(self, APLValue::Null)
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
            APLValue::Str(_) => "string",
            APLValue::Array(_) => "array",
            APLValue::Null => "null",
            APLValue::Deferred { .. } => "deferred",
            APLValue::UserFn { .. } => "lambda",
            APLValue::UserOp { .. } => "operator",
            APLValue::Symbol { .. } => "symbol",
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
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
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
            APLValue::Array(a) => {
                a.elements().iter().map(|e| e.format_plain()).collect()
            }
            APLValue::Deferred { .. } => "<deferred>".to_string(),
            APLValue::UserFn { .. } => "<function>".to_string(),
            APLValue::UserOp { .. } => "<operator>".to_string(),
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
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
            APLValue::UserOp { .. } => "<operator>".to_string(),
            APLValue::Symbol { name, namespace } => match namespace {
                Some(ns) if ns == "keyword" => format!(":{}", name),
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            },
        }
    }

    // --- Array-shape accessors ---
    // A `Str` is a rank-1 array (vector of its chars) in Kap, exactly matching
    // Kotlin `APLBmpString.dimensions = dimensionsOfSize(content.length)`. These
    // accessors let array primitives (rho/tally/reverse/transpose) treat `Str`
    // uniformly with `Array` without restructuring the value (which would touch
    // every `Str(...)` construction site). Scalars (Number/Char/Null) are rank 0.

    /// Dimensions of the value. `Str` => `[len]`; scalars => `[]`; arrays => their dims.
    pub fn dimensions(&self) -> Vec<usize> {
        match self {
            APLValue::Array(a) => a.dimensions.clone(),
            APLValue::Str(s) => vec![s.chars().count()],
            _ => vec![],
        }
    }

    /// Rank of the value (`dimensions().len()`).
    pub fn rank(&self) -> usize {
        match self {
            APLValue::Array(a) => a.dimensions.len(),
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
            APLValue::Str(s) => s.chars().count(),
            _ => 1,
        }
    }

    /// Element at flat index `i`. For `Str`, returns the i-th character as a `Char`.
    pub fn value_at(&self, i: usize) -> APLValue {
        match self {
            APLValue::Array(a) => a.elements().get(i).map(|e| e.as_ref().clone()).unwrap_or(APLValue::Null),
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
            APLValue::Str(s) => s.chars().map(|c| Rc::new(APLValue::Char(c))).collect(),
            APLValue::Null => vec![],
            other => vec![Rc::new(other.clone())],
        }
    }

    /// Cross-kind total-order comparison, mirroring Kotlin `compareTotalOrdering`.
    /// Two numeric values (any mix of Long/BigInt/Rational/Double/Complex) compare
    /// numerically; two chars compare by codepoint; two arrays compare recursively;
    /// otherwise distinct types order by Kap's `typeSortOrder`
    /// (number < char < array < null). Complex numbers are not orderable → `None`.
    /// Used by the dyadic `⍸` (interval) form which searches boundaries across kinds.
    pub fn total_cmp(&self, other: &APLValue) -> Option<Ordering> {
        use APLValue::*;
        // Number vs Number → numeric.
        if let (Number(a), Number(b)) = (self, other) {
            return a.numeric_cmp(b).ok();
        }
        // Char vs Char → codepoint.
        if let (Char(a), Char(b)) = (self, other) {
            return Some(a.cmp(b));
        }
        // Array vs Array → recursive flat element comparison (first mismatch wins;
        // shorter array is "less" at the first missing index).
        if let (Array(a), Array(b)) = (self, other) {
            let ea = a.elements();
            let eb = b.elements();
            let n = ea.len().min(eb.len());
            for i in 0..n {
                if let Some(o) = ea[i].total_cmp(&eb[i]) {
                    if o != Ordering::Equal {
                        return Some(o);
                    }
                }
            }
            return Some(ea.len().cmp(&eb.len()));
        }
        // Distinct types → by Kap type sort order.
        let pos = |v: &APLValue| -> Option<usize> {
            match v {
                Number(_) => Some(0),
                Char(_) => Some(5),
                Array(_) => Some(7),
                Null => Some(11),
                _ => None,
            }
        };
        let pa = pos(self)?;
        let pb = pos(other)?;
        Some(pa.cmp(&pb))
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
}

/// Lexical environment. Symbols live behind a `RefCell` so assignment can mutate the
/// shared `Rc<Environment>` in place (single-threaded, per D1). `parent` enables lexical
/// scoping: a lookup walks outward until it finds the name. `ns_registry` is the
/// module-level namespace table, shared (via `Rc`) across all scopes.
#[derive(Debug, Default, Clone)]
pub struct Environment {
    /// Lexical (block-scope) bindings: key = (name, namespace). Values are shared refs.
    /// Holds dfn params (`⍵`/`⍺`), block locals, and operator operands — NOT module symbols.
    pub symbols: RefCell<HashMap<(String, Option<String>), AplRef<APLValue>>>,
    /// Parent scope for lexical lookup.
    pub parent: Option<AplRef<Environment>>,
    /// Shared namespace registry (module-level symbol table + import/export metadata).
    pub ns_registry: Rc<NamespaceRegistry>,
}

impl Environment {
    /// Build a child lexical scope that inherits the same shared namespace registry.
    pub fn child(parent: &Rc<Environment>) -> Rc<Environment> {
        Rc::new(Environment {
            symbols: RefCell::new(HashMap::new()),
            parent: Some(parent.clone()),
            ns_registry: parent.ns_registry.clone(),
        })
    }

    /// Build a fresh ROOT environment whose namespace registry is pre-seeded with the
    /// native quad constants (`⎕A`/`⎕a`/`⎕d`). B5 / code_analysis_03 / ROADMAP P6:
    /// Real Kap registers these as engine-level read-only constants — a FRESH session
    /// (no stdlib) already has `⎕A → "ABCDEFGHIJKLMNOPQRSTUVWXYZ"` and
    /// `⎕A ← 5` errors "Assignment to constant variable: kap:⎕A" (oracle-verified).
    pub fn new_root() -> Rc<Environment> {
        let env = Rc::new(Environment::default());
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
        }
        env
    }

    /// Is this the root (module-level) scope? Root scopes hold module bindings; child scopes
    /// hold lexical (block-local) bindings.
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
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
    /// Control-flow return signal raised by the `→` (branch/return) primitive. It is
    /// caught by the enclosing user-function frame, which returns the wrapped value.
    /// If it escapes to the top level (no enclosing function), the evaluator converts
    /// it to a "Call to return without a function call" runtime error.
    #[error("return: {0:?}")]
    Return(AplRef<APLValue>),
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
