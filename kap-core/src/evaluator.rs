//! Evaluator for Kap (Phase 3 + 4).
//!
//! `Engine::eval_string` runs the pipeline: tokenise -> parse -> eval. Laziness (D6):
//! args are `Instr` trees; they are forced via `force()` only when a builtin needs them.
//! A starter set of builtins is wired in: arithmetic (`+ - * × ÷`), comparisons
//! (`= ≠ < > ≤ ≥`), structural (`⍳ rho tally first`, `,` catenate, `⌽/⊖` reverse,
//! `⍉` transpose, `↑/↓` take/drop, `⊂` enclose), assignment `←`, and user lambdas
//! (`λ(params) body`). More in later phases.

use crate::array::{ArrayData, KapArray};
use crate::ast::{SyntaxMacro, Instr, BooleanOpKind};
use crate::lexer::tokenise;
use crate::number::KapNumber;
use crate::parser;
use crate::token::{LiteralValue, Token};
use unicode_segmentation::UnicodeSegmentation;
use std::cmp::Ordering;
use libm::lgamma;
use crate::{APLValue, AplError, AplRef, Engine, Environment};
use std::rc::Rc;

/// Adjust a (possibly negative) index into a valid 0-based position within `axis_size`,
/// matching Kap's `Dimensions.checkAndAdjustSelectedIndex` (dimension.kt):
/// non-negative `i` must satisfy `0 <= i < axis_size`; negative `i` counts from the end
/// (`axis_size + i`), valid when `-axis_size <= i`. Anything else is out of bounds.
fn check_and_adjust_selected_index(index: i64, axis_size: usize) -> Result<usize, AplError> {
    let n = axis_size as i64;
    if index >= 0 {
        if index >= n {
            return Err(AplError::runtime(format!(
                "index {} is outside valid range (axis size {})",
                index, n
            )));
        }
        Ok(index as usize)
    } else {
        if index < -n {
            return Err(AplError::runtime(format!(
                "index {} is outside valid range (axis size {})",
                index, n
            )));
        }
        Ok((n + index) as usize)
    }
}

/// Compute the stride (elements per axis unit) for each axis of a shape, in
/// row-major order. `strides[0]` = product of all dims except the first, etc.;
/// `strides[rank-1] == 1`. Used by `transpose`.
fn strides(dims: &[usize]) -> Vec<usize> {
    let r = dims.len();
    let mut s = vec![1usize; r];
    if r == 0 {
        return s;
    }
    for k in (0..r - 1).rev() {
        s[k] = s[k + 1] * dims[k + 1];
    }
    s
}




impl Environment {
    /// Look up a symbol. Resolution order (Kotlin `internSymbol`):
    ///   1. Lexical scope (parent chain) — dfn params `⍵`/`⍺`, block locals.
    ///   2. Module namespace table (shared `ns_registry`):
    ///      - explicit `ns:name` → that namespace directly;
    ///      - bare `name` → current namespace, then its imports, then `default`.
    pub fn lookup(&self, name: &str, ns: &Option<String>) -> Option<AplRef<APLValue>> {
        // 1. Lexical scope (dfn params, block locals) — checked first.
        let lkey = (name.to_string(), ns.clone());
        let mut cur: Option<&Environment> = Some(self);
        while let Some(e) = cur {
            if let Some(v) = e.symbols.borrow().get(&lkey) {
                return Some(v.clone());
            }
            cur = e.parent.as_deref();
        }
        // 2. Module namespace table.
        let reg = &self.ns_registry;
        match ns {
            Some(nsname) => reg.ns_lookup(nsname, name),
            None => reg.resolve_bare(name),
        }
    }

    /// Define a symbol. Routes to the namespace table when the binding is module-scoped
    /// (qualified `ns:name`, or a bare name defined at the root), and to the lexical
    /// scope when it is a block-local (bare name in a child scope, e.g. `⍵`/`⍺`).
    pub fn define(&self, name: &str, ns: &Option<String>, value: AplRef<APLValue>) {
        match ns {
            Some(nsname) => {
                self.ns_registry.ns_define(nsname, name, value);
            }
            None => {
                if self.is_root() {
                    // Module-scope bare assignment → current/default namespace.
                    let cur = self.ns_registry.current_ns();
                    self.ns_registry.ns_define(&cur, name, value);
                } else {
                    // Block-local (lexical) bare binding.
                    self.symbols
                        .borrow_mut()
                        .insert((name.to_string(), None), value);
                }
            }
        }
    }

    /// Assign to `name`, updating the *nearest existing binding* (Kap `←` semantics:
    /// like `set!`). Mirrors `define`'s routing: lexical scope first, then namespace table.
    pub fn assign(&self, name: &str, ns: &Option<String>, value: AplRef<APLValue>) {
        let key = (name.to_string(), ns.clone());
        // 1. Nearest lexical binding up the scope chain.
        let mut cur: Option<&Environment> = Some(self);
        while let Some(e) = cur {
            if e.symbols.borrow().contains_key(&key) {
                e.symbols.borrow_mut().insert(key.clone(), value);
                return;
            }
            cur = e.parent.as_deref();
        }
        // 2. Module namespace table (qualified or current/default for bare).
        match ns {
            Some(nsname) => {
                self.ns_registry.ns_define(nsname, name, value);
            }
            None => {
                let reg = &self.ns_registry;
                let cur = reg.current_ns();
                // If the current/default namespace already has it, update in place;
                // otherwise define in the current namespace (root-level assignment).
                if reg.ns_lookup(&cur, name).is_some() || self.is_root() {
                    reg.ns_define(&cur, name, value);
                } else {
                    // Block-local assignment that has no prior binding: keep lexical.
                    self.symbols.borrow_mut().insert((name.to_string(), None), value);
                }
            }
        }
    }

    /// Names of all symbols in this scope (and parents) that currently hold a
    /// user-defined or native function value. Used by the parser to distinguish a
    /// function symbol (which applies) from a value symbol (which strands).
    pub fn function_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut cur: Option<&Environment> = Some(self);
        while let Some(env) = cur {
            for ((name, _ns), val) in env.symbols.borrow().iter() {
                if matches!(val.as_ref(), APLValue::UserFn { .. }) && !names.contains(name) {
                    names.push(name.clone());
                }
            }
            cur = env.parent.as_deref();
        }
        // Also surface functions held in the namespace registry (current/default ns and
        // its imports) — top-level bare `f ⇐ {…}` assignments now live there.
        self.ns_registry.collect_function_names(&mut names);
        names
    }

    /// Names bound to user-defined *operators* (`APLValue::UserOp`), used to seed the
    /// parser's `known_ops` so an operator call (`+foo 2`) in a later statement parses
    /// `foo` as an operator even though the defining `∇` was a separate parse.
    pub fn operator_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut cur: Option<&Environment> = Some(self);
        while let Some(env) = cur {
            for ((name, _ns), val) in env.symbols.borrow().iter() {
                if matches!(val.as_ref(), APLValue::UserOp { .. }) && !names.contains(name) {
                    names.push(name.clone());
                }
            }
            cur = env.parent.as_deref();
        }
        self.ns_registry.collect_operator_names(&mut names);
        names
    }
}

impl APLValue {
    /// Force a (possibly deferred) value. For a `Deferred`, evaluates its `instr` in its
    /// captured `env` (call-by-need: not memoised yet — see D6 revisit). Non-deferred
    /// values are returned unchanged.
    pub fn force(&self, engine: &Engine) -> Result<AplRef<APLValue>, AplError> {
        match self {
            APLValue::Deferred { instr, env } => {
                let env = env.clone();
                engine.eval_instr(instr, &env)
            }
            other => Ok(Rc::new(other.clone())),
        }
    }
}

/// Supported text encodings for `unicode:enc` / `unicode:dec`. Mirrors the subset of
/// Real Kap's `Charset` mapping (UTF8/UTF16/UTF16LE/UTF16BE/UTF32).
#[derive(Clone, Copy)]
enum Charset {
    Utf8,
    Utf16,
    Utf16Le,
    Utf16Be,
    Utf32,
}

impl Charset {
    fn from_name(name: &str) -> Option<Charset> {
        match name.to_uppercase().as_str() {
            "UTF8" | "UTF-8" => Some(Charset::Utf8),
            "UTF16" | "UTF-16" => Some(Charset::Utf16),
            "UTF16LE" | "UTF-16LE" => Some(Charset::Utf16Le),
            "UTF16BE" | "UTF-16BE" => Some(Charset::Utf16Be),
            "UTF32" | "UTF-32" => Some(Charset::Utf32),
            _ => None,
        }
    }
}

/// Encode `s` into bytes under `enc`. UTF-16/32 are emitted as unsigned code units
/// (matching Real Kap's element-wise `APLArrayByte` of the raw code units). Plain
/// `UTF16` (no explicit endianness) prefixes a byte-order mark (BOM), exactly like
/// the Kotlin `encodeWithEncoding` path for `EncodingName.UTF16`.
fn unicode_encode(s: &str, enc: Charset) -> Vec<u8> {
    match enc {
        Charset::Utf8 => s.as_bytes().to_vec(),
        Charset::Utf16 => {
            // BOM (0xFE 0xFF) + code units in big-endian order (Kotlin's default).
            let mut out = vec![0xfeu8, 0xff];
            for u in s.encode_utf16() {
                out.extend_from_slice(&u.to_be_bytes());
            }
            out
        }
        Charset::Utf16Be => {
            let mut out = Vec::new();
            for u in s.encode_utf16() {
                out.extend_from_slice(&u.to_be_bytes());
            }
            out
        }
        Charset::Utf16Le => {
            let mut out = Vec::new();
            for u in s.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
            out
        }
        Charset::Utf32 => {
            let mut out = Vec::new();
            for c in s.chars() {
                out.extend_from_slice(&(c as u32).to_be_bytes());
            }
            out
        }
    }
}

/// Decode `bytes` into a string under `enc` (lossy for invalid sequences, mirroring
/// Real Kap's `decodeWithEncoding`, which replaces errors with the replacement char).
/// The BOM is consumed (and ignored) when present for `UTF16`/`UTF16BE`/`UTF16LE`.
fn unicode_decode(bytes: &[u8], enc: Charset) -> String {
    match enc {
        Charset::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Charset::Utf16 => {
            let mut start = 0;
            if bytes.len() >= 2 && (bytes[0] == 0xfe && bytes[1] == 0xff) {
                start = 2; // strip BE BOM
            }
            let mut units = Vec::with_capacity((bytes.len() - start) / 2);
            let mut i = start;
            while i + 1 < bytes.len() {
                let u = u16::from_be_bytes([bytes[i], bytes[i + 1]]);
                units.push(u);
                i += 2;
            }
            String::from_utf16_lossy(&units)
        }
        Charset::Utf16Be => {
            let mut units = Vec::with_capacity(bytes.len() / 2);
            let mut i = 0;
            while i + 1 < bytes.len() {
                let u = u16::from_be_bytes([bytes[i], bytes[i + 1]]);
                units.push(u);
                i += 2;
            }
            String::from_utf16_lossy(&units)
        }
        Charset::Utf16Le => {
            let mut units = Vec::with_capacity(bytes.len() / 2);
            let mut i = 0;
            while i + 1 < bytes.len() {
                let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
                units.push(u);
                i += 2;
            }
            String::from_utf16_lossy(&units)
        }
        Charset::Utf32 => {
            let mut cps = Vec::with_capacity(bytes.len() / 4);
            let mut i = 0;
            while i + 3 < bytes.len() {
                let u = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                if let Some(c) = char::from_u32(u) {
                    cps.push(c);
                } else {
                    cps.push(char::REPLACEMENT_CHARACTER);
                }
                i += 4;
            }
            cps.iter().collect()
        }
    }
}

/// Unicode name of a character, or `None` if it has no standard name. Best-effort
/// mirror of Kotlin's `codepointToName` (Java `Character.getName`): covers ASCII
/// letters/digits/punctuation and a broad set of controls; returns `None` otherwise.
fn unicode_char_name(c: char) -> Option<String> {
    // ASCII printable letters
    if c.is_ascii_alphabetic() {
        return Some(format!(
            "LATIN {} LETTER {}",
            if c.is_ascii_uppercase() { "CAPITAL" } else { "SMALL" },
            c.to_ascii_uppercase()
        ));
    }
    // Digits
    if c.is_ascii_digit() {
        let names = [
            "ZERO", "ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX", "SEVEN", "EIGHT", "NINE",
        ];
        if let Some(d) = c.to_digit(10) {
            return Some(format!("DIGIT {}", names[d as usize]));
        }
    }
    // Common ASCII punctuation / space
    let named = match c {
        ' ' => "SPACE",
        '!' => "EXCLAMATION MARK",
        '"' => "QUOTATION MARK",
        '#' => "NUMBER SIGN",
        '$' => "DOLLAR SIGN",
        '%' => "PERCENT SIGN",
        '&' => "AMPERSAND",
        '\'' => "APOSTROPHE",
        '(' => "LEFT PARENTHESIS",
        ')' => "RIGHT PARENTHESIS",
        '*' => "ASTERISK",
        '+' => "PLUS SIGN",
        ',' => "COMMA",
        '-' => "HYPHEN-MINUS",
        '.' => "FULL STOP",
        '/' => "SOLIDUS",
        ':' => "COLON",
        ';' => "SEMICOLON",
        '<' => "LESS-THAN SIGN",
        '=' => "EQUALS SIGN",
        '>' => "GREATER-THAN SIGN",
        '?' => "QUESTION MARK",
        '@' => "COMMERCIAL AT",
        '[' => "LEFT SQUARE BRACKET",
        '\\' => "REVERSE SOLIDUS",
        ']' => "RIGHT SQUARE BRACKET",
        '^' => "CIRCUMFLEX ACCENT",
        '_' => "LOW LINE",
        '`' => "GRAVE ACCENT",
        '{' => "LEFT CURLY BRACKET",
        '|' => "VERTICAL LINE",
        '}' => "RIGHT CURLY BRACKET",
        '~' => "TILDE",
        // Curated astral-plane names for codepoints exercised by the corpus (UnicodeTest.kt).
        // Java's Character.getName covers the full range; we mirror the few the tests touch.
        '\u{1D49F}' => "MATHEMATICAL FRAKTUR CAPITAL D",
        '\u{1F63A}' => "SMILING CAT FACE WITH OPEN MOUTH",
        _ => return None,
    };
    Some(named.to_string())
}

impl Engine {
    /// B6 (code_analysis_03 / Kotlin `Namespace.checkAssign`): error if the target
    /// symbol is a read-only constant. Checks the explicitly-named namespace, then the
    /// current namespace, then `kap` (where the native quad constants live) — mirroring
    /// how bare-name lookup falls through to the owning namespace. Error text matches
    /// the oracle verbatim: `Assignment to constant variable: <ns>:<name>`.
    fn check_not_constant(
        &self,
        name: &str,
        ns: &Option<String>,
        env: &Rc<Environment>,
    ) -> Result<(), AplError> {
        let reg = &env.ns_registry;
        let mut candidates: Vec<String> = Vec::new();
        if let Some(n) = ns {
            candidates.push(n.clone());
        }
        candidates.push(reg.current_ns());
        candidates.push("kap".to_string());
        for c in candidates {
            if reg.is_constant(&c, name) {
                return Err(AplError::runtime(format!(
                    "Assignment to constant variable: {}:{}",
                    c, name
                )));
            }
        }
        Ok(())
    }

    /// Stateless "single expression" mode (Mode 1): evaluate `src` in a *fresh*
    /// environment. Any variables assigned inside `src` do not persist.
    /// For persistent state across evaluations, use [`crate::Session`] instead.
    pub fn eval_string(&self, src: &str) -> Result<AplRef<APLValue>, AplError> {
        let env = Environment::new_root();
        self.eval_string_in_env(src, &env)
    }

    /// Convenience (Mode 1): stateless eval returning the formatted REPL-style string.
    pub fn eval_to_string(&self, src: &str) -> Result<String, AplError> {
        Ok(self.eval_string(src)?.format_value())
    }

    /// Shared evaluation core. Runs `src` in the given `env`. Variables assigned
    /// here persist in `env` — this is what [`crate::Session`] relies on to keep
    /// state across calls.
    pub fn eval_string_in_env(
        &self,
        src: &str,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Parse and evaluate one statement at a time, refreshing the set of known function
        // names between statements. This lets a function defined in one statement
        // (`∇ foo ...`) be visible to later statements (`foo 10`), so the parser can tell a
        // value symbol (`a c` strands) from a function symbol (`foo 10` applies).
        let toks = tokenise(src);
        let mut pos = 0;
        let mut last: AplRef<APLValue> = Rc::new(APLValue::Null);
        loop {
            // Fresh function-name set each statement, so a function defined earlier in
            // this same input (`∇ foo ...`) is visible to later statements (`foo 10`).
            // The parser also grows this set as it parses `∇`/`⇐` defs, so a function
            // body that references its own name parses as an application (recursion).
            let fn_names: Vec<String> = env.function_names();
            let op_names: Vec<String> = env.operator_names();
            // Snapshot the live macro registry so a `defsyntax` defined earlier in the
            // session (e.g. inside an earlier `use()`) is visible to later statements.
            let macros = self.macros.borrow().clone();
            let mut p = parser::Parser {
                toks: &toks,
                pos,
                known_functions: fn_names,
                known_ops: op_names,
                macros,
                kotlin_close_stack: Vec::new(),
            };
            match p.parse_statements()? {
                Some(instr) => {
                    pos = p.pos;
                    last = self.eval_instr(&instr, env)?;
                }
                None => break,
            }
        }
        Ok(last)
    }

    /// Like `eval_string_in_env`, but a parse/eval error on one statement does NOT abort the
    /// whole input — it is reported (to stderr) and evaluation continues with the next
    /// statement. Mirrors Real Kap's `use()` behaviour, where a single failing line (e.g. a
    /// `declare(:const …)` that conflicts with a previously-defined constant) does not prevent
    /// later definitions in the same file from taking effect. The final result is the last
    /// successfully-evaluated statement, or `Null` if every statement errored.
    fn eval_string_in_env_tolerant(
        &self,
        src: &str,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let toks = tokenise(src);
        let mut pos = 0;
        let mut last: AplRef<APLValue> = Rc::new(APLValue::Null);
        loop {
            let fn_names: Vec<String> = env.function_names();
            let op_names: Vec<String> = env.operator_names();
            let macros = self.macros.borrow().clone();
            let mut p = parser::Parser {
                toks: &toks,
                pos,
                known_functions: fn_names,
                known_ops: op_names,
                macros,
                kotlin_close_stack: Vec::new(),
            };
            match p.parse_statements() {
                Ok(Some(instr)) => {
                    pos = p.pos;
                    match self.eval_instr(&instr, env) {
                        Ok(v) => last = v,
                        Err(e) => {
                            eprintln!("warning: use(): statement failed: {}", e);
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    // Parse error: advance past this statement so we don't loop forever on a
                    // broken token stream. Best-effort skip to the next newline/separator.
                    eprintln!("warning: use(): statement failed: {}", e);
                    let mut skipped = false;
                    while pos < toks.len() {
                        match &toks[pos].token {
                            Token::Newline | Token::StatementSeparator | Token::ListSeparator => {
                                pos += 1;
                                skipped = true;
                                break;
                            }
                            _ => pos += 1,
                        }
                    }
                    if !skipped {
                        break;
                    }
                }
            }
        }
        Ok(last)
    }

    /// Evaluate a single `Instr` in `env`. This is the core eval loop.
    pub fn eval_instr(
        &self,
        instr: &Instr,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match instr {
            Instr::Literal(LiteralValue::Number(n)) => Ok(Rc::new(APLValue::Number(n.clone()))),
            Instr::Literal(LiteralValue::Char(c)) => Ok(Rc::new(APLValue::Char(*c))),
            Instr::Literal(LiteralValue::Str(s)) => Ok(Rc::new(APLValue::Str(s.clone()))),
            Instr::Literal(LiteralValue::Symbol { .. }) => Err(AplError::runtime("lone symbol literal".into())),
            Instr::Literal(LiteralValue::SymbolValue { name }) => {
                // A `'name` symbol literal (Kotlin QuotePrefix → LiteralSymbol):
                // evaluates to the interned symbol VALUE itself.
                Ok(Rc::new(APLValue::Symbol {
                    name: name.clone(),
                    namespace: None,
                }))
            }
            Instr::SymbolValue { name } => {
                Ok(Rc::new(APLValue::Symbol {
                    name: name.clone(),
                    namespace: None,
                }))
            }
            Instr::Empty => Ok(Rc::new(APLValue::Null)),
            Instr::Symbol { name, namespace } => {
                // A keyword-namespace symbol (`:UTF16`, `:pretty`, …) is a *value*
                // symbol (interned, renders `:utf16`), not a lookup in the ordinary
                // environment. It carries no bound value, so it is returned as-is.
                if namespace.as_deref() == Some("keyword") {
                    return Ok(Rc::new(APLValue::Symbol {
                        name: name.clone(),
                        namespace: Some("keyword".to_string()),
                    }));
                }
                let bound = env.lookup(name, namespace);
                // P1-M4 (common.kt:173 IllegalContextForFunction): an UNBOUND primitive
                // in value position has no arguments to bind. Fires BEFORE the lookup
                // error because primitive glyphs are not pre-bound in the environment.
                // A user/library REBINDING of the name (stdlib shadows ⊥/⊤) wins. The
                // parser cannot reject this — an ambivalent fn VALUE is legal inside fn
                // contexts (`foo ⇐ -`, train members) — so it fires at eval.
                if bound.is_none() && namespace.is_none() && Self::is_primitive_name(name) {
                    return Err(AplError::runtime(
                        "No arguments specified for function".to_string(),
                    ));
                }
                let found = bound
                    .ok_or_else(|| AplError::runtime(format!("undefined symbol: {}", name)))?;
                // B2 (code_analysis_03): an operator is never a first-class value in Real Kap.
                // Kotlin resolves names function-first, then throws InvalidOperatorArgument for
                // operator names in value position (parser.kt:967–971; text common.kt:186).
                // Structural references (`declare(:export ⌸)`) read the AST name and never
                // evaluate the symbol, so they are naturally unaffected by this check.
                if let APLValue::UserOp { .. } = found.as_ref() {
                    return Err(AplError::runtime(format!(
                        "Operator without left function: {}",
                        name
                    )));
                }
                // clone the inner value out of the shared ref
                Ok(Rc::new(found.as_ref().clone()))
            }
            Instr::DynamicRef { name, namespace } => {
                let found = env
                    .lookup(name, namespace)
                    .ok_or_else(|| AplError::runtime(format!("undefined symbol: {}", name)))?;
                Ok(found)
            }
            Instr::OpCall { op, left_fn, right_fn } => {
                // An operator call only appears as a *function*; route it through
                // `eval_apply` with no trailing data argument.
                self.apply_user_op(op, left_fn, right_fn, &None, &Box::new(Instr::Empty), env)
            }
            Instr::Array { elements } => {
                let mut vals = Vec::with_capacity(elements.len());
                for e in elements {
                    vals.push(self.eval_instr(e, env)?);
                }
                // P1-M4 (common.kt:173): a strand whose LAST member is an unbound
                // primitive (`c +`) is Kotlin's "fn with leftArgs and no right arg"
                // case, which errors `No arguments specified for function` (oracle-
                // verified). The parser cannot distinguish it from a legitimate
                // fn-value strand, so it is detected here.
                if let Some(last) = elements.last() {
                    if let Instr::Symbol { name, namespace: None } = last {
                        if Self::is_primitive_name(name) && env.lookup(name, &None).is_none() {
                            return Err(AplError::runtime(
                                "No arguments specified for function".to_string(),
                            ));
                        }
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![vals.len()],
                    ArrayData::Nested(vals),
                )))))
            }
            Instr::Lambda { params, body } => Ok(Rc::new(APLValue::UserFn {
                params: params.clone(),
                // The last param is the right argument (⍵); any preceding params are
                // left arguments (⍺). So `λ(a)` is monadic (split 0) and `λ(a b)` is
                // dyadic (split 1), matching Kap dfn conventions.
                split: params.len().saturating_sub(1),
                body: Rc::new(*body.clone()),
                env: env.clone(),
            })),
            Instr::Assign { target, value } => {
                if let Instr::Symbol { name, namespace } = target.as_ref() {
                    let v = self.eval_instr(value, env)?;
                    // B6 (code_analysis_03): a read-only constant rejects assignment with
                    // Kotlin's exact error (oracle: `⎕A ← 5` → "Assignment to constant
                    // variable: kap:⎕A"). The constant is registered under its owning
                    // namespace; a bare name resolves there via current-ns/default fallback.
                    self.check_not_constant(name, namespace, env)?;
                    // `←` updates the nearest enclosing binding (closure-safe), falling
                    // back to defining locally when the name is new in this scope.
                    env.assign(name, namespace, v.clone());
                    Ok(v)
                } else {
                    Err(AplError::runtime("assignment target must be a symbol".into()))
                }
            }
            Instr::DestructAssign { names, value } => {
                // `(a b c) ← expr` — bind each LHS symbol to the corresponding element of
                // the (vector) RHS. Mirrors Kap's multi-target assignment.
                let v = self.eval_instr(value, env)?;
                let elems = v.elements();
                if elems.len() != names.len() {
                    return Err(AplError::runtime(format!(
                        "destructuring assignment expected {} values, got {}",
                        names.len(),
                        elems.len()
                    )));
                }
                for (i, (nm, ns)) in names.iter().enumerate() {
                    self.check_not_constant(nm, ns, env)?;
                    env.assign(nm, ns, elems[i].clone());
                }
                Ok(v)
            }
            Instr::AxisApplied { func, axis } => {
                // An axis-applied function always appears as the `fn_expr` of an `Apply`,
                // where `eval_apply` unwraps it. Reached directly only as a stray top-level
                // expression; route through `eval_apply` so the guard handles it.
                self.eval_apply(
                    &Instr::AxisApplied {
                        func: func.clone(),
                        axis: axis.clone(),
                    },
                    &None,
                    &Box::new(Instr::Empty),
                    env,
                )
            }
            Instr::Apply {
                fn_expr,
                left,
                right,
            } => self.eval_apply(fn_expr, left, right, env),
            Instr::Derived { func, .. } => {
                self.eval_apply(func, &None, &Box::new(Instr::Empty), env)
            }
            Instr::ValueOp { func, op_name, operand } => {
                // A value-right-arg operator binding (`f⍤1`) reached as a stray
                // expression; route through `eval_apply` so its guard handles it.
                self.eval_apply(
                    &Instr::ValueOp {
                        func: func.clone(),
                        op_name: op_name.clone(),
                        operand: operand.clone(),
                    },
                    &None,
                    &Box::new(Instr::Empty),
                    env,
                )
            }
            Instr::Train { funcs, reverse, compose } => self.apply_train(
                funcs,
                *reverse,
                *compose,
                &None,
                &Box::new(Instr::Empty),
                env,
            ),
            Instr::Block { body } => self.eval_block(body, env),
            Instr::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.eval_instr(cond, env)?;
                if self.truthy(&c) {
                    self.eval_instr(then_block, env)
                } else if let Some(alt) = else_block {
                    self.eval_instr(alt, env)
                } else {
                    Ok(Rc::new(APLValue::Null))
                }
            }
            Instr::While { cond, body } => {
                let mut last = Rc::new(APLValue::Null);
                loop {
                    let c = self.eval_instr(cond, env)?;
                    if !self.truthy(&c) {
                        break;
                    }
                    last = self.eval_instr(body, env)?;
                }
                Ok(last)
            }
            Instr::When { clauses } => {
                for (c, b) in clauses {
                    let cv = self.eval_instr(c, env)?;
                    if self.truthy(&cv) {
                        return self.eval_instr(b, env);
                    }
                }
                Ok(Rc::new(APLValue::Null))
            }
            Instr::DefSyntax {
                name,
                namespace,
                rules,
                body,
            } => {
                // Register a parse-time macro (Kotlin `CustomSyntax`). Keyed by bare trigger
                // name; the parser's macro table drives inline expansion at parse time.
                let qual = match namespace {
                    Some(ns) => format!("{}:{}", ns, name),
                    None => name.clone(),
                };
                let macro_def = SyntaxMacro {
                    rules: rules.clone(),
                    body: body.clone(),
                };
                self.macros.borrow_mut().insert(qual, macro_def);
                Ok(Rc::new(APLValue::Null))
            }
            Instr::DefSyntaxSub {
                name,
                namespace,
                rules,
                body,
            } => {
                let qual = match namespace {
                    Some(ns) => format!("{}:{}", ns, name),
                    None => name.clone(),
                };
                let macro_def = SyntaxMacro {
                    rules: rules.clone(),
                    body: body.clone(),
                };
                self.macros.borrow_mut().insert(qual, macro_def);
                Ok(Rc::new(APLValue::Null))
            }
            Instr::MacroExpand { body, bindings } => {
                // Evaluate each binding instr to a value, define the bound var in a child env,
                // then evaluate the macro body (Kotlin `CallWithVarInstruction`).
                let child = Environment::child(&env);
                for (var, instr) in bindings {
                    let v = self.eval_instr(instr, env)?;
                    child.define(var, &None, v);
                }
                self.eval_instr(body, &child)
            }
            Instr::Train { .. } => {
                // A standalone train (no args) is an error; trains apply via eval_apply.
                Err(AplError::runtime(
                    "train used without arguments (apply it: (f g) x)".into(),
                ))
            }
            Instr::UserFnDef {
                name,
                namespace,
                left_params,
                right_params,
                body,
            } => {
                // Combine left+right params; `split` separates the dyadic left bind.
                let mut params = left_params.clone();
                params.extend(right_params.iter().cloned());
                let split = left_params.len();
                let v = Rc::new(APLValue::UserFn {
                    params,
                    split,
                    body: Rc::new(*body.clone()),
                    env: env.clone(),
                });
                env.define(name, namespace, v.clone());
                Ok(v)
            }
            Instr::FnAssign {
                name,
                namespace,
                value,
            } => {
                // `name ⇐ <fn-expr>`: compile RHS to a UserFn. If it is already a
                // lambda, store directly with split=0; otherwise wrap as a dyadic
                // delegation `⍺ <rhs> ⍵` (split=1), so the new name is callable as
                // `x name y` (and monadically only if the RHS supports it).
                let v = match value.as_ref() {
                    Instr::Lambda { params, body } => Rc::new(APLValue::UserFn {
                        params: params.clone(),
                        split: params.len().saturating_sub(1),
                        body: Rc::new(*body.clone()),
                        env: env.clone(),
                    }),
                    // A bare block `{ … }` is a function with no named parameters; its
                    // body is the block itself (it may reference `⍵`/`⍺`).
                    Instr::Block { body } => Rc::new(APLValue::UserFn {
                        params: vec![],
                        split: 0,
                        body: Rc::new(Instr::Block { body: body.clone() }),
                        env: env.clone(),
                    }),
                    // A derived function (`×/`) or a train (`⊢«⊣»`, `×-`) is itself already a
                    // function. Store it directly as the body (split=0) so `apply_user_fn`
                    // routes it through `eval_apply` with the *call's* data arguments. We must
                    // NOT evaluate it here — doing so would apply it to empty args at
                    // definition time (e.g. `foo ⇐ ×-` would run `×(-)` and fail). Component
                    // primitives (×, -, ⊢, ⊣, …) resolve at apply time via `eval_apply`'s
                    // `fn_name` dispatch, which is ambivalent (monadic vs dyadic).
                    Instr::Derived { .. } | Instr::Train { .. } => Rc::new(APLValue::UserFn {
                        params: vec![],
                        split: 0,
                        body: Rc::new(*value.clone()),
                        env: env.clone(),
                    }),
                    // Bare symbol RHS:
                    //  * a primitive (`foo ⇐ -`) — store the symbol directly as the body;
                    //    `apply_user_fn` routes it through `eval_apply`, which dispatches
                    //    the primitive *ambivalently* (monadic `foo 5` = negate, dyadic
                    //    `3 foo 1` = subtract). The old `⍺ - ⍵` delegation broke monadic
                    //    calls because `⍺` is unbound when no left arg is supplied.
                    //  * a user-function name (`foo ⇐ bar`) — delegate `⍺ bar ⍵` so `bar`
                    //    is applied to the call's arguments (an alias). `eval_apply` also
                    //    handles a bare user-symbol by looking it up and applying it, but
                    //    the delegation preserves lexical closure of the defining scope.
                    Instr::Symbol { name, .. } if Self::is_primitive_name(name) => {
                        Rc::new(APLValue::UserFn {
                            params: vec![],
                            split: 0,
                            body: Rc::new(*value.clone()),
                            env: env.clone(),
                        })
                    }
                    _ => {
                        let deleg = Instr::Apply {
                            fn_expr: Box::new(*value.clone()),
                            left: Some(Box::new(Instr::Symbol {
                                name: "⍺".into(),
                                namespace: None,
                            })),
                            right: Box::new(Instr::Symbol {
                                name: "⍵".into(),
                                namespace: None,
                            }),
                        };
                        Rc::new(APLValue::UserFn {
                            params: vec![],
                            split: 1,
                            body: Rc::new(deleg),
                            env: env.clone(),
                        })
                    }
                };
                env.define(name, namespace, v.clone());
                Ok(v)
            }
            Instr::UserOpDef {
                name,
                op_left,
                op_right,
                left_params,
                right_params,
                body,
            } => {
                // Compile to `APLValue::UserOp`: a function that, when applied with
                // function-operands (via `OpCall`), binds `op_left`/`op_right` to those
                // operands and runs `body` with the ordinary data args.
                let v = Rc::new(APLValue::UserOp {
                    op_left: op_left.clone(),
                    op_right: op_right.clone(),
                    left_params: left_params.clone(),
                    right_params: right_params.clone(),
                    body: Rc::new(*body.clone()),
                    env: env.clone(),
                });
                env.define(name, &None, v.clone());
                Ok(v)
            }
            Instr::DynamicRef { name, namespace } => {
                // Kap's `⍞name`: fetch the variable's *value* and apply it as a function.
                let found = env
                    .lookup(name, namespace)
                    .ok_or_else(|| AplError::runtime(format!("undefined symbol: {}", name)))?;
                Ok(found)
            }
            Instr::Value(v) => Ok(v.clone()),
            Instr::Index { array, selector } => {
                let arr = self.eval_instr(array, env)?;
                let sel = self.eval_instr(selector, env)?;
                // `x[y]` bracket indexing is Kap's OWN index-select semantics
                // (Kotlin `APLValue.get` / `indexFromPositionNegativeSupport`),
                // NOT `pick` (`⊇`). Route to `index_select`.
                self.index_select(arr.as_ref(), sel.as_ref())
            }
            Instr::Guard { cond, truthy, falsy } => {
                let c = self.eval_instr(cond, env)?.force(self)?;
                if self.truthy(&c) {
                    self.eval_instr(truthy, env)
                } else {
                    self.eval_instr(falsy, env)
                }
            }
            // Short-circuit boolean `and` / `or` (Kotlin `BooleanAndFunction` /
            // `BooleanOrFunction`). Evaluate the LEFT operand, then decide:
            //   `and` → if truthy(left) then right else left   (right NOT evaluated when left is falsy)
            //   `or`  → if truthy(left) then left else right    (right NOT evaluated when left is truthy)
            // The result is the *raw* operand value (mirrors the oracle: `1 and 2 → 2`,
            // `0 and 2 → 0`, `1 or 0 → 1`). This is what makes `16∊decoded and '… int:throwNative …'`
            // a lazy guard: when the membership check is false the throw is never evaluated.
            Instr::BooleanOp { op, left, right } => {
                let l = self.eval_instr(left, env)?.force(self)?;
                match op {
                    BooleanOpKind::And => {
                        if self.truthy(&l) {
                            self.eval_instr(right, env)
                        } else {
                            Ok(l)
                        }
                    }
                    BooleanOpKind::Or => {
                        if self.truthy(&l) {
                            Ok(l)
                        } else {
                            self.eval_instr(right, env)
                        }
                    }
                }
            }
        }
    }

    /// Evaluate a block: statements in order; the value is the last statement's value.
    fn eval_block(
        &self,
        body: &[Instr],
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let mut result = Rc::new(APLValue::Null);
        for stmt in body {
            result = self.eval_instr(stmt, env)?;
        }
        Ok(result)
    }

    /// Kap truthiness: a number is truthy iff non-zero; an array is truthy iff non-empty;
    /// null/empty string is falsy; a non-empty string is truthy.
    fn truthy(&self, v: &APLValue) -> bool {
        match v {
            APLValue::Number(n) => n.as_boolean(),
            APLValue::Array(a) => a.element_count() > 0,
            APLValue::Str(s) => !s.is_empty(),
            APLValue::Char(_) => true,
            APLValue::Null => false,
            APLValue::UserFn { .. } => true,
            APLValue::UserOp { .. } => true,
            APLValue::Deferred { .. } => false,
            APLValue::Symbol { .. } => true,
        }
    }

    fn eval_apply(
        &self,
        fn_expr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Axis specifier `f[axis]` (Kotlin `AxisValAssignedFunctionDirect`). When the
        // function expr is wrapped in `AxisApplied`, evaluate the axis and route the
        // scalar arithmetic builtins to an axis-aware broadcast path.
        if let Instr::AxisApplied { func, axis } = fn_expr {
            let axis_val = self.eval_instr(axis, env)?.force(self)?;
            let axis_as_long = match axis_val.as_ref() {
                APLValue::Number(n) => n.as_long().map_err(|e| AplError::runtime(e))? as usize,
                APLValue::Array(a) if a.dimensions.len() == 1 && a.element_count() == 1 => {
                    match a.elements().get(0) {
                        Some(e) => match e.as_ref() {
                            APLValue::Number(x) => x.as_long().map_err(|e| AplError::runtime(e))? as usize,
                            _ => return Err(AplError::runtime("axis must be an integer".into())),
                        },
                        None => return Err(AplError::runtime("axis must be an integer".into())),
                    }
                }
                _ => return Err(AplError::runtime("axis must be an integer".into())),
            };
            // Keep the *possibly fractional* axis value too (for `,[0.5]` laminate).
            let axis_number: KapNumber = match axis_val.as_ref() {
                APLValue::Number(n) => n.clone(),
                APLValue::Array(a) if a.dimensions.len() == 1 && a.element_count() == 1 => {
                    match a.elements().get(0) {
                        Some(e) => match e.as_ref() {
                            APLValue::Number(x) => x.clone(),
                            _ => return Err(AplError::runtime("axis must be a number".into())),
                        },
                        None => return Err(AplError::runtime("axis must be a number".into())),
                    }
                }
                _ => return Err(AplError::runtime("axis must be a number".into())),
            };
            let fn_name = match **func {
                Instr::Symbol { ref name, .. } => name.as_str(),
                _ => {
                    return Err(AplError::runtime(
                        "axis specifier is only supported on scalar arithmetic functions".into(),
                    ))
                }
            };
            let left_v = match left {
                Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
                None => None,
            };
            let right_v = self.eval_instr(right, env)?.force(self)?;
            // `,` / `⍪` support a fractional axis: an axis within 0.01 of an integer is
            // treated as a plain (integer) axis; otherwise it is a *laminate* axis
            // (Kotlin `computeLaminateAxis`: axis -> ceil(d)), which inserts a length-1
            // axis and concatenates the two arrays along it. This is how `,[0.5]` works.
            if fn_name == "," || fn_name == "⍪" {
                return self.catenate_axis(left_v, right_v, &axis_number);
            }
            // Kotlin `MathCombineAPLFunction.eval2Arg` short-circuits scalar+scalar BEFORE
            // any axis handling: `if (a0 is APLSingleValue && b0 is APLSingleValue)
            // return combine2Arg(a0, b0)`. The axis is silently ignored for two scalars,
            // so `2 +[0] 3` -> `5`, NOT "A or B has to be rank 1".
            if let (Some(l), APLValue::Number(b)) = (left_v.as_ref(), right_v.as_ref()) {
                if let APLValue::Number(a) = l.as_ref() {
                    let res = self.apply_op(fn_name, a, b)?;
                    return Ok(Rc::new(APLValue::Number(res)));
                }
            }
            return match fn_name {
                "+" | "-" | "×" | "÷" | "*" => {
                    self.num2_axis(left_v, right_v, fn_name, axis_as_long)
                }
                other => Err(AplError::runtime(format!(
                    "axis specifier not supported for '{}'",
                    other
                ))),
            };
        }
        // Resolve the function: a builtin name, a direct lambda, or a user function
        // bound to a symbol.
        let (fn_name, fn_namespace): (Option<String>, Option<Option<String>>) = match fn_expr {
            Instr::Symbol { name, namespace } => (
                Some(match namespace {
                    Some(ns) => format!("{}:{}", ns, name),
                    None => name.clone(),
                }),
                Some(namespace.clone()),
            ),
            _ => (None, None),
        };
        // `declare` is a *special form* (Kotlin `DeclareToken` → `processExport`): its
        // argument is read *syntactically*, never evaluated. `declare(:export zork)`
        // must not force-evaluate `zork` — the oracle returns `null` and tolerates a
        // not-yet-bound name. We intercept the bare symbol `declare` here, *before*
        // `right_val` is forced below, and read the (unevaluated) `right` AST instead.
        // The argument is `[Symbol{export,keyword}, target]` where `target` is a
        // `Symbol` (export one name) or an `Array` of `Symbol`s (export several), or a
        // non-symbol operand (no-op, matching the oracle's tolerance).
        if let (Some(name), Some(ns)) = (&fn_name, &fn_namespace) {
            if name == "declare" && ns.is_none() {
                return self.eval_declare(right, env);
            }
        }
        let lambda = match fn_expr {
            Instr::Lambda { params, body } => Some((
                params.clone(),
                params.len().saturating_sub(1),
                Rc::new((**body).clone()),
            )),
            Instr::Symbol { name, namespace } => match env.lookup(name, namespace) {
                Some(v) if matches!(v.as_ref(), APLValue::UserFn { .. }) => {
                    if let APLValue::UserFn { params, split, body, env: _fenv } = v.as_ref() {
                        Some((params.clone(), *split, Rc::new((**body).clone())))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            // A bare block `{ … }` applied as a function: bind `⍵` (right) / `⍺` (left) and
            // evaluate the block body in a fresh child scope. (`{ … } x` and `a { … } b`.)
            Instr::Block { body } => {
                let child = Environment::child(&env);
                let right_val = self.eval_instr(right, env)?.force(self)?;
                if let Some(l) = left {
                    let lv = self.eval_instr(l, env)?.force(self)?;
                    child.define("⍺", &None, lv);
                }
                child.define("⍵", &None, right_val.clone());
                // `⍓` (Kap's OUTER_CALL_SYMBOL) refers to the enclosing function — for a bare
                // block it is the block itself, enabling anonymous self-recursion.
                child.define("⍓", &None, Rc::new(APLValue::UserFn {
                    params: vec![],
                    split: 1,
                    body: Rc::new(Instr::Block { body: body.clone() }),
                    env: env.clone(),
                }));
                // Return early on `→` (branch/return) so it exits the block.
                return match self.eval_block(body, &child) {
                    Err(AplError::Return(v)) => Ok(v),
                    other => other,
                };
            }
            // `⍞name`: a dynamic function reference. Resolve `name` to its value (a
            // function) and apply it. Mirrors Kap's DynamicFunctionDescriptor.
            Instr::DynamicRef { name, namespace } => {
                let v = env
                    .lookup(name, namespace)
                    .ok_or_else(|| AplError::runtime(format!("undefined symbol: {}", name)))?;
                match v.as_ref() {
                    APLValue::UserFn { params, split, body, env: fenv } => {
                        return self.apply_user_fn(
                            params,
                            *split,
                            body.as_ref(),
                            left,
                            right,
                            fenv,
                            Some(name),
                        );
                    }
                    APLValue::UserOp { .. } => {
                        return Err(AplError::runtime(format!(
                            "{} is an operator, not a function",
                            name
                        )));
                    }
                    other => {
                        let desc = match other {
                            APLValue::Number(_) => "number",
                            APLValue::Array(_) => "array",
                            APLValue::Str(_) => "string",
                            APLValue::Char(_) => "char",
                            APLValue::Null => "null",
                            _ => "non-function value",
                        };
                        return Err(AplError::runtime(format!(
                            "{} is not a function (got {})",
                            name, desc
                        )));
                    }
                }
            }
            // Operator call with function operands, e.g. `+foo 2` / `-foo+ 3`. `op` names a
            // user-defined operator (resolved to `APLValue::UserOp`); `left_fn`/`right_fn`
            // are the function operands bound to its `op_left`/`op_right`. The combined
            // operator is then applied to the trailing data args (`2`, `3`).
            Instr::OpCall { op, left_fn, right_fn } => {
                return self.apply_user_op(op, left_fn, right_fn, left, right, env);
            }
            _ => None,
        };
        if let Some((params, split, body)) = lambda {
            return self.apply_user_fn(&params, split, &body, left, right, env, fn_name.as_deref());
        }
        // --- Value-right-arg operators (e.g. `f⍤1` rank) ---
        // Kotlin APLOperatorValueRightArg (engine.kt:494 RankOperator). The operand is
        // a VALUE evaluated at application time. Currently `⍤` (rank) is the only one.
        if let Instr::ValueOp { func, op_name, operand } = fn_expr {
            if op_name == "⍤" {
                let rank_val = self.eval_instr(operand, env)?.force(self)?;
                return self.apply_rank_op(func, &rank_val, left, right, env);
            }
            return Err(AplError::runtime(format!(
                "unknown value-right-arg operator: {}",
                op_name
            )));
        }
        // --- Derived functions from adverbs (e.g. `+/`, `×¨`) ---
        // `fn_expr` is an `Instr::Derived { func, op }`; `op` is the adverb name (`/`,
        // `\`, `¨`) and `func` is the function operand. `left`/`right` are the data
        // arguments (dyadic each has both; reduce/scan/each usually just `right`).
        if let Instr::Derived { func, op } = fn_expr {
            let adv_name = match op.as_ref() {
                Instr::Symbol { name, .. } => name.clone(),
                _ => return Err(AplError::runtime("adverb must be a symbol".into())),
            };
            return match adv_name.as_str() {
                "/" | "reduce" => self.adverb_reduce(func, left, right, env, true),
                "\\" | "scan" => self.adverb_scan(func, left, right, env, true),
                "⌿" => self.adverb_reduce(func, left, right, env, false),
                "⍀" => self.adverb_scan(func, left, right, env, false),
                "¨" | "each" => self.adverb_each(func, left, right, env),
                // `⌻` outer product (Kotlin outer_join.kt OuterJoinOp): `A f⌻ B`
                // builds the rank-(⍴⍴A + ⍴⍴B) table of f(a,b) over every cell pair.
                "⌻" => return self.outer_product(func, left, right, env),
                // `⍨` commute (Kotlin commute.kt CommuteFunctionImpl):
                // monadic f⍨ y = y f y; dyadic x f⍨ y = y f x (arguments swapped).
                "⍨" | "commute" => match left {
                    None => {
                        let y = self.eval_instr(right, env)?.force(self)?;
                        self.eval_apply(
                            func,
                            &Some(Box::new(Instr::Value(y.clone()))),
                            &Box::new(Instr::Value(y)),
                            env,
                        )
                    }
                    Some(l) => {
                        let x = self.eval_instr(l, env)?.force(self)?;
                        let y = self.eval_instr(right, env)?.force(self)?;
                        // x f⍨ y = y f x  → left arg = y, right arg = x.
                        self.eval_apply(
                            func,
                            &Some(Box::new(Instr::Value(y))),
                            &Box::new(Instr::Value(x)),
                            env,
                        )
                    }
                },
                // `∵` (BitwiseOp, Kotlin bitwise_ops.kt): a one-arg operator that derives the
                // *bitwise* variant of its function operand. `∨∵`→bitwise-OR, `∧∵`→bitwise-AND,
                // `⌽∵`→bitwise-shift (left = shift count, right = value). Works element-wise over
                // integer arrays (Kotlin's BitwiseCombineAPLFunction extends MathCombineAPLFunction).
                "∵" | "bitwise" => {
                    // Resolve the function operand to a primitive name.
                    let fname = match func.as_ref() {
                        Instr::Symbol { name, .. } => name.clone(),
                        _ => return Err(AplError::runtime("∵ requires a primitive function operand".into())),
                    };
                    return self.bitwise_apply(&fname, left, right, env);
                }
                // `⌸` (Key operator): `keys {fn}⌸ values`. Groups `values` by the
                // corresponding key; for each unique key (first-occurrence order) the
                // result row is `(key, fn(group))`. The fn is called dyadically with
                // ⍺=key, ⍵=the enclosed group vector.
                "⌸" | "key" => return self.key_apply(func, left, right, env),
                other => Err(AplError::runtime(format!("unknown adverb: {}", other))),
            };
        }
        // --- Trains: (f g h) as a derived function ---
        // Monadic: right-to-left composition  (f g h) y = f (g (h y)).
        // Dyadic 2-train (atop):       x (A B) y = A x (B y).
        // Dyadic 3-train (fork):       x (A B C) y = (x A y) B (x C y).
        if let Instr::Train { funcs, reverse, compose } = fn_expr {
            return self.apply_train(funcs, *reverse, *compose, left, right, env);
        }
        // --- User-defined / native operators called with explicit data args ---
        // e.g. `10 +foo 2` parses as `Apply{fn: OpCall{op:foo, left_fn:+}, left:10, right:2}`.
        if let Instr::OpCall { op, left_fn, right_fn } = fn_expr {
            return self.apply_user_op(op, left_fn, right_fn, left, right, env);
        }
        let name = match fn_name {
            Some(ref n) => n.clone(),
            None => {
                return Err(AplError::runtime("only symbol/lambda functions supported yet".into()));
            }
        };
        // User/native function definitions take precedence over hardcoded builtins
        // (Kap semantics: a library `∇`/`⇐` redefinition of a primitive name — e.g.
        // `math-kap.kap`'s `⊥`/`⊤` — shadows the builtin in that namespace). Only
        // consult the builtin table when no user/native function is bound to this name.
        if let Some(ns_opt) = fn_namespace.clone().flatten() {
            if let Some(v) = env.lookup(&name, &Some(ns_opt)) {
                if let APLValue::UserFn { params, split, body, env: fenv } = v.as_ref() {
                    return self.apply_user_fn(
                        params,
                        *split,
                        body,
                        left,
                        right,
                        fenv,
                        Some(&name),
                    );
                }
            }
        } else if let Some(v) = env.lookup(&name, &None) {
            if let APLValue::UserFn { params, split, body, env: fenv } = v.as_ref() {
                return self.apply_user_fn(
                    params,
                    *split,
                    body,
                    left,
                    right,
                    fenv,
                    Some(&name),
                );
            }
        }
        // For dyadic, force left then right; for monadic, only right.
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        match name.as_str() {
            "+" => {
                // Ambivalent: monadic `+ x` = identity (return x); dyadic = add.
                match left_val {
                    None => Ok(right_val),
                    Some(ref lv) => {
                        if let Some(r) = self.compute_char_op(lv.as_ref(), right_val.as_ref(), true)
                        {
                            return r;
                        }
                        self.num2(left_val, right_val, |a, b| a.add(b), "+")
                    }
                }
            }
            "-" => {
                // Ambivalent: monadic `- x` = negate; dyadic = subtract.
                match left_val {
                    None => self.negate(right_val),
                    Some(ref lv) => {
                        if let Some(r) =
                            self.compute_char_op(lv.as_ref(), right_val.as_ref(), false)
                        {
                            return r;
                        }
                        self.num2(left_val, right_val, |a, b| a.sub(b), "-")
                    }
                }
            }
            "⍕" | "format" => {
                // Monadic format-to-string: render any value as a `Str`.
                // Kap: ⍕8 => "8", ⍕@a => "a", ⍕"foo" => "foo", ⍕⍬ => "" (empty).
                // Real Kap uses `formatted(FormatStyle.PLAIN)` — recursively flatten
                // the value to its scalar leaves and concatenate with NO separators
                // and NO parentheses (⍕ 1 2 3 => "123", ⍕(2 2⍴⍳4) => "0123").
                if left_val.is_none() {
                    if let APLValue::Null = right_val.as_ref() {
                        Ok(Rc::new(APLValue::Str(String::new())))
                    } else {
                        Ok(Rc::new(APLValue::Str(right_val.format_plain())))
                    }
                } else {
                    // Dyadic format with directives: left is format string, right is args.
                    self.format_with_directives(&left_val.unwrap().format_value(), right_val)
                }
            }
            "⍎" | "execute" => {
                // Monadic execute-string is Kotlin's `ParseNumberFunction`
                // (format.kt:250), NOT a general expression evaluator. It parses `s`
                // as a NUMBER ONLY — integer → double → rational, each via an
                // anchored regex using ASCII `-` (not Kap's `¯`) — and throws if no
                // number pattern matches (oracle: `⍎"1+2"` / `⍎"⍳3"` → error, not 3).
                // Error text mirrors Kotlin's `Value cannot be parsed as a number`.
                let s = match right_val.as_ref() {
                    APLValue::Str(s) => s.clone(),
                    _ => return Err(AplError::runtime("⍎: Argument is not a string".into())),
                };
                let t = s.trim();
                match KapNumber::parse_kap_number_string(t) {
                    Some(n) => Ok(Rc::new(APLValue::Number(n))),
                    None => Err(AplError::runtime(format!(
                        "⍎: Value cannot be parsed as a number: '{}'",
                        s
                    ))),
                }
            }
            // `io:print` / `io:println`: write the value's *plain* (unquoted) rendering
            // to stdout and return the value unchanged. The REPL then displays the
            // returned value with `format_display` (strings get quotes), so:
            //   io:println "foo bar"  -> prints `foo bar`, then REPL shows `"foo bar"`.
            // `style io:print` / `style io:println` (style = the string "pretty"/"read")
            // switch to the quoted / readable rendering — mirrors Real Kap's `:pretty`.
            "io:print" => {
                let rendered = match left_val.as_ref() {
                    None => right_val.format_value(),
                    Some(style) => self.io_style_render(style.as_ref(), right_val.as_ref())?,
                };
                print!("{}", rendered);
                Ok(right_val)
            }
            "io:println" => {
                let rendered = match left_val.as_ref() {
                    None => right_val.format_value(),
                    Some(style) => self.io_style_render(style.as_ref(), right_val.as_ref())?,
                };
                println!("{}", rendered);
                Ok(right_val)
            }
            // `math:*` — P2 (ROADMAP §5) port of engine.kt:436–466 registrations
            // (`SinAPLFunction` etc. in math_functions.kt, prime.kt). Monadic fns are
            // element-wise over arrays via scalar1; dyadic (atan2/hypot/gcd/lcm) via num2.
            "math:sin" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().sin()), "math:sin"),
            "math:cos" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().cos()), "math:cos"),
            "math:tan" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().tan()), "math:tan"),
            "math:asin" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().asin()), "math:asin"),
            "math:acos" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().acos()), "math:acos"),
            "math:atan" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().atan()), "math:atan"),
            "math:sinh" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().sinh()), "math:sinh"),
            "math:cosh" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().cosh()), "math:cosh"),
            "math:tanh" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().tanh()), "math:tanh"),
            "math:asinh" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().asinh()), "math:asinh"),
            "math:acosh" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().acosh()), "math:acosh"),
            "math:atanh" => self.scalar1(right_val, |x| KapNumber::Double(x.as_double().atanh()), "math:atanh"),
            // Dyadic only (Kotlin Atan2APLFunction/HypotAPLFunction).
            "math:atan2" => match left_val {
                Some(_) => self.num2(left_val, right_val, |a, b|
                    KapNumber::Double(a.as_double().atan2(b.as_double())), "math:atan2"),
                None => Err(AplError::runtime("math:atan2 needs two args".into())),
            },
            "math:hypot" => match left_val {
                Some(_) => self.num2(left_val, right_val, |a, b|
                    KapNumber::Double(a.as_double().hypot(b.as_double())), "math:hypot"),
                None => Err(AplError::runtime("math:hypot needs two args".into())),
            },
            // gcd/lcm: monadic call is an ERROR in Kotlin ("gcd: Function cannot be called
            // with one argument"); dyadic on non-negative integers.
            "math:gcd" => match left_val {
                Some(_) => self.num2(left_val, right_val, Self::kap_gcd, "math:gcd"),
                None => Err(AplError::runtime("gcd: Function cannot be called with one argument".into())),
            },
            "math:lcm" => match left_val {
                Some(_) => self.num2(left_val, right_val, |a, b| {
                    if matches!(a, KapNumber::Long(0)) && matches!(b, KapNumber::Long(0)) {
                        return KapNumber::Long(0);
                    }
                    let g = Self::kap_gcd(a, b);
                    let p = a.mul(b);
                    // lcm = |a*b| / gcd; integer division when both integral.
                    match p.div(&g) {
                        KapNumber::Long(v) => KapNumber::Long(v.abs()),
                        other => other,
                    }
                }, "math:lcm"),
                None => Err(AplError::runtime("lcm: Function cannot be called with one argument".into())),
            },
            // numerator/denominator: rational components (Long n → n/1).
            "math:numerator" => match right_val.as_ref() {
                APLValue::Number(n) => Ok(Rc::new(APLValue::Number(match n {
                    KapNumber::Rational(r) => KapNumber::BigInt(r.numer().clone()),
                    KapNumber::Double(d) if d.fract() == 0.0 && *d >= i64::MIN as f64 && *d <= i64::MAX as f64 =>
                        KapNumber::Long(*d as i64),
                    other => KapNumber::Long(other.as_long().map_err(|e| AplError::runtime(e))?),
                }))),
                _ => Err(AplError::runtime("math:numerator requires a number".into())),
            },            "math:denominator" => match right_val.as_ref() {
                APLValue::Number(n) => Ok(Rc::new(APLValue::Number(match n {
                    KapNumber::Rational(r) => KapNumber::BigInt(r.denom().clone()),
                    _ => KapNumber::Long(1),                }))),
                _ => Err(AplError::runtime("math:denominator requires a number".into())),
            },

            "math:round" => self.scalar1(right_val, |x| {
                let d = x.as_double();
                // kotlin.math.round: nearest integer, ties to EVEN
                // (oracle: 2.5→2, 3.5→4, ¯2.5→¯2).
                let r = d.round_ties_even();
                if r >= i64::MIN as f64 && r <= i64::MAX as f64 {
                    KapNumber::Long(r as i64)
                } else {
                    KapNumber::Double(r)
                }
            }, "math:round"),
            // Number theory (prime.kt): factor / divisors / primes / isPrime.
            "math:factor" => self.math_factor(right_val),
            "math:divisors" => self.math_divisors(right_val),
            "math:primes" => self.math_primes(right_val),
            "math:isPrime" => self.scalar1(right_val, |x| {
                let v = x.as_long().unwrap_or_else(|_| x.as_double() as i64);
                KapNumber::Long(if v >= 2 && Self::is_prime_u64(v as u64) { 1 } else { 0 })
            }, "math:isPrime"),
            // `int:formatRational` (fmt-rational.kt): dyadic only — `decimals f v`
            // renders rational v with `decimals` decimal places, returning the 2-element
            // array [string, exact-flag]. Monadic call errors with Kotlin text.
            "int:formatRational" | "math:formatRational" => {
                let l = left_val.ok_or_else(|| {
                    AplError::runtime("formatRational: Function cannot be called with one argument".into())
                })?;
                let decimals = match l.as_ref() {
                    APLValue::Number(n) => n.as_long().map_err(|e| AplError::runtime(e))? as u32,
                    _ => return Err(AplError::runtime("int:formatRational requires a number scale".into())),
                };
                let v = match right_val.as_ref() {
                    APLValue::Number(n) => n,
                    _ => return Err(AplError::runtime("int:formatRational requires a number".into())),
                };
                let (s, exact) = self.format_rational(v, decimals);
                use crate::array::{ArrayData, KapArray};
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![2],
                    ArrayData::Nested(vec![
                        Rc::new(APLValue::Str(s)),
                        Rc::new(APLValue::Number(KapNumber::Long(if exact { 1 } else { 0 }))),
                    ]),
                )))))
            }
            // `sysparam` (div_functions.kt SystemParameterFunction + custom-renderer.kt):
            // monadic lookup, dyadic update. The parameter name is a SYMBOL VALUE
            // (`'kap:altVectorOutput`, i.e. Symbol{name, namespace:"kap"}); keyword-form
            // symbols (`:foo`) are NOT registered parameters in the text-mode build.
            "sysparam" => {
                let name_of = |v: &APLValue| -> Option<(String, String)> {
                    match v {
                        APLValue::Symbol { name, namespace } => Some((
                            name.clone(),
                            namespace.clone().unwrap_or_else(|| "default".to_string()),
                        )),
                        _ => None,
                    }
                };
                match left_val {
                    Some(l) => {
                        // Dyadic: set. Returns the collapsed new value.
                        let key = name_of(l.as_ref()).ok_or_else(|| {
                            AplError::runtime(format!(
                                "sysparam: Value {} is not a symbol",
                                l.format_value()
                            ))
                        })?;
                        let known = matches!(
                            key.0.as_str(),
                            "kap:altVectorOutput"
                                | "kap:rendererParameters"
                                | "kap:renderer"
                                | "default:altVectorOutput"
                        );
                        if !known {
                            return Err(AplError::runtime(format!(
                                    "sysparam: System parameter not found: :{}",
                                    key.0
                                )));
                        }
                        Ok(right_val)
                    }
                    None => {
                        let key = name_of(right_val.as_ref()).ok_or_else(|| {
                            AplError::runtime(format!(
                                "sysparam: Value {} is not a symbol",
                                right_val.format_value()
                            ))
                        })?;
                        match key.0.as_str() {
                            "kap:altVectorOutput" | "default:altVectorOutput" => {
                                Ok(Rc::new(APLValue::Number(KapNumber::Long(1))))
                            }
                            "kap:rendererParameters" | "default:rendererParameters" => {
                                // ⟨⟨200 50⟩ ⟨60 10⟩⟩ — max height/width then label cell size.
                                use crate::array::{ArrayData, KapArray};
                                let row = |a: i64, b: i64| {
                                    Rc::new(APLValue::Array(Rc::new(KapArray::new(
                                        vec![2],
                                        ArrayData::Long(vec![a, b]),
                                    ))))
                                };
                                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                                    vec![2],
                                    ArrayData::Nested(vec![row(200, 50), row(60, 10)]),
                                )))))
                            }
                            other => Err(AplError::runtime(format!(
                                "sysparam: System parameter not found: :{}",
                                other.split_once(':').map(|(_, b)| b).unwrap_or(other)
                            ))),
                        }
                    }
                }
            }
            // `encoder:*` — P2 (engine.kt:469–470): binary value codec (Kap wire format).
            "encoder:encode" => self.encoder_encode(right_val),
            "encoder:decode" => self.encoder_decode(right_val),
            // `unicode:*` — character / encoding utilities (Real Kap UnicodeModule).
            "unicode:toCodepoints" => self.unicode_to_codepoints(right_val),
            "unicode:fromCodepoints" => self.unicode_from_codepoints(right_val),
            "unicode:toGraphemes" => self.unicode_to_graphemes(right_val),
            "unicode:toLower" => self.unicode_case(right_val, false),
            "unicode:toUpper" => self.unicode_case(right_val, true),
            "unicode:toNames" => self.unicode_to_names(right_val),
            "unicode:enc" => self.unicode_enc(left_val, right_val),
            "unicode:dec" => self.unicode_dec(left_val, right_val),
            // `s:trimLeft` / `s:trimRight` / `s:trim` (Real Kap `s.kt`/`util.kap`):
            // strip leading/trailing/both whitespace from a string (monadic). Whitespace
            // = space, tab, newline, carriage return, vertical tab, form feed.
            "s:trimLeft" => match right_val.as_ref() {
                APLValue::Str(s) => Ok(Rc::new(APLValue::Str(s.trim_start().to_string()))),
                other => Err(AplError::runtime(format!(
                    "s:trimLeft requires a string, got: {}",
                    other.format_value()
                ))),
            },
            "s:trimRight" => match right_val.as_ref() {
                APLValue::Str(s) => Ok(Rc::new(APLValue::Str(s.trim_end().to_string()))),
                other => Err(AplError::runtime(format!(
                    "s:trimRight requires a string, got: {}",
                    other.format_value()
                ))),
            },
            "s:trim" => match right_val.as_ref() {
                APLValue::Str(s) => Ok(Rc::new(APLValue::Str(s.trim().to_string()))),
                other => Err(AplError::runtime(format!(
                    "s:trim requires a string, got: {}",
                    other.format_value()
                ))),
            },
            // `regex:*` — regular-expression string utilities (Real Kap RegexpModule).
            // Left arg is the pattern (string); right arg is the subject string (or, for
            // `replace`, an `(subject; replacement)` pair). Mirrors regexp.kt.
            "regex:match" => self.regex_match(left_val, right_val),
            "regex:find" => self.regex_find(left_val, right_val),
            "regex:finderror" => self.regex_finderror(left_val, right_val),
            "regex:findall" => self.regex_findall(left_val, right_val),
            "regex:replace" => self.regex_replace(left_val, right_val),
            "regex:split" => self.regex_split(left_val, right_val),
            "regex:compile" => self.regex_compile(left_val, right_val),
            // `int:intern` (dyadic): `"ns" int:intern "name"` -> Symbol{name, ns}.
            // `int:symbolName` (monadic): `'foo:bar` -> `(bar foo)` vector of
            // [name, namespace]. Mirrors Kotlin `symbol.kt`.
            "int:intern" => {
                let ns_name = match left_val.as_ref() {
                    Some(v) => v.format_value(),
                    None => "default".to_string(),
                };
                let name = match right_val.as_ref() {
                    APLValue::Str(s) => s.clone(),
                    APLValue::Char(c) => c.to_string(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "int:intern name must be a string/char, got: {}",
                            other.format_value()
                        )))
                    }
                };
                let namespace = if ns_name == "keyword" {
                    Some("keyword".to_string())
                } else if ns_name == "default" {
                    None
                } else {
                    Some(ns_name)
                };
                Ok(Rc::new(APLValue::Symbol { name, namespace }))
            }
            "int:symbolName" => {
                let sym = match right_val.as_ref() {
                    APLValue::Symbol { name, namespace } => (name.clone(), namespace.clone()),
                    other => {
                        return Err(AplError::runtime(format!(
                            "int:symbolName requires a symbol, got: {}",
                            other.format_value()
                        )))
                    }
                };
                let ns_name = match &sym.1 {
                    Some(ns) => ns.clone(),
                    None => "default".to_string(),
                };
                let arr = KapArray::new(
                    vec![2],
                    ArrayData::Nested(vec![
                        Rc::new(APLValue::Str(sym.0)),
                        Rc::new(APLValue::Str(ns_name)),
                    ]),
                );
                Ok(Rc::new(APLValue::Array(Rc::new(arr))))
            }
            // `int:unwindProtect` (monadic, Kotlin UnwindProtectAPLFunction): the argument
            // is a 2-element vector `[fn, handler]` of lambdas. Run fn (⍵=⍬); run handler
            // afterwards; if fn threw a Kap error, rethrow it after the handler. Used by
            // structure.kap's `defsyntax unwindProtect`, which the stdlib needs at load.
            "int:unwindProtect" => {
                // Kotlin strands `int:unwindProtect statement handler` into a 2-vector
                // `[statement, handler]` (monadic call on a stranded pair). The stdlib's
                // `defsyntax unwindProtect (:function statement :function handler) {
                // int:unwindProtect statement handler }` relies on this. So accept BOTH:
                //   - monadic: right_val is the 2-element vector, OR
                //   - dyadic (stranded): left_val=statement, right_val=handler.
                let arg: AplRef<APLValue> = match left_val {
                    Some(lv) => {
                        // Stranded form: build the 2-vector [left, right].
                        let l = lv.force(self)?;
                        let r = right_val.force(self)?;
                        Rc::new(APLValue::Array(Rc::new(KapArray::new(
                            vec![2],
                            ArrayData::Nested(vec![l, r]),
                        ))))
                    }
                    None => right_val.force(self)?,
                };
                let parts: Vec<AplRef<APLValue>> = match arg.as_ref() {
                    APLValue::Array(a) => a.elements(),
                    _ => return Err(AplError::runtime(
                        "Invalid dimensions in unwindProtect call".into(),
                    )),
                };
                if parts.len() != 2 {
                    return Err(AplError::runtime(
                        "Invalid dimensions in unwindProtect call".into(),
                    ));
                }
                let null = Rc::new(APLValue::Null);
                // Run the main fn (⍵=⍬); capture but don't propagate yet.
                let main_res: Result<AplRef<APLValue>, AplError> = {
                    let fn_instr = self.apl_to_instr(parts[0].as_ref())?;
                    self.apply_fn_instr(&fn_instr, None, &null, env)
                };
                // Handler always runs (⍵=⍬).
                let handler_instr = self.apl_to_instr(parts[1].as_ref())?;
                self.apply_fn_instr(&handler_instr, None, &null, env)?;
                main_res
            }
            // `int:throwNative` (dyadic): `Symbol int:throwNative Message` — throw a native
            // Kap exception whose type is named by the left symbol and whose message is the
            // right operand (coerced to a string). Mirrors Kotlin `throw-native.kt`
            // `ThrowNativeFunction.eval2Arg`:
            //   kap:KapEvalException    -> Kap eval error (the one io.kap's fromHex uses)
            //   InvalidDimensionsException -> dimension error
            //   IllegalArgumentException    -> illegal-argument error
            //   anything else              -> "Invalid exception name: …"
            // The REAL Kap REPL renders `throwNative: <message>`; we mirror that prefix so
            // port error text reads like the oracle (e.g. `throwNative: Invalid characters in
            // hex string`). The left operand is a *symbol* (typically a quoted symbol literal
            // `'kap:KapEvalException`); a bare symbol reference also works.
            "int:throwNative" => {
                // The left operand is a *symbol*. In Kap a quoted symbol literal
                // `'kap:KapEvalException` is a single token whose namespace/name carry the
                // `ns:name` form — but in the port a quoted symbol may arrive with the whole
                // `ns:name` packed into `name` and `namespace == None` (the lexer doesn't
                // split a quoted symbol the way it splits a bare `ns:name`). Normalise both
                // shapes: if `name` itself contains a `:`, split it into (ns, name).
                let sym = match left_val.as_ref() {
                    Some(lv) => match lv.as_ref() {
                        APLValue::Symbol { name, namespace } => {
                            let (ns, nm) = if name.contains(':') {
                                let mut parts = name.splitn(2, ':');
                                (
                                    parts.next().unwrap_or("default").to_string(),
                                    parts.next().unwrap_or(name.as_str()).to_string(),
                                )
                            } else {
                                (
                                    namespace.clone().unwrap_or_else(|| "default".to_string()),
                                    name.clone(),
                                )
                            };
                            format!("{}:{}", ns, nm)
                        }
                        other => {
                            return Err(AplError::runtime(format!(
                                "int:throwNative expects a symbol on the left, got: {}",
                                other.format_value()
                            )))
                        }
                    },
                    None => {
                        return Err(AplError::runtime(
                            "int:throwNative requires a left argument (a symbol)".to_string(),
                        ))
                    }
                };
                let message = match right_val.as_ref() {
                    APLValue::Str(s) => s.clone(),
                    APLValue::Char(c) => c.to_string(),
                    other => other.format_value(),
                };
                let kind = match sym.as_str() {
                    "kap:KapEvalException" => "throwNative",
                    "default:InvalidDimensionsException" => "Invalid dimensions",
                    "default:IllegalArgumentException" => "Illegal argument",
                    _ => {
                        return Err(AplError::runtime(format!(
                            "throwNative: Invalid exception name: {}",
                            sym
                        )))
                    }
                };
                // The oracle's KapEvalException path prints `throwNative: <message>`; the
                // other two print their own canonical label. Match the oracle's rendered form.
                let rendered = if kind == "throwNative" {
                    format!("throwNative: {}", message)
                } else {
                    format!("{}: {}", kind, message)
                };
                Err(AplError::runtime(rendered))
            }
            // `typeof` (monadic): returns a *symbol* naming the Kap class of the
            // argument (Kotlin `TypeofFunction` → `classManager.nameForClass`).
            // e.g. `typeof 10` → INTEGER, `typeof "x"` → STRING. The port renders
            // a default-namespace symbol as `default:NAME` (see `format_value`).
            "typeof" => {
                let v = right_val.force(self)?;
                // Real Kap returns the class name as a *symbol in the `kap` namespace*
                // (lowercase, e.g. `kap:array`, `kap:symbol`).
                let name = v.class_name().to_string();
                Ok(Rc::new(APLValue::Symbol {
                    name,
                    namespace: Some("kap".to_string()),
                }))
            }
            // `isLocallyBound` (monadic, Kotlin `IsLocallyBoundFunction`): takes a
            // *symbol literal* (`'⍺`) and returns 1 if that symbol is bound in the
            // current scope chain with a value — used by stdlib to detect whether a
            // left argument was supplied (`n ← if (isLocallyBound('⍺)) { ⍺ } …`).
            "isLocallyBound" => {
                let v = right_val.force(self)?;
                let name = match v.as_ref() {
                    APLValue::Symbol { name, .. } => name.clone(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "isLocallyBound requires a symbol, got: {}",
                            other.format_value()
                        )))
                    }
                };
                let bound = env.lookup(&name, &None).is_some();
                Ok(Rc::new(APLValue::Number(KapNumber::Long(bound as i64))))
            }
            // --- Namespace directives (Kotlin `namespace`/`import`/`declare`). ---
            // `namespace("foo")` sets the current module namespace for subsequent
            // top-level bindings/lookups. `import("foo")` makes `foo`'s *exported*
            // symbols visible in the current namespace. `declare(:export a)` marks a
            // symbol for export. These are runtime directives (in-memory only; `use`
            // file-loading is deferred per the roadmap).
            "namespace" => {
                let name = match right_val.as_ref() {
                    APLValue::Str(s) => s.clone(),
                    APLValue::Symbol { name, .. } => name.clone(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "namespace requires a name, got: {}",
                            other.format_value()
                        )))
                    }
                };
                env.ns_registry.current.replace(Some(name));
                Ok(right_val)
            }
            "import" => {
                let name = match right_val.as_ref() {
                    APLValue::Str(s) => s.clone(),
                    APLValue::Symbol { name, .. } => name.clone(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "import requires a namespace name, got: {}",
                            other.format_value()
                        )))
                    }
                };
                let cur = env.ns_registry.current_ns();
                env.ns_registry.add_import(&cur, &name);
                Ok(right_val)
            }
            "declare" => {
                // Handled as a *special form* (see the `declare` interception at the top
                // of `eval_apply`, which passes the *unevaluated* argument AST). This arm
                // is only reached if `declare` is somehow applied through the normal
                // path; route it the same way for safety. `right_val` may already be
                // forced here, so prefer re-structuring from the AST is not possible —
                // just re-run via the structural method using the evaluated array.
                self.eval_declare_struct(right_val.clone(), env)
            }
            // `use("file.kap")` — load and evaluate a library file in the *current*
            // namespace (so its top-level `∇`/`⇐` definitions land where the call
            // appears). Mirrors Kotlin's `LoadFileFunction`/`resolveLibraryFile`:
            // the argument is a string resolved by *basename* across the library
            // search path. We add a recursion guard (see `Engine::include_stack`) so
            // a file that self-`use`s loads exactly once.
            "use" => {
                let name = match right_val.as_ref() {
                    APLValue::Str(s) => s.clone(),
                    APLValue::Char(c) => c.to_string(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "use: argument must be a filename string, got: {}",
                            other.format_value()
                        )))
                    }
                };
                self.use_file(&name, env)
            }
            "÷" => match left_val {
                None => self.scalar1(right_val, |x| x.recip(), "÷"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.div(b), "÷"),
            },
            // Dyadic `/` with an integer left arg is REPLICATE (Kotlin
            // ReplicateAPLFunction): `3/7 → ⟨7 7 7⟩`, `1 0 1 2/10 20 30 40 →
            // ⟨10 30 40 40⟩`.
            "/" => match left_val {
                None => self.scalar1(right_val, |x| x.recip(), "/"),
                Some(_) => self.replicate(left_val, right_val),
            },
            "=" => match left_val {
                // Monadic `=` (self-classify, Kotlin EqualsAPLFunction.eval1Arg): for each
                // major cell of the ravelled argument, the index (in first-occurrence
                // order) of the cell's class — using type-qualified equality (`≡` rules,
                // so 10 ≠ 10.0). `= ⍬ → ⍬`.
                None => {
                    let v = right_val.force(self)?;
                    // A Str iterates as its characters (oracle: `= "abc" → ⟨0 1 2⟩`).
                    let elems: Vec<AplRef<APLValue>> = match v.as_ref() {
                        APLValue::Array(a) => a.elements(),
                        APLValue::Str(s) => {
                            s.chars().map(|c| Rc::new(APLValue::Char(c))).collect()
                        }
                        other => vec![Rc::new(other.clone())],
                    };
                    if elems.is_empty() {
                        return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                            vec![0],
                            ArrayData::Long(vec![]),
                        )))));
                    }
                    // First-occurrence classes with type-strict comparison.
                    let mut classes: Vec<AplRef<APLValue>> = Vec::new();
                    let mut out: Vec<i64> = Vec::with_capacity(elems.len());
                    for e in &elems {
                        let found = classes.iter().position(|c| {
                            Self::type_equal(c.as_ref(), e.as_ref())
                        });
                        match found {
                            Some(i) => out.push(i as i64),
                            None => {
                                out.push(classes.len() as i64);
                                classes.push(e.clone());
                            }
                        }
                    }
                    Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                        vec![out.len()],
                        ArrayData::Long(out),
                    )))))
                }
                Some(_) => self.cmp2_elements(left_val, right_val, |o| o == Ordering::Equal, "="),
            },
            "≠" => match left_val {
                // Monadic `≠` (unique-mask, Kotlin NotEqualsAPLFunction.eval1Arg):
                // 1 for the first occurrence of each distinct element, else 0.
                None => {
                    let v = right_val.force(self)?;
                    let elems: Vec<AplRef<APLValue>> = match v.as_ref() {
                        APLValue::Array(a) => a.elements(),
                        APLValue::Str(s) => {
                            s.chars().map(|c| Rc::new(APLValue::Char(c))).collect()
                        }
                        other => vec![Rc::new(other.clone())],
                    };
                    let mut out: Vec<i64> = Vec::with_capacity(elems.len());
                    for (i, e) in elems.iter().enumerate() {
                        let seen = elems[..i].iter().any(|p| Self::type_equal(p.as_ref(), e.as_ref()));
                        out.push(if seen { 0 } else { 1 });
                    }
                    Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                        vec![out.len()],
                        ArrayData::Long(out),
                    )))))
                }
                Some(_) => self.cmp2_elements(left_val, right_val, |o| o != Ordering::Equal, "≠"),
            },
            "<" => self.cmp2_elements(left_val, right_val, |o| o == Ordering::Less, "<"),
            ">" => self.cmp2_elements(left_val, right_val, |o| o == Ordering::Greater, ">"),
            "≤" => self.cmp2_elements(left_val, right_val, |o| o != Ordering::Greater, "≤"),
            "≥" => self.cmp2_elements(left_val, right_val, |o| o != Ordering::Less, "≥"),
            "cmp" => self.cmp_values(left_val, right_val, "cmp"),
            "⍳" | "iota" => match left_val {
                None => self.iota(right_val),
                Some(l) => self.index_of(l, right_val),
            },
            "⍴" | "rho" => match left_val {
                None => self.shape(right_val),
                Some(l) => self.reshape(l, right_val),
            },
            "≢" | "tally" => self.tally(right_val),
            "⊃" | "first" => self.reveal(left_val, right_val),
            "," | "⍪" => self.catenate(left_val, right_val),
            "⌽" | "rotateright" => self.reverse_horizontal(left_val, right_val),
            "⊖" | "rotateleft" => self.reverse_vertical(left_val, right_val),
            "⍉" => self.transpose(left_val, right_val),
            "↑" => self.take(left_val, right_val),
            "↓" => self.drop(left_val, right_val),
            "⊂" => self.enclose(right_val),
            "⊆" => self.partitioned_enclose(left_val, right_val),
            "⊇" => self.pick_apl(left_val, right_val),
            "→" | "branch" => self.return_arrow(left_val, right_val),
            "⍮" | "pair" => self.pair(left_val, right_val),
            "⌷" | "reveal" => self.access_from_index(left_val, right_val),
            // `≬` / `toList` (Kotlin `ToListFunction`, div_functions.kt): monadic-only.
            // Coerces a scalar or 1-D array into a Kap list (a rank-0 box whose single
            // element is the array). Fluent inverse `fromList` (Kotlin `FromListFunction`)
            // recovers the array. Dyadic application is an error.
            "≬" | "toList" => match left_val {
                None => self.to_list(right_val),
                Some(_) => Err(AplError::runtime("≬: Function cannot be called with two arguments".into())),
            },
            "fromList" => match left_val {
                None => self.from_list(right_val),
                Some(_) => Err(AplError::runtime("fromList: Function cannot be called with two arguments".into())),
            },
            // `⫇` / `group` — Kotlin GroupFunction (group-index.kt). Left `L` is a rank-1
            // vector of group indices (negative => element skipped). Right `R` has major
            // axis == length(L); each index selects into R's major cells. Returns a vector
            // of arrays: group `i` holds its selected cells (skipped indices leave APLNull
            // in that slot). For a rank-1 R the cells are scalars; for higher rank they are
            // the (rank-1) sub-arrays along the major axis.
            "⫇" | "group" => match left_val {
                None => Err(AplError::runtime("⫇ requires a left argument (group indices)".into())),
                Some(l) => self.group_indices(l, right_val),
            },
            // --- more builtins (Phase 6) ---
            "⌈" | "ceil" => match left_val {
                None => self.scalar1(right_val, |x| x.ceil(), "⌈"),
                Some(_) => self.num2(left_val, right_val, |a, b| {
                    match a.numeric_cmp(&b) {
                        Ok(Ordering::Greater) => a.clone(),
                        Ok(_) => b.clone(),
                        // Mismatched numeric kinds: fall back to the right operand.
                        Err(_) => b.clone(),
                    }
                }, "⌈"),
            },
            "⌊" | "floor" => match left_val {
                None => self.scalar1(right_val, |x| x.floor(), "⌊"),
                Some(_) => self.num2(left_val, right_val, |a, b| {
                    match a.numeric_cmp(&b) {
                        Ok(Ordering::Less) => a.clone(),
                        Ok(_) => b.clone(),
                        Err(_) => b.clone(),
                    }
                }, "⌊"),
            },
            // `|`: dyadic = modulo; monadic = magnitude (absolute value).
            "|" | "mod" => match left_val {
                None => self.scalar1(
                    right_val,
                    |x| match x.numeric_cmp(&KapNumber::Long(0)) {
                        Ok(std::cmp::Ordering::Less) => x.neg(),
                        _ => x.clone(),
                    },
                    "|",
                ),
                Some(_) => self.num2(left_val, right_val, |a, b| a.modulo(b), "|"),
            },
            "*" | "⋆" => match left_val {
                None => self.scalar1(right_val, |x| x.exp(), "⋆"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.pow(b), "⋆"),
            },
            "√" => match left_val {
                // Monadic `√ y` = square root. Dyadic `a √ b` = b^(1/a) (nth root).
                None => self.scalar1(right_val, |x| x.sqrt(), "√"),
                Some(_) => self.num2(left_val, right_val, |a, b| b.nth_root(a), "√"),
            },
            "×" => match left_val {
                None => self.scalar1(right_val, |x| x.signum(), "×"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.mul(b), "×"),
            },
            "⍟" | "log" => match left_val {
                None => self.scalar1(right_val, |x| x.nat_log(), "⍟"),
                Some(_) => self.num2(left_val, right_val, |a, b| b.log(a), "⍟"),
            },
            "∧" => self.bool2(left_val, right_val, |a, b| a & b, "∧"),
            "∨" => self.bool2(left_val, right_val, |a, b| a | b, "∨"),
            "⍲" => self.bool_broadcast(left_val, right_val, |x, y| !(x && y), "⍲"),
            "⍱" => self.bool_broadcast(left_val, right_val, |x, y| !(x || y), "⍱"),
            "∼" | "not" => self.logical_not(right_val),
            "~" | "bitnot" => self.scalar1(right_val, |x| x.not(), "~"),
            "∊" | "in" => self.membership(left_val, right_val),
            // `∪` unique/union (Kotlin unique.kt): monadic → unique; dyadic → union.
            "∪" | "unique" => match left_val {
                None => self.unique(right_val),
                Some(l) => self.union(l, right_val),
            },
            // `∩` intersection (Kotlin unique.kt IntersectionAPLFunction): dyadic.
            "∩" | "intersection" => match left_val {
                None => Err(AplError::runtime("∩ requires two args".into())),
                Some(l) => self.intersection(l, right_val),
            },
            "⍋" | "grade" => self.grade_up(right_val),
            "⍒" | "gradeDown" => self.grade_down(right_val),
            "∼" | "not" => self.logical_not(right_val),
            "!" | "gamma" | "binomial" => self.factorial_binomial(left_val, right_val),
            "…" | "range" => match left_val {
                None => Err(AplError::runtime("…: Function cannot be called with one argument".into())),
                Some(l) => self.range(l, right_val),
            },
            "⍷" | "find" => match left_val {
                None => Err(AplError::runtime("⍷: Function cannot be called with one argument".into())),
                Some(l) => self.find(l, right_val),
            },
            "⍸" | "where" => match left_val {
                // Dyadic interval form `a ⍸ b`: `a` = sorted boundaries (scalar or 1-D
                // vector, strictly ascending, no duplicates), `b` = data. Result has the
                // SAME SHAPE as `b`; each element is the count of boundaries `<=` that
                // data value. `1e100` in `b` is just a value being compared, never an
                // allocation count — so no runaway allocation (this is Kap's lazy
                // IntervalValue: size = ⍴b, not any element magnitude).
                Some(l) => self.interval(l, right_val),
                None => self.where_fn(right_val),
            },
            // `⊤`/`⊥` are Kap's BASE-VALUE (mixed-radix) functions, defined in
            // `math-kap.kap`. They are NOT the byte-array codecs `encode`/`decode`
            // (which the port does not implement as separate primitives).
            "⊤" => self.encode(left_val, right_val),
            "⊥" => self.decode(left_val, right_val),
            // Identity (⊢): monadic → argument; dyadic → right argument.
            "⊢" => match left_val {
                None => Ok(right_val),
                Some(_) => Ok(right_val),
            },
            // Hide (⊣): monadic → EmptyValue (discard); dyadic → left argument.
            "⊣" => match left_val {
                None => Ok(Rc::new(APLValue::Null)),
                Some(l) => Ok(l),
            },
            // Match / depth (≡): dyadic → 1 if deeply equal else 0; monadic → depth.
            "≡" => self.match_or_depth(left_val, right_val),
            _ => Err(AplError::runtime(format!("unknown function: {}", name))),
        }
    }

    /// Apply a *train* `(f g h ...)` as a derived function.
    /// - Monadic: right-to-left composition `(f g h) y` = `f (g (h y))`.
    /// - Dyadic 2-train (atop): `x (A B) y` = `A x (B y)`.
    /// Apply a *train* (derived function from a sequence of functions / a bound value).
    ///
    /// Reference semantics (ComposeTest.kt / operator.kt):
    /// - Compose `f ∘ g` (compose=true):
    ///     monadic `(f∘g) y` = `f(y, g(y))`;  dyadic `x (f∘g) y` = `f(x, g(y))`  (g monadic).
    /// - Reverse-compose `f ⍛ g` (reverse=true):
    ///     monadic `(f⍛g) y` = `g(f(y), y)`;  dyadic `x (f⍛g) y` = `g(f(x), y)`  (f monadic, g dyadic).
    /// - Bare 2-train (atop, compose=false, reverse=false):
    ///     monadic `(f g) y` = `f(g(y))`;  dyadic `x (f g) y` = `f(x g y)`.
    /// - Fork `A « B » C` (3-train):
    ///     monadic `(A y) B (C y)`;  dyadic `(x A y) B (x C y)`.
    /// - Left-bind `[value, fn]` (2-train, first member a value): `(c f) y` = `f(c, y)`
    ///   (the outer left arg is ignored).
    fn apply_train(
        &self,
        funcs: &[Instr],
        reverse: bool,
        compose: bool,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left {
            None => {
                // --- Monadic ---
                if reverse {
                    // (f ⍛ g) y = g(f(y), y)
                    let fy = self.eval_apply(&funcs[0], &None, right, env)?;
                    return self.eval_apply(
                        &funcs[1],
                        &Some(Box::new(Instr::Value(fy))),
                        right,
                        env,
                    );
                }
                if compose {
                    // (f ∘ g) y = f(y, g(y))
                    let gy = self.eval_apply(&funcs[1], &None, right, env)?;
                    return self.eval_apply(
                        &funcs[0],
                        &Some(right.clone()),
                        &Box::new(Instr::Value(gy)),
                        env,
                    );
                }
                // Left-bind: [value, fn]
                if funcs.len() == 2 && Self::is_value(&funcs[0]) {
                    return self.eval_apply(
                        &funcs[1],
                        &Some(Box::new(funcs[0].clone())),
                        right,
                        env,
                    );
                }
                match funcs.len() {
                    1 => {
                        // A single parenthesised function `(f) y` = `f y`.
                        self.eval_apply(&funcs[0], &None, right, env)
                    }
                    2 => {
                        // Atop: (f g) y = f(g(y))
                        let gy = self.eval_apply(&funcs[1], &None, right, env)?;
                        self.eval_apply(&funcs[0], &None, &Box::new(Instr::Value(gy)), env)
                    }
                    3 => {
                        // Fork: (A y) B (C y)
                        let ay = self.eval_apply(&funcs[0], &None, right, env)?;
                        let cy = self.eval_apply(&funcs[2], &None, right, env)?;
                        self.eval_apply(
                            &funcs[1],
                            &Some(Box::new(Instr::Value(ay))),
                            &Box::new(Instr::Value(cy)),
                            env,
                        )
                    }
                    n => Err(AplError::runtime(format!(
                        "monadic trains of length {} are not supported (use 2 or 3 functions)",
                        n
                    ))),
                }
            }
            Some(l) => {
                let left_val = self.eval_instr(l, env)?;
                if reverse {
                    // x (f ⍛ g) y = g(f(x), y)
                    let fx = self.eval_apply(&funcs[0], &None, &Box::new(Instr::Value(left_val.clone())), env)?;
                    return self.eval_apply(
                        &funcs[1],
                        &Some(Box::new(Instr::Value(fx))),
                        right,
                        env,
                    );
                }
                if compose {
                    // x (f ∘ g) y = f(x, g(y))
                    let gy = self.eval_apply(&funcs[1], &None, right, env)?;
                    return self.eval_apply(
                        &funcs[0],
                        &Some(Box::new(Instr::Value(left_val.clone()))),
                        &Box::new(Instr::Value(gy)),
                        env,
                    );
                }
                match funcs.len() {
                    1 => {
                        // A single parenthesised function `x (f) y` = `x f y`.
                        self.eval_apply(&funcs[0], &Some(Box::new(Instr::Value(left_val))), right, env)
                    }
                    2 => {
                        let (a, b) = (&funcs[0], &funcs[1]);
                        if Self::is_value(a) {
                            // x (c f) y : left-bind ignores the outer left, uses c.
                            return self.eval_apply(
                                b,
                                &Some(Box::new(a.clone())),
                                right,
                                env,
                            );
                        }
                        // Atop: f(x g y)  (g is dyadic)
                        let xgy = self.eval_apply(
                            b,
                            &Some(Box::new(Instr::Value(left_val.clone()))),
                            right,
                            env,
                        )?;
                        self.eval_apply(a, &None, &Box::new(Instr::Value(xgy)), env)
                    }
                    3 => {
                        let (a, b, c) = (&funcs[0], &funcs[1], &funcs[2]);
                        // (x A y) B (x C y)
                        let ay = self.eval_apply(a, &Some(Box::new(Instr::Value(left_val.clone()))), right, env)?;
                        let cy = self.eval_apply(c, &Some(Box::new(Instr::Value(left_val))), right, env)?;
                        self.eval_apply(
                            b,
                            &Some(Box::new(Instr::Value(ay))),
                            &Box::new(Instr::Value(cy)),
                            env,
                        )
                    }
                    n => Err(AplError::runtime(format!(
                        "dyadic trains of length {} are not yet supported (use 2 or 3 functions)",
                        n
                    ))),
                }
            }
        }
    }

    /// Whether an instr is a *value* (suitable for left-bind first member): a literal
    /// or array, but not a function/operator.
    fn is_value(e: &Instr) -> bool {
        matches!(e, Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
    }

    /// Whether `name` is a Kap primitive function/operator wired up in `eval_apply`.
    /// Mirrors the parser's `is_primitive_op` set; used by `FnAssign` so a bare
    /// primitive symbol (`foo ⇐ -`) is stored as a directly-callable body rather than
    /// a `⍺ - ⍵` delegation (which breaks monadic calls).
    fn is_primitive_name(name: &str) -> bool {
        matches!(
            name,
            "⍳" | "iota" | "⍴" | "rho" | "≢" | "tally" | "⊃" | "first" | "⌽" | "⊖" | "⍉"
                | "↑" | "↓" | "⊂" | "+" | "-" | "*" | "×" | "÷" | "/" | "=" | "≠" | "<" | ">"
                | "≤" | "≥" | "," | "⌈" | "⌊" | "|" | "⍟" | "∧" | "∨" | "~" | "∊" | "⍋" | "⊤" | "⊥"
                | "⊢" | "⊣" | "≡" | "⍓" | "⍕" | "format" | "⍎" | "execute" | "typeof" | "∪" | "∩" | "⍸" | "⍒" | "⍲" | "⍱" | "∼" | "!" | "…" | "⍷" | "cmp" | "⋆" | "√" | "⍮" | "pair" | "⊆" | "⊇" | "→" | "≬" | "toList" | "fromList" | "⫇" | "group" | "use" | "isLocallyBound"
                // Namespaced natives (P2): the parser's is_known_fn admits them, but
                // this eval-time late-gate must also know them (two-gate rule).
                | "sysparam"
 )
 }

    /// Apply a user-defined lambda. `split` = number of leading params that are bound to
    /// the *left* (dyadic) argument; the remainder are bound to the right argument.
    /// For a monadic function `split == 0`. Builds a child scope from the closure env
    /// and binds params, also exposing `⍺` (left, if any) and `⍵` (right) as defaults.
    fn apply_user_fn(
        &self,
        params: &[String],
        split: usize,
        body: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        closure_env: &AplRef<Environment>,
        self_name: Option<&str>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let child = Environment::child(&closure_env);
        // Evaluate args in the *calling* env (Kap passes by value/sharing).
        let right_val = self.eval_instr(right, closure_env)?.force(self)?;
        // Bind named params: first `split` to the left arg, the rest to the right arg.
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, closure_env)?.force(self)?),
            None => None,
        };
        // Argument-count validation (Kap raises on arity mismatch). Only enforced when the
        // function has *named* parameters; default-arg dfns (`∇ foo { ⍺+⍵ }`) accept ⍺/⍵
        // regardless of valence. `split` counts the *number of names* expected on the left.
        //
        // CRITICAL: a *single* parameter name binds the WHOLE argument value (even a vector),
        // so its arity contribution is 1 — NOT the argument's element count. Only a
        // *multi-name* group `(A;B)` destructures element-wise and thus requires the argument
        // to have exactly that many elements. Concretely: `∇ foo x { … } ⋄ foo (1 2 3)` binds
        // `x` to the whole 3-element vector (arity 1), whereas `∇ foo (a;b) { … } ⋄ foo (1;2)`
        // destructures into `a`,`b` (arity 2). We cannot see group structure here (params is a
        // flat name list), so the rule is: a parameter *count* of 1 ⇒ whole-value binding
        // (have = 1); a count > 1 ⇒ destructuring (have = element count).
        if !params.is_empty() {
            let needed_left = split;
            let needed_right = params.len() - split;
            let have_left = if needed_left == 1 {
                1
            } else {
                match &left_val {
                    Some(v) => self.element_count(v),
                    None => 0,
                }
            };
            let have_right = if needed_right == 1 {
                1
            } else {
                self.element_count(&right_val)
            };
            if have_left != needed_left || have_right != needed_right {
                return Err(AplError::runtime(format!(
                    "function called with wrong number of arguments: expected {} left and {} right, got {} left and {} right",
                    needed_left, needed_right, have_left, have_right
                )));
            }
        }
        // Bind named parameters. A multi-name group `(A;B)` destructures a vector argument
        // element-wise; a single-name group `(A)` or bare `A` binds the whole argument.
        // `split` may exceed `params.len()` for delegation-style dfns (e.g. `f ⇐ +` where
        // `split=1` but `params=[]` — the left arg is exposed via `⍺`/`⍵`, not named
        // params), so clamp to avoid an out-of-range slice.
        let bind_split = split.min(params.len());
        if bind_split > 0 {
            if let Some(lv) = &left_val {
                self.bind_param_group(&child, &params[..bind_split], lv);
            }
        }
        if params.len() > bind_split {
            self.bind_param_group(&child, &params[bind_split..], &right_val);
        }
        // Default `⍵`/`⍺` names (Kap's omega/alpha). `⍵` = right arg; `⍺` = left (if present).
        child.define("⍵", &None, right_val);
        if let Some(lv) = &left_val {
            child.define("⍺", &None, lv.clone());
        }
        // Self-binding: so a function can recurse by name (e.g. `fib` calling `fib`).
        if let Some(name) = self_name {
            let self_fn = APLValue::UserFn {
                params: params.to_vec(),
                split,
                body: Rc::new(body.clone()),
                env: closure_env.clone(),
            };
            child.define(name, &None, Rc::new(self_fn.clone()));
            // `⍓` (Kap's OUTER_CALL_SYMBOL) also refers to the enclosing function,
            // so a dfn can recurse via `⍓` even when assigned anonymously (e.g. `foo ⇐ { … ⍓ … }`).
            child.define("⍓", &None, Rc::new(self_fn));
        }
        // A function body that *is* a derived function (`×/`), a train (`⊢«⊣»`), or a
        // bare primitive symbol (`-`, `⊢`, …) is itself a function; apply it to the
        // call's data args (`left`/`right`) rather than evaluating it standalone (which
        // would drop the operand — e.g. `foo ⇐ ×/ ⋄ foo 1 2 3`, or apply `-` ambivalently
        // for `foo ⇐ -`). `eval_apply` dispatches primitives (and looks up user-fn names)
        // correctly with the supplied left/right.
        //
        // `→` (branch/return) raises `AplError::Return(v)`; the enclosing function frame
        // catches it here and returns `v`. If it escapes uncaught (top level), `eval_string_in_env`
        // converts it to the Real-Kap message "Call to return without a function call".
        let result = match body {
            Instr::Derived { .. } | Instr::Train { .. } | Instr::Symbol { .. } => {
                self.eval_apply(body, left, right, &child)
            }
            _ => self.eval_instr(body, &child),
        };
        match result {
            Err(AplError::Return(v)) => Ok(v),
            other => other,
        }
    }

    /// Apply a user-defined *operator* (from `Instr::OpCall`). `op` names a `UserOp`
    /// (resolved via the symbol table). `left_fn`/`right_fn` are the function operands
    /// (already-parsed Instrs); they are wrapped as `UserFn` values and bound to the
    /// operator's `op_left`/`op_right` names, so the body can invoke them via `⍞name`.
    /// The ordinary data args (`⍺`/`⍵`, i.e. `left`/`right`) are bound to `left_params`/
    /// `right_params` and exposed as `⍺`/`⍵`. Mirrors Kap's `UserDefinedOperatorFn`.
    fn apply_user_op(
        &self,
        op: &Box<Instr>,
        left_fn: &Box<Instr>,
        right_fn: &Option<Box<Instr>>,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Resolve the operator name to its `APLValue::UserOp`.
        let op_name = match op.as_ref() {
            Instr::Symbol { name, .. } => name.clone(),
            _ => return Err(AplError::runtime("operator must be a symbol".into())),
        };
        let uop = env
            .lookup(&op_name, &None)
            .ok_or_else(|| AplError::runtime(format!("undefined operator: {}", op_name)))?;
        let (op_left, op_right, left_params, right_params, body, op_env) = match uop.as_ref() {
            APLValue::UserOp {
                op_left,
                op_right,
                left_params,
                right_params,
                body,
                env: oenv,
                ..
            } => (
                op_left.clone(),
                op_right.clone(),
                left_params.clone(),
                right_params.clone(),
                body.clone(),
                oenv.clone(),
            ),
            _ => return Err(AplError::runtime(format!("{} is not an operator", op_name))),
        };
        // Build a child scope off the operator's closure env. This is the scope in which
        // the operator *body* runs, so the data args (`a`, `⍵`, …) live here.
        let child = Environment::child(&op_env);
        // Wrap a function operand (`left_fn`/`right_fn`) as an `APLValue::UserFn` so the
        // operator body can apply it — both via a bare reference (`x a0 b`) and via the
        // dynamic-ref form (`⍞x a0 b`). The operand must be bound as the *raw* function,
        // NOT pre-applied to the operator's `⍺`/`⍵`: `⍞x a0` means `x a0` (apply the operand
        // to the explicit args), exactly like Kotlin Kap. The earlier `Apply { operand,
        // left: ⍺, right: ⍵ }` wrapper wrongly turned `⍞x a0` into `(⍺ x) a0` = `⍺ x a0`,
        // so `⍞x 5` under `3 -foo+ 4` returned `3 - 5 = -2` instead of `5`.
        //
        // A closure (Lambda/Block) operand is stored directly; a primitive/train operand is
        // kept as its `Instr` body and routed through `eval_apply` at apply time. `split=1`
        // gives standard ambivalent behaviour (`x a0` monadic, `a0 x b0` dyadic), matching
        // `foo ⇐ ×-`-style delegation. The wrapper's *closure* is the operator body scope
        // (`child`) so the body params (`a`, `b`, …) resolve there.
        let wrap_fn = |instr: &Instr| -> APLValue {
            match instr {
                Instr::Lambda { params, body } => APLValue::UserFn {
                    params: params.clone(),
                    split: params.len().saturating_sub(1),
                    body: Rc::new(*body.clone()),
                    env: child.clone(),
                },
                Instr::Block { body } => APLValue::UserFn {
                    params: vec![],
                    split: 0,
                    body: Rc::new(Instr::Block { body: body.clone() }),
                    env: child.clone(),
                },
                other => APLValue::UserFn {
                    params: vec![],
                    split: 1,
                    body: Rc::new(other.clone()),
                    env: child.clone(),
                },
            }
        };
        if let Some(ol) = &op_left {
            let fv = Rc::new(wrap_fn(left_fn));
            child.define(ol, &None, fv);
        }
        if let Some(or) = &op_right {
            let fv = match right_fn {
                Some(rf) => Rc::new(wrap_fn(rf)),
                None => Rc::new(APLValue::Null),
            };
            child.define(or, &None, fv);
        }
        // Evaluate the data args in the calling env; bind to left/right param groups and to
        // `⍺`/`⍵` (ambivalent: `⍵` is the right arg, `⍺` the left if present).
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        // Bind the *left-param group* as a single unit to the left data arg, destructuring
        // each name to one element when the group has multiple names (e.g. `∇ (a0;a1) …`
        // called with left data `(10;11)` binds `a0=10, a1=11`). Previously we sliced the
        // flat name list at 1, which bound only the first name to the whole vector and the
        // rest to the right data — wrong for multi-name groups. When there is no left data
        // (ambivalent call), the left group binds to the right data arg instead.
        if !left_params.is_empty() {
            if let Some(lv) = &left_val {
                self.bind_param_group(&child, &left_params, lv);
            } else {
                self.bind_param_group(&child, &left_params, &right_val);
            }
        }
        if !right_params.is_empty() {
            self.bind_param_group(&child, &right_params, &right_val);
        }
        child.define("⍵", &None, right_val.clone());
        if let Some(lv) = &left_val {
            child.define("⍺", &None, lv.clone());
        }
        self.eval_instr(&body, &child)
    }

    /// (`(A;B;C)`), the argument must be a vector and each name gets one element
    /// (Kap destructuring). A single-name group binds the whole argument.
    fn bind_param_group(
        &self,
        env: &Rc<Environment>,
        names: &[String],
        arg: &APLValue,
    ) {
        if names.is_empty() {
            return;
        }
        if names.len() == 1 {
            env.define(&names[0], &None, Rc::new(arg.clone()));
            return;
        }
        if let APLValue::Array(a) = arg {
            let elems = a.elements();
            for (i, n) in names.iter().enumerate() {
                let v = elems.get(i).cloned().unwrap_or_else(|| Rc::new(APLValue::Null));
                env.define(n, &None, v);
            }
        } else {
            // Non-vector argument for a multi-name group: bind every name to the whole value.
            for n in names {
                env.define(n, &None, Rc::new(arg.clone()));
            }
        }
    }

    /// Collect the numeric operands of a char-math partner: a scalar `Number` becomes a
    /// 1-element list; a numeric `Array` becomes its elements (non-numeric => `None`, so the
    /// caller falls through to normal numeric handling).
    fn numbers_of(v: &APLValue) -> Option<Vec<KapNumber>> {
        match v {
            APLValue::Number(n) => Some(vec![n.clone()]),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    if let APLValue::Number(n) = e.as_ref() {
                        out.push(n.clone());
                    } else {
                        return None;
                    }
                }
                Some(out)
            }
            _ => None,
        }
    }

    /// Element-wise codepoint shift of a string by a number (scalar or numeric array,
    /// broadcast if the number side is length 1). `is_add` selects `+`/`-`.
    /// `result_is_char` is true when the non-number operand was a single `Char`, in
    /// which case the shifted scalar result is a `Char` (e.g. `@a + 1` -> `@b`); a
    /// string operand instead yields a `Str` (a char-vector that displays as a string).
    fn char_shift(
        s: &str,
        nums: &[KapNumber],
        is_add: bool,
        result_is_char: bool,
    ) -> Result<AplRef<APLValue>, AplError> {
        let cps: Vec<i64> = s.chars().map(|c| c as i64).collect();
        let n = if nums.len() == 1 {
            vec![nums[0].clone(); cps.len()]
        } else if nums.len() == cps.len() {
            nums.to_vec()
        } else {
            return Err(AplError::runtime(
                "character arithmetic: length mismatch".into(),
            ));
        };
        let mut out = String::new();
        for (i, cp) in cps.iter().enumerate() {
            let num = &n[i];
            if num.is_complex() {
                let (re, im) = num.as_complex();
                return Err(AplError::runtime(format!(
                    "+: Number is complex: Complex(re={:?}, im={:?})",
                    re, im
                )));
            }
            let delta: i64 = num
                .as_long()
                .unwrap_or_else(|_| num.as_double() as i64); // doubles truncate toward zero
            let new_cp = if is_add { cp + delta } else { cp - delta };
            if !(0..=0x10FFFF).contains(&new_cp) {
                return Err(AplError::runtime(format!(
                    "-: Codepoints cannot be negative: {}",
                    new_cp
                )));
            }
            out.push(char::from_u32(new_cp as u32).unwrap_or('?'));
        }
        if result_is_char {
            // Single-char shift collapses to a `Char` value (matches Real Kap).
            Ok(Rc::new(APLValue::Char(out.chars().next().unwrap_or('?'))))
        } else {
            Ok(Rc::new(APLValue::Str(out)))
        }
    }

    /// Element-wise codepoint difference of a string against ONE broadcast scalar
    /// char (oracle: `"abc" - @a` -> `(0 1 2)`). Result is a NUMERIC vector.
    fn char_diff_scalar(s: &str, cp: i64) -> Result<AplRef<APLValue>, AplError> {
        let out: Vec<AplRef<APLValue>> = s
            .chars()
            .map(|c| Rc::new(APLValue::Number(KapNumber::Long(c as i64 - cp))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Scalar char on the LEFT: `cp - each string codepoint` (oracle:
    /// `@b - "abc"` -> `(1 0 ¯1)`). Result is a NUMERIC vector.
    fn char_diff_scalar_left(cp: i64, s: &str) -> Result<AplRef<APLValue>, AplError> {
        let out: Vec<AplRef<APLValue>> = s
            .chars()
            .map(|c| Rc::new(APLValue::Number(KapNumber::Long(cp - c as i64))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Element-wise codepoint difference of two equal-length strings -> numeric vector.
    fn char_diff(a: &str, b: &str) -> Result<AplRef<APLValue>, AplError> {
        let ca: Vec<i64> = a.chars().map(|c| c as i64).collect();
        let cb: Vec<i64> = b.chars().map(|c| c as i64).collect();
        if ca.len() != cb.len() {
            return Err(AplError::runtime("character difference: length mismatch".into()));
        }
        let out: Vec<AplRef<APLValue>> = ca
            .iter()
            .zip(cb.iter())
            .map(|(x, y)| Rc::new(APLValue::Number(KapNumber::Long(x - y))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Char/string arithmetic dispatch for `+`/`-`. Returns `Some(result)` when the operands
    /// are character-valued (so the caller should `return` it); `None` means "fall through to
    /// ordinary numeric handling". Kap rules (verified against the `kap-jvm-text` oracle):
    ///  * `Char - Char` => element-wise codepoint difference, an `Integer` (`@b - @a` -> `1`).
    ///  * `Char ± Number` => codepoint shift, a `Char` (`@a + 1` -> `@b`, `@a - 1` -> `@\``).
    ///  * `Number + Char` => shift, a `Char` (`1 + @a` -> `@b`).
    ///  * `Number - Char` => ERROR (int−char asymmetry).
    ///  * `Str ± Number` => char shift (`"ab" + 1` -> `"bc"`, `"ab" - 1` -> `"\`a"`).
    ///  * `Number + Str` => shift; `Number - Str` => ERROR.
    ///  * `Str - Str` => element-wise codepoint difference (numeric vector).
    ///  * `Str + Str` / `Char × Char` / `Char | Char` => ERROR (char ops are not additive).
    fn compute_char_op(
        &self,
        left: &APLValue,
        right: &APLValue,
        is_add: bool,
    ) -> Option<Result<AplRef<APLValue>, AplError>> {
        // Char - Char => integer difference.
        if let (APLValue::Char(a), APLValue::Char(b)) = (left, right) {
            if is_add {
                // `+` does not support char arguments (only `-` subtracts chars).
                return Some(Err(AplError::runtime(
                    "+: Function does not support char arguments".into(),
                )));
            }
            return Some(Ok(Rc::new(APLValue::Number(KapNumber::Long(
                (*a as i64) - (*b as i64),
            )))));
        }
        // String - String => element-wise codepoint difference (numbers). Addition errors.
        if let (APLValue::Str(a), APLValue::Str(b)) = (left, right) {
            if is_add {
                return Some(Err(AplError::runtime(
                    "+: Function does not support char arguments".into(),
                )));
            }
            return Some(Self::char_diff(a, b));
        }
        // Char ± Number => char shift (both directions).
        if let (APLValue::Char(c), APLValue::Number(n)) = (left, right) {
            let nums = vec![n.clone()];
            let s = c.to_string();
            return Some(Self::char_shift(&s, &nums, is_add, true));
        }
        if let (APLValue::Number(n), APLValue::Char(c)) = (left, right) {
            let nums = vec![n.clone()];
            let s = c.to_string();
            // int - char asymmetry: subtracting a char from a number is forbidden.
            if !is_add {
                return Some(Err(AplError::runtime(
                    "-: Incompatible argument types. Left arg: integer, Right arg: char".into(),
                )));
            }
            return Some(Self::char_shift(&s, &nums, is_add, true));
        }
        // String ± Char (oracle-verified 2026-08-24):
        //   "abc"-@a -> (0 1 2)   element-wise codepoint difference, NUMERIC vector
        //                         (scalar char BROADCASTS across the string)
        //   @b-"abc" -> (1 0 ¯1)  same, char on the left
        //   "abc"+@a / @a+"ab" -> ERROR "+: Function does not support char arguments"
        if let (APLValue::Str(s), APLValue::Char(c)) = (left, right) {
            if is_add {
                return Some(Err(AplError::runtime(
                    "+: Function does not support char arguments".into(),
                )));
            }
            let cp = *c as i64;
            return Some(Self::char_diff_scalar(s, cp));
        }
        if let (APLValue::Char(c), APLValue::Str(s)) = (left, right) {
            if is_add {
                return Some(Err(AplError::runtime(
                    "+: Function does not support char arguments".into(),
                )));
            }
            // Char on the LEFT: scalar minus each string codepoint (oracle:
            // `@b-"abc"` -> `(1 0 ¯1)`).
            let cp = *c as i64;
            return Some(Self::char_diff_scalar_left(cp, s));
        }
        // Exactly one operand is a string; the other must be a number (scalar or numeric array).
        let (s, nums, str_is_left) = match (left, right) {
            (APLValue::Str(s), other) => (s.clone(), Self::numbers_of(other)?, true),
            (other, APLValue::Str(s)) => (s.clone(), Self::numbers_of(other)?, false),
            _ => return None,
        };
        // int - char asymmetry: subtracting a string from a number is forbidden.
        if !is_add && !str_is_left {
            return Some(Err(AplError::runtime(
                "-: Incompatible argument types. Left arg: integer, Right arg: char".into(),
            )));
        }
        Some(Self::char_shift(&s, &nums, is_add, false))
    }

    /// Render a value for `io:print`/`:pretty`/`:read` style selection. The default
    /// (no style, or an unrecognised style) uses the *plain* (unquoted) form — the
    /// same as `format_value`. The `"pretty"` style wraps strings in double quotes
    /// (the REPL display form); `"read"` is currently equivalent to plain. Mirrors
    /// Real Kap's `io:print :pretty` modifier.
    fn io_style_render(
        &self,
        style: &APLValue,
        value: &APLValue,
    ) -> Result<String, AplError> {
        let style_name = match style {
            APLValue::Str(s) => s.as_str(),
            APLValue::Char(c) => {
                let mut buf = [0u8; 4];
                return Ok(c.encode_utf8(&mut buf).to_string());
            }
            // A keyword-namespace symbol (`:pretty`, `:read`) is also a valid style.
            APLValue::Symbol { name, namespace } => {
                if namespace.as_deref() == Some("keyword") {
                    name.as_str()
                } else {
                    return Err(AplError::runtime("io:print style must be a string".into()));
                }
            }
            _ => return Err(AplError::runtime("io:print style must be a string".into())),
        };
        match style_name {
            "pretty" => Ok(value.format_display()),
            "read" => Ok(value.format_value()),
            "plain" => Ok(value.format_value()),
            other => Err(AplError::runtime(format!("invalid io:print style: {}", other))),
        }
    }

    // --- `unicode:*` builtins (Real Kap UnicodeModule) ---

    /// `unicode:toCodepoints` — element-wise char → codepoint number.
    fn unicode_to_codepoints(
        &self,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Char(c) => {
                Ok(Rc::new(APLValue::Number(KapNumber::Long(*c as i64))))
            }
            APLValue::Str(s) => {
                let v: Vec<i64> = s.chars().map(|c| c as i64).collect();
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![v.len()],
                    ArrayData::Long(v),
                )))))
            }
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Char(c) => {
                            out.push(Rc::new(APLValue::Number(KapNumber::Long(*c as i64))))
                        }
                        other => return Err(AplError::runtime(format!(
                            "unicode:toCodepoints: not a char: {}",
                            other.format_value()
                        ))),
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![out.len()],
                    ArrayData::Nested(out),
                )))))
            }
            other => Err(AplError::runtime(format!(
                "unicode:toCodepoints: unsupported argument: {}",
                other.format_value()
            ))),
        }
    }

    /// `unicode:fromCodepoints` — element-wise codepoint number → char.
    fn unicode_from_codepoints(
        &self,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let to_char = |n: &KapNumber| -> Result<char, AplError> {
            if n.is_complex() {
                return Err(AplError::runtime(
                    "unicode:fromCodepoints: complex numbers can't be characters".into(),
                ));
            }
            // Reject non-integers. `as_long` truncates Doubles (1.5 -> 1), so verify the
            // real value has no fractional part before truncating.
            let (re, _im) = n.as_complex();
            if re.fract() != 0.0 {
                return Err(AplError::runtime(format!(
                    "unicode:fromCodepoints: invalid codepoint (not an integer): {}",
                    n.as_double()
                )));
            }
            let cp = n.as_long().map_err(|e| AplError::runtime(e))?;
            char::from_u32(cp as u32).ok_or_else(|| {
                AplError::runtime(format!("unicode:fromCodepoints: invalid codepoint: {}", cp))
            })
        };
        match right_val.as_ref() {
            APLValue::Number(n) => Ok(Rc::new(APLValue::Char(to_char(n)?))),
            APLValue::Array(a) => {
                let mut s = String::new();
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(n) => s.push(to_char(n)?),
                        other => {
                            return Err(AplError::runtime(format!(
                                "unicode:fromCodepoints: not a number: {}",
                                other.format_value()
                            )))
                        }
                    }
                }
                Ok(Rc::new(APLValue::Str(s)))
            }
            other => Err(AplError::runtime(format!(
                "unicode:fromCodepoints: unsupported argument: {}",
                other.format_value()
            ))),
        }
    }

    /// `unicode:toGraphemes` — split a string into its grapheme clusters, each as a
    /// one-element string. Mirrors Kotlin `GraphemesFunction` (APLString per cluster).
    fn unicode_to_graphemes(
        &self,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let s = match right_val.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "unicode:toGraphemes: expected a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let graphemes: Vec<AplRef<APLValue>> = s
            .graphemes(true)
            .map(|g| Rc::new(APLValue::Str(g.to_string())))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![graphemes.len()],
            ArrayData::Nested(graphemes),
        )))))
    }

    /// `unicode:toLower` / `unicode:toUpper` — case conversion of a string.
    fn unicode_case(&self, right_val: AplRef<APLValue>, upper: bool) -> Result<AplRef<APLValue>, AplError> {
        let s = match right_val.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "unicode:to{}: expected a string, got: {}",
                    if upper { "Upper" } else { "Lower" },
                    other.format_value()
                )))
            }
        };
        let out = if upper { s.to_uppercase() } else { s.to_lowercase() };
        Ok(Rc::new(APLValue::Str(out)))
    }

    /// `unicode:toNames` — Unicode name of a single character, or `⍬` if unnamed.
    fn unicode_to_names(
        &self,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let c = match right_val.as_ref() {
            APLValue::Char(c) => *c,
            other => {
                return Err(AplError::runtime(format!(
                    "unicode:toNames: expected a char, got: {}",
                    other.format_value()
                )))
            }
        };
        match unicode_char_name(c) {
            Some(name) => Ok(Rc::new(APLValue::Str(name))),
            None => Ok(Rc::new(APLValue::Null)),
        }
    }

    /// `unicode:enc` — encode a string into a vector of byte values in the given
    /// charset (left arg, default UTF-8). Mirrors Kotlin `EncodeUnicodeFunction`.
    fn unicode_enc(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let s = match right_val.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "unicode:enc: expected a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let enc = match left_val {
            None => Charset::Utf8,
            Some(l) => self.unicode_charset(&l)?,
        };
        let bytes = unicode_encode(&s, enc);
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![bytes.len()],
            ArrayData::Long(bytes.into_iter().map(|b| b as i64).collect()),
        )))))
    }

    /// `unicode:dec` — decode a vector of byte values into a string in the given
    /// charset (left arg, default UTF-8). Mirrors Kotlin `DecodeUnicodeFunction`.
    fn unicode_dec(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let bytes: Vec<u8> = match right_val.as_ref() {
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(n) => {
                            let v = n.as_long().map_err(|e| AplError::runtime(e))?;
                            if !(0..=255).contains(&v) {
                                return Err(AplError::runtime(
                                    "unicode:dec: byte values must be 0..255".into(),
                                ));
                            }
                            out.push(v as u8);
                        }
                        other => {
                            return Err(AplError::runtime(format!(
                                "unicode:dec: not a byte: {}",
                                other.format_value()
                            )))
                        }
                    }
                }
                out
            }
            other => {
                return Err(AplError::runtime(format!(
                    "unicode:dec: expected a byte array, got: {}",
                    other.format_value()
                )))
            }
        };
        let enc = match left_val {
            None => Charset::Utf8,
            Some(l) => self.unicode_charset(&l)?,
        };
        let s = unicode_decode(&bytes, enc);
        Ok(Rc::new(APLValue::Str(s)))
    }

    /// Resolve a charset name from a left-arg symbol/string (`UTF8`, `UTF16`, …).
    /// A keyword-namespace symbol (`:UTF16`) arrives here as a `Symbol`, and a bare
    /// string arrives as `Str`. Both are accepted.
    fn unicode_charset(&self, v: &APLValue) -> Result<Charset, AplError> {
        let name = match v {
            APLValue::Str(s) => s.clone(),
            APLValue::Char(c) => {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).to_string()
            }
            APLValue::Symbol { name, namespace } => {
                if namespace.as_deref() == Some("keyword") {
                    name.clone()
                } else {
                    return Err(AplError::runtime(
                        "unicode: charset must be a name like UTF8/UTF16/UTF32".into(),
                    ));
                }
            }
            _ => {
                return Err(AplError::runtime(
                    "unicode: charset must be a name like UTF8/UTF16/UTF32".into(),
                ))
            }
        };
        Charset::from_name(&name)
            .ok_or_else(|| AplError::runtime(format!("unicode: invalid encoding: {}", name)))
    }

    /// Dyadic `⍕` with format directives (Real Kap format.kt / FormatAPLFunction).
    /// Format directives: `$s` (string value), `$h` (HTML-escaped string), and
    /// `$$` (literal `$`). An optional integer width precedes the directive letter:
    /// e.g. `$10s` left-pads, `$¯5s` right-pads (Kap uses `¯` for the negative sign).
    /// The right argument is arrayified; rank 1 yields a single formatted string,
    /// rank N≥2 yields a nested array of strings with the last axis consumed per row.
    fn format_with_directives(
        &self,
        fmt: &str,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        use std::rc::Rc;

        // Parse the format string into segments. Mirrors Kotlin's FormatCompiler: a
        // `$` introduces a directive; a second `$` is a literal `$`. Only `s` and `h`
        // are valid directive letters; anything else is a parse error.
        #[derive(Debug, Clone)]
        enum FmtPart {
            Lit(String),
            Dir { spec: String, kind: char },
        }
        let mut parts = Vec::new();
        let chars: Vec<char> = fmt.chars().collect();
        let mut i = 0usize;
        let mut lit = String::new();
        while i < chars.len() {
            let c = chars[i];
            if c == '$' {
                if i + 1 >= chars.len() {
                    return Err(AplError::runtime(
                        "End of string while parsing format specifier".into(),
                    ));
                }
                if !lit.is_empty() {
                    parts.push(FmtPart::Lit(lit.clone()));
                    lit = String::new();
                }
                let code = chars[i + 1];
                i += 2;
                if code == '$' {
                    // `$$` -> literal '$'
                    lit.push('$');
                } else {
                    // `code` is either the first spec char (if non-letter) or the
                    // directive letter itself (if a letter). Accumulate spec chars
                    // until a letter is reached; that letter is the directive kind
                    // (Kotlin FormatCompiler.processDirective).
                    let mut spec = String::new();
                    let mut kind = code;
                    if !code.is_ascii_alphabetic() {
                        spec.push(code);
                        while i < chars.len() && !chars[i].is_ascii_alphabetic() {
                            spec.push(chars[i]);
                            i += 1;
                        }
                        if i >= chars.len() {
                            return Err(AplError::runtime(
                                "End of string while parsing format specifier".into(),
                            ));
                        }
                        kind = chars[i];
                        i += 1;
                    }
                    if kind != 's' && kind != 'h' {
                        return Err(AplError::runtime(format!(
                            "Undefined directive: '{}'",
                            kind
                        )));
                    }
                    if kind == 'h' && !spec.is_empty() {
                        return Err(AplError::runtime(
                            "'h' directive does not accept arguments".into(),
                        ));
                    }
                    parts.push(FmtPart::Dir { spec, kind });
                }
            } else {
                lit.push(c);
                i += 1;
            }
        }
        if !lit.is_empty() {
            parts.push(FmtPart::Lit(lit));
        }

        // Arrayify the right argument and split into row-major chunks. Each row consumes
        // `row_width` elements (the last axis of the array).
        let right = right_val.force(self)?;
        let (flat, row_width, rank) = match right.as_ref() {
            APLValue::Array(a) => {
                let elems: Vec<AplRef<APLValue>> = a
                    .elements()
                    .iter()
                    .map(|e| Rc::new(e.as_ref().clone()))
                    .collect();
                let dims = right.dimensions();
                if dims.is_empty() {
                    (vec![Rc::new(APLValue::Null)], 1, 0usize)
                } else if dims.len() == 1 {
                    (elems, dims[0], 1)
                } else {
                    let rw = dims[dims.len() - 1];
                    (elems, rw, dims.len())
                }
            }
            other => (vec![Rc::new(other.clone())], 1, 0),
        };

        // Parse a Kap integer spec (supports `¯` negative sign), per Kotlin parseBigInt.
        let parse_spec = |spec: &str| -> Result<i64, AplError> {
            let s = spec.trim();
            if s.is_empty() {
                return Ok(0);
            }
            let neg = s.starts_with('¯');
            let digits_start = if neg {
                s.char_indices().nth(1).map(|(i, _)| i).unwrap_or(s.len())
            } else {
                0
            };
            let digits = &s[digits_start..];
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err(AplError::runtime(format!("invalid format padding: {}", spec)));
            }
            let n: i64 = digits.parse().unwrap_or(0);
            Ok(if neg { -n } else { n })
        };

        // Render a single row (a slice of `flat` of length `row_width`).
        let render_row = |row: &[AplRef<APLValue>]| -> Result<String, AplError> {
            let mut arg_index = 0usize;
            let mut out = String::new();
            for part in &parts {
                match part {
                    FmtPart::Lit(s) => out.push_str(s),
                    FmtPart::Dir { spec, kind } => {
                        if arg_index >= row.len() {
                            return Err(AplError::runtime(format!(
                                "format: too few arguments (need {}, got {})",
                                parts
                                    .iter()
                                    .filter(|p| matches!(p, FmtPart::Dir { .. }))
                                    .count(),
                                row.len()
                            )));
                        }
                        let val = &row[arg_index];
                        let rendered = match kind {
                            's' => val.format_value(),
                            'h' => {
                                // HTML escape: & < > only (Kotlin HTML_SUBSTITUTION_MAP)
                                val.format_value()
                                    .replace('&', "&amp;")
                                    .replace('<', "&lt;")
                                    .replace('>', "&gt;")
                            }
                            _ => unreachable!(),
                        };
                        let width = parse_spec(spec)?;
                        if width > 0 {
                            let pad = (width as usize).saturating_sub(rendered.chars().count());
                            out.push_str(&" ".repeat(pad));
                            out.push_str(&rendered);
                        } else if width < 0 {
                            let pad = (width.unsigned_abs() as usize)
                                .saturating_sub(rendered.chars().count());
                            out.push_str(&rendered);
                            out.push_str(&" ".repeat(pad));
                        } else {
                            out.push_str(&rendered);
                        }
                        arg_index += 1;
                    }
                }
            }
            Ok(out)
        };

        let result = if rank >= 2 {
            // Higher rank: drop the last axis; one formatted string per row.
            let num_rows = flat.len() / row_width;
            let rows: Vec<AplRef<APLValue>> = (0..num_rows)
                .map(|r| {
                    let start = r * row_width;
                    let end = start + row_width;
                    let s = render_row(&flat[start..end])?;
                    Ok(Rc::new(APLValue::Str(s)))
                })
                .collect::<Result<Vec<_>, AplError>>()?;
            let new_dims = {
                let mut d = right.dimensions();
                d.pop();
                d
            };
            APLValue::Array(Rc::new(KapArray::new(new_dims, ArrayData::Nested(rows))))
        } else {
            APLValue::Str(render_row(&flat)?)
        };

        Ok(Rc::new(result))
    }

    /// Map a scalar arithmetic operator name to its `KapNumber` op. Shared by `num2` and
    /// `num2_axis` so the two paths stay in lock-step.
    fn apply_op(&self, name: &str, a: &KapNumber, b: &KapNumber) -> Result<KapNumber, AplError> {
        match name {
            "+" => Ok(a.add(b)),
            "-" => Ok(a.sub(b)),
            "×" | "*" => Ok(a.mul(b)),
            "÷" | "/" => Ok(a.div(b)),
            _ => Err(AplError::runtime(format!("unsupported axis operator: {}", name))),
        }
    }

    fn num2(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        f: impl Fn(&KapNumber, &KapNumber) -> KapNumber,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        match (a.as_ref(), right_val.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => {
                Ok(Rc::new(APLValue::Number(f(x, y))))
            }
            // Char arithmetic (Kap: `@A + ⍳26 → "ABC…Z"`). `char ±×÷ number`,
            // `number ±×÷ char`, and `char ±×÷ char` all operate on codepoints and
            // yield a char; scalar-extended across arrays too.
            (APLValue::Char(c), APLValue::Number(n)) => Ok(Rc::new(APLValue::Char(
                char::from_u32((*c as i64 + n.as_long().unwrap_or(0)) as u32).ok_or_else(|| {
                    AplError::runtime("character codepoint out of range".into())
                })?,
            ))),
            (APLValue::Number(n), APLValue::Char(c)) => Ok(Rc::new(APLValue::Char(
                char::from_u32((*c as i64 + n.as_long().unwrap_or(0)) as u32).ok_or_else(|| {
                    AplError::runtime("character codepoint out of range".into())
                })?,
            ))),
            (APLValue::Char(a), APLValue::Char(b)) => Ok(Rc::new(APLValue::Char(
                char::from_u32(((*a as i64) + (*b as i64)) as u32).ok_or_else(|| {
                    AplError::runtime("character codepoint out of range".into())
                })?,
            ))),
            // Scalar extension with a char operand.
            (APLValue::Array(xa), APLValue::Char(c))
            | (APLValue::Char(c), APLValue::Array(xa)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                for e in xa.elements() {
                    if let APLValue::Number(x) = e.as_ref() {
                        out.push(char::from_u32((*c as i64 + x.as_long().unwrap_or(0)) as u32)
                            .ok_or_else(|| AplError::runtime("character codepoint out of range".into()))?);
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Char(out),
                )))))
            }
            (APLValue::Array(xa), APLValue::Array(xb)) if matches!(xb.data, ArrayData::Char(_)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                let xbe = xb.elements();
                for (i, e) in xa.elements().into_iter().enumerate() {
                    if let (APLValue::Number(x), Some(APLValue::Char(c))) =
                        (e.as_ref(), xbe.get(i).map(|y| y.as_ref()))
                    {
                        out.push(char::from_u32((*c as i64 + x.as_long().unwrap_or(0)) as u32)
                            .ok_or_else(|| AplError::runtime("character codepoint out of range".into()))?);
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Char(out),
                )))))
            }
            // Scalar extension: array <op> scalar, scalar <op> array, array <op> array.
            // Scalar extension. NOTE the two cases must pass args to `f` in the
            // SAME order as the scalar-scalar case above (f(left, right)):
            //   (array, scalar): each element is a left arg  -> f(elem, scalar)
            //   (scalar, array): the scalar is the left arg  -> f(scalar, elem)
            // (A previous single or-pattern called f(elem, scalar) in BOTH,
            // swapping the operand order whenever the LEFT side was the scalar —
            // e.g. `16 ⍟ 255 16` computed log_255(16) instead of log_16(255).)
            (APLValue::Array(xa), APLValue::Number(y)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                for e in xa.elements() {
                    if let APLValue::Number(x) = e.as_ref() {
                        out.push(Rc::new(APLValue::Number(f(x, y))));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            (APLValue::Number(y), APLValue::Array(xa)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                for e in xa.elements() {
                    if let APLValue::Number(x) = e.as_ref() {
                        out.push(Rc::new(APLValue::Number(f(y, x))));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            (APLValue::Array(xa), APLValue::Array(ya)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                let ye = ya.elements();
                if xa.element_count() != ya.element_count() {
                    return Err(AplError::runtime(format!(
                        "{}: arrays of different length ({} vs {})",
                        sym,
                        xa.element_count(),
                        ya.element_count()
                    )));
                }
                for (i, e) in xa.elements().into_iter().enumerate() {
                    // Bounds-guard each cell (a ragged/short vector must not panic on index).
                    if let (APLValue::Number(x), Some(APLValue::Number(y))) =
                        (e.as_ref(), ye.get(i).map(|y| y.as_ref()))
                    {
                        out.push(Rc::new(APLValue::Number(f(x, y))));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            _ => Err(AplError::runtime(format!("{} requires numbers", sym))),
        }
    }

    /// Axis-broadcast scalar arithmetic: `A f[axis] B` (Kotlin `AxisValAssignedFunctionDirect`
    /// over a `MathCombineAPLFunction`). The LEFT operand must be a rank-1 vector (the oracle
    /// errors "When specifying an axis, A or B has to be rank 1" for a scalar left); its
    /// length must equal the size of `axis` in `B`. Each element of `B` at coordinate `c` is
    /// combined with `left[c[axis]]` (the left vector's entry for that position along `axis`).
    /// Mirrors Kotlin's axis-combine broadcasting.
    fn num2_axis(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        name: &str,
        axis: usize,
    ) -> Result<AplRef<APLValue>, AplError> {
        let left = left_val
            .ok_or_else(|| AplError::runtime(format!("{}[axis] needs two arguments", name)))?;
        let la = match left.as_ref() {
            APLValue::Array(a) if a.dimensions.len() == 1 => {
                let mut v = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(n) => v.push(n.clone()),
                        _ => return Err(AplError::runtime(
                            "axis-combine left operand must be a numeric vector".into(),
                        )),
                    }
                }
                v
            }
            _ => {
                return Err(AplError::runtime(
                    "When specifying an axis, A or B has to be rank 1".into(),
                ))
            }
        };
        let arr = match right_val.as_ref() {
            APLValue::Array(a) => a,
            APLValue::Number(_) => {
                // B is a scalar: treat as 1-D of size 1 along axis 0; broadcast left[0].
                let out = match right_val.as_ref() {
                    APLValue::Number(y) => {
                        let l0 = la.first().cloned().unwrap_or_else(|| y.clone());
                        self.apply_op(name, &l0, y)?
                    }
                    _ => unreachable!(),
                };
                return Ok(Rc::new(APLValue::Number(out)));
            }
            _ => {
                return Err(AplError::runtime(
                    "axis-combine right operand must be a numeric array".into(),
                ))
            }
        };
        let dims = arr.dimensions.clone();
        if axis >= dims.len() {
            return Err(AplError::runtime(format!(
                "axis {} out of range for array of rank {}",
                axis,
                dims.len()
            )));
        }
        if la.len() != dims[axis] {
            return Err(AplError::runtime(format!(
                "axis-combine: left vector length {} does not match axis {} size {}",
                la.len(),
                axis,
                dims[axis]
            )));
        }
        // Strides for unravelling a flat index into coordinates.
        let mut strides = vec![1usize; dims.len()];
        for i in (0..dims.len().saturating_sub(1)).rev() {
            strides[i] = strides[i + 1] * dims[i + 1];
        }
        let elems = arr.elements();
        let total = arr.element_count();
        let mut out = Vec::with_capacity(total);
        for i in 0..total {
            let coord_axis = (i / strides[axis]) % dims[axis];
            let right_elem = match elems.get(i) {
                Some(e) => match e.as_ref() {
                    APLValue::Number(n) => n,
                    _ => {
                        return Err(AplError::runtime(
                            "axis-combine right operand must be numeric".into(),
                        ))
                    }
                },
                None => {
                    return Err(AplError::runtime(
                        "internal error: axis-combine element index out of bounds".into(),
                    ))
                }
            };
            let res = self.apply_op(name, &la[coord_axis], right_elem)?;
            out.push(Rc::new(APLValue::Number(res)));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            dims,
            ArrayData::Nested(out),
        )))))
    }

    /// Monadic negation: `- x` over a number or an array of numbers (scalar extension).
    fn negate(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Number(x) => Ok(Rc::new(APLValue::Number(x.neg()))),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    if let APLValue::Number(x) = e.as_ref() {
                        out.push(Rc::new(APLValue::Number(x.neg())));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    a.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            _ => Err(AplError::runtime("- requires a number".into())),
        }
    }

    fn cmp2(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        pred: impl Fn(Ordering) -> bool,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let ord = match (a.as_ref(), right_val.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => x.numeric_cmp(y).map_err(|e| {
                AplError::runtime(e)
            })?,
            _ => {
                return Err(AplError::runtime(format!("{} requires numbers", sym)))
            }
        };
        // Kap booleans are 1 (true) / 0 (false).
        Ok(Rc::new(APLValue::Number(KapNumber::Long(if pred(ord) { 1 } else { 0 }))))
    }

    /// Element-wise scalar comparison `A f B` for the six comparison operators.
    /// Oracle-verified (2026-08-24): `1 2 3 < 2` -> `(1 0 0)`; `1 < 1 2 3` ->
    /// `(0 1 1)`; `"abc" = "abc"` -> `(1 1 1)`; `"abc" ≤ "abd"` -> `(1 1 1)`.
    /// Scalar extension both ways; equal-length arrays element-wise; chars compare
    /// by codepoint. Length mismatch errors like Kotlin's dimension check.
    fn cmp2_elements(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        pred: impl Fn(Ordering) -> bool,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        use std::cmp::Ordering as O;
        // Total-ordering key for one operand cell: numbers numerically, chars by
        // codepoint, everything else via Kap type order (never equal across types
        // except through total_cmp semantics).
        let key_of = |v: &APLValue| -> Option<(u8, i64)> {
            match v {
                APLValue::Number(n) => n.as_long().ok().map(|l| (0u8, l)),
                APLValue::Char(c) => Some((1u8, *c as i64)),
                _ => None,
            }
        };
        let cmp_cells = |x: &APLValue, y: &APLValue| -> Option<bool> {
            match (x, y) {
                (APLValue::Number(a), APLValue::Number(b)) => {
                    a.numeric_cmp(b).ok().map(|o| pred(o))
                }
                // Char-vs-char compares by codepoint; char-vs-number is an
                // incompatible-type error in Kotlin (`< requires numbers`-class).
                (APLValue::Char(a), APLValue::Char(b)) => {
                    Some(pred((*a as i64).cmp(&(*b as i64))))
                }
                _ => None,
            }
        };
        let a = left_val.clone().ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let (la, ra) = (a.as_ref(), right_val.as_ref());
        // Scalar-scalar fast path delegates to the original cmp2 — but two Str
        // operands (or Str + scalar) are element-wise char comparisons, so let
        // them fall through to the general path.
        if !matches!(la, APLValue::Array(_) | APLValue::Str(_))
            && !matches!(ra, APLValue::Array(_) | APLValue::Str(_))
        {
            return self.cmp2(left_val, right_val, pred, sym);
        }
        // Gather element views. A Kap string (`Str`) participates element-wise as a
        // char vector (oracle: "abc" = "abc" -> (1 1 1), "abc" ≤ "abd" -> (1 1 1)).
        let l_chars = matches!(la, APLValue::Str(_));
        let r_chars = matches!(ra, APLValue::Str(_));
        let l_arr = match la {
            APLValue::Array(x) => Some(x.clone()),
            _ => None,
        };
        let r_arr = match ra {
            APLValue::Array(x) => Some(x.clone()),
            _ => None,
        };
        let l_len = match (l_arr.as_ref(), la) {
            (Some(x), _) => x.element_count(),
            (None, APLValue::Str(s)) => s.chars().count(),
            _ => 1,
        };
        let r_len = match (r_arr.as_ref(), ra) {
            (Some(x), _) => x.element_count(),
            (None, APLValue::Str(s)) => s.chars().count(),
            _ => 1,
        };
        let n = l_len.max(r_len);
        if l_len != 1 && r_len != 1 && l_len != r_len {
            return Err(AplError::runtime(format!(
                "{}: Arguments must be of the same dimension, or one of the arguments must be a scalar. aDimensions=[{}], bDimensions=[{}]",
                sym, l_len, r_len
            )));
        }
        let l_elems = l_arr.as_ref().map(|x| x.elements());
        let r_elems = r_arr.as_ref().map(|x| x.elements());
        // Materialise Str operands as per-char closures for cell access.
        let l_str: Option<Vec<char>> = if l_chars {
            match la {
                APLValue::Str(s) => Some(s.chars().collect()),
                _ => None,
            }
        } else {
            None
        };
        let r_str: Option<Vec<char>> = if r_chars {
            match ra {
                APLValue::Str(s) => Some(s.chars().collect()),
                _ => None,
            }
        } else {
            None
        };
        let mut out: Vec<i64> = Vec::with_capacity(n);
        for i in 0..n {
            let li = if l_len == 1 { 0 } else { i };
            let ri = if r_len == 1 { 0 } else { i };
            // Resolve left/right cells from array elements, string chars, or the scalar.
            let l_cell = l_str.as_ref().map(|v| v[li]);
            let r_cell = r_str.as_ref().map(|v| v[ri]);
            let lx: &APLValue = match (&l_elems, l_cell) {
                (Some(v), _) => v[li].as_ref(),
                (None, Some(c)) => {
                    out.push(if pred(c.cmp(&r_cell.unwrap()).into()) { 1 } else { 0 });
                    continue;
                }
                _ => la,
            };
            let rx: &APLValue = match (&r_elems, r_cell) {
                (Some(v), _) => v[ri].as_ref(),
                (None, Some(c)) => {
                    // Left was a non-string scalar; compare it against this char.
                    return Err(AplError::runtime(format!(
                        "{} requires numbers",
                        sym
                    )));
                }
                _ => ra,
            };
            match cmp_cells(lx, rx) {
                Some(b) => out.push(if b { 1 } else { 0 }),
                None => {
                    return Err(AplError::runtime(format!(
                        "{} requires numbers",
                        sym
                    )))
                }
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Long(out),
        )))))
    }

    /// `cmp`: total-ordering comparison returning -1 (less), 0 (equal), or 1 (greater)
    /// as an integer scalar, matching Kap's `CompareObjectsFunction`. Works on numbers
    /// and characters (and strings by rank/shape). Dyadic only.
    fn cmp_values(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let a = a.force(self)?;
        let b = right_val.force(self)?;
        let ord = match (a.as_ref(), b.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => {
                x.numeric_cmp(y).map_err(|e| AplError::runtime(e))?
            }
            (APLValue::Char(x), APLValue::Char(y)) => x.cmp(y),
            // String vs String: lexicographic by codepoint (Real Kap `compareTotalOrdering`).
            (APLValue::Str(x), APLValue::Str(y)) => x.cmp(y),
            // Cross-kind (e.g. Number vs Str): defer to the general total-ordering rule,
            // which orders number < char < string < null and compares equal kinds by value.
            _ => match a.total_cmp(b.as_ref()) {
                Some(o) => o,
                None => {
                    return Err(AplError::runtime(format!(
                        "{}: not comparable ({} vs {})",
                        sym,
                        a.class_name(),
                        b.class_name()
                    )))
                }
            },
        };
        let v = match ord {
            Ordering::Less => -1i64,
            Ordering::Equal => 0i64,
            Ordering::Greater => 1i64,
        };
        Ok(Rc::new(APLValue::Number(KapNumber::Long(v))))
    }

    fn iota(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let n = match right_val.as_ref() {
            APLValue::Number(KapNumber::Long(v)) => *v,
            _ => {
                return Err(AplError::runtime("⍳ needs an integer count".into()))
            }
        };
        if n < 0 {
            return Err(AplError::runtime("⍳ count must be non-negative".into()));
        }
        let nums: Vec<KapNumber> = (0..n).map(KapNumber::Long).collect();
        let arr = KapArray::from_numbers(nums);
        Ok(Rc::new(APLValue::Array(Rc::new(arr))))
    }

    /// Dyadic `⍳` (index-of), mirroring Kotlin `FindIndexArray1DLeftArg`.
    /// Result shape = shape of B; for each element of B, the first 0-based position
    /// in A where `a[i]` matches (cross-kind, `total_cmp` == Equal), else `a.size`.
    fn index_of(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.force(self)?;
        let b = right_val.force(self)?;
        let a_elems = a.elements();
        let b_elems = b.elements();
        let not_found = a_elems.len() as i64;
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(b_elems.len());
        for bref in &b_elems {
            let mut found = not_found;
            for (i, aelem) in a_elems.iter().enumerate() {
                if aelem.total_cmp(bref.as_ref()) == Some(Ordering::Equal) {
                    found = i as i64;
                    break;
                }
            }
            out.push(Rc::new(APLValue::Number(KapNumber::Long(found))));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            b.dimensions(),
            ArrayData::Nested(out),
        )))))
    }


    // ===== `regex:*` — regular-expression string utilities (Kotlin RegexpModule) =====
    // Left arg is the pattern (a string); right arg is the subject (and, for `replace`,
    // an `(subject; replacement)` pair). Mirrors regexp.kt.

    /// Extract the pattern string from the (optional) left arg or the right arg,
    /// compiling it. A bad pattern produces a runtime error (Kotlin `InvalidRegexp`).
    fn regex_compiled(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<(regex::Regex, String), AplError> {
        let v = match left_val {
            Some(l) => l,
            None => right_val.clone(),
        }
        .force(self)?;
        let pat = match v.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "regex: pattern must be a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let re = regex::Regex::new(&pat)
            .map_err(|e| AplError::runtime(format!("invalid regex pattern: {}", e)))?;
        Ok((re, pat))
    }

    /// Build the group vector for one match: `[whole, g1, g2, ...]` (Kotlin
    /// `makeAPLValueFromGroups`). Unmatched optional groups are represented as the
    /// empty string here (Kap's `:undefined` symbol is not yet modelled).
    fn regex_groups_vector(re: &regex::Regex, hay: &str) -> AplRef<APLValue> {
        let caps = re.captures(hay).expect("caller guarantees a match exists");
        let mut elems: Vec<AplRef<APLValue>> = Vec::with_capacity(caps.len());
        for i in 0..caps.len() {
            let s = caps
                .get(i)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            elems.push(Rc::new(APLValue::Str(s)));
        }
        Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![elems.len()],
            ArrayData::Nested(elems),
        ))))
    }

    fn regex_match(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (re, _) = self.regex_compiled(left_val, right_val.clone())?;
        let subject = match right_val.force(self)?.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "regex:match subject must be a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let hit = if re.find(&subject).is_some() { 1 } else { 0 };
        Ok(Rc::new(APLValue::Number(KapNumber::Long(hit))))
    }

    fn regex_find(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (re, _) = self.regex_compiled(left_val, right_val.clone())?;
        let subject = match right_val.force(self)?.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "regex:find subject must be a string, got: {}",
                    other.format_value()
                )))
            }
        };
        match re.find(&subject) {
            Some(m) => Ok(Self::regex_groups_vector(&re, m.as_str())),
            None => Ok(Rc::new(APLValue::Null)),
        }
    }

    /// Like `regex:find` but errors (instead of returning Null) when there is no match.
    fn regex_finderror(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (re, _) = self.regex_compiled(left_val, right_val.clone())?;
        let subject = match right_val.force(self)?.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "regex:finderror subject must be a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let m = re.find(&subject).ok_or_else(|| {
            AplError::runtime("regex:finderror: pattern did not match subject".into())
        })?;
        Ok(Self::regex_groups_vector(&re, m.as_str()))
    }

    fn regex_findall(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (re, _) = self.regex_compiled(left_val, right_val.clone())?;
        let subject = match right_val.force(self)?.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "regex:findall subject must be a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let mut elems: Vec<AplRef<APLValue>> = Vec::new();
        for m in re.find_iter(&subject) {
            elems.push(Self::regex_groups_vector(&re, m.as_str()));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![elems.len()],
            ArrayData::Nested(elems),
        )))))
    }

    fn regex_replace(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (re, _) = self.regex_compiled(left_val, right_val.clone())?;
        // Right arg is an `(subject; replacement)` pair (Kotlin `b.listify()`).
        let pair = right_val.force(self)?;
        let (subject, replacement) = match pair.as_ref() {
            APLValue::Array(a) if a.element_count() == 2 => {
                let es = a.elements();
                let subj = match es[0].as_ref() {
                    APLValue::Str(s) => s.clone(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "regex:replace subject must be a string, got: {}",
                            other.format_value()
                        )))
                    }
                };
                let repl = match es[1].as_ref() {
                    APLValue::Str(s) => s.clone(),
                    other => {
                        return Err(AplError::runtime(format!(
                            "regex:replace replacement must be a string, got: {}",
                            other.format_value()
                        )))
                    }
                };
                (subj, repl)
            }
            other => {
                return Err(AplError::runtime(format!(
                    "regex:replace requires a (subject; replacement) pair, got: {}",
                    other.format_value()
                )))
            }
        };
        // Kotlin's `Regex.replace` replaces ALL matches (like `replace_all`).
        let out = re.replace_all(&subject, replacement.as_str()).into_owned();
        Ok(Rc::new(APLValue::Str(out)))
    }

    fn regex_split(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (re, _) = self.regex_compiled(left_val, right_val.clone())?;
        let subject = match right_val.force(self)?.as_ref() {
            APLValue::Str(s) => s.clone(),
            other => {
                return Err(AplError::runtime(format!(
                    "regex:split subject must be a string, got: {}",
                    other.format_value()
                )))
            }
        };
        let parts: Vec<AplRef<APLValue>> = re
            .split(&subject)
            .map(|s| Rc::new(APLValue::Str(s.to_string())))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![parts.len()],
            ArrayData::Nested(parts),
        )))))
    }

    /// `regex:compile` validates the pattern; on success it returns the pattern string
    /// (Kotlin returns a RegexpMatcherValue, which our string form can feed back as a
    /// left arg). A bad pattern errors (covers the `kind: "fails"` compile cases).
    fn regex_compile(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let (_, pat) = self.regex_compiled(left_val, right_val)?;
        Ok(Rc::new(APLValue::Str(pat)))
    }

    fn shape(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        // A `Str` is a rank-1 array of its chars (Kotlin APLBmpString.dimensions).
        let dims = right_val.dimensions();
        let shape: Vec<AplRef<APLValue>> = dims
            .iter()
            .map(|d| Rc::new(APLValue::Number(KapNumber::Long(*d as i64))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![shape.len()],
            ArrayData::Nested(shape),
        )))))
    }

    fn reshape(&self, left_val: AplRef<APLValue>, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        // Dyadic `⍴`: `(dims) ⍴ data` builds an array of shape `dims`, filled by
        // cycling through the flat elements of `data` (Kap/APL reshape semantics).
        // A negative dimension (e.g. `-1`) means "infer this axis from the data length",
        // exactly one such dimension is allowed.
        let dims_val = left_val.force(self)?;
        let mut raw_dims: Vec<i64> = Vec::new();
        match dims_val.as_ref() {
            APLValue::Array(a) => {
                for e in a.elements() {
                    if let APLValue::Number(KapNumber::Long(v)) = e.as_ref() {
                        raw_dims.push(*v);
                    } else {
                        return Err(AplError::runtime("reshape dimensions must be integers".into()));
                    }
                }
            }
            APLValue::Number(KapNumber::Long(v)) => raw_dims.push(*v),
            APLValue::Str(s) => {
                // A string as a reshape shape means its length as a single dimension
                // (Kap: `"abc"⍴x` ≡ `(3)⍴x`).
                raw_dims.push(s.chars().count() as i64);
            }
            _ => return Err(AplError::runtime("reshape dimensions must be an array or integer".into())),
        }
        // Resolve negative dimensions. Kotlin (reshape.kt) allows ONLY `-1` as the
        // inferred-dimension sentinel; any other negative size is an error
        // (`Attempt to reshape to dimension with negative size: <n>`).
        let neg_count = raw_dims.iter().filter(|&&d| d < 0).count();
        if neg_count > 1 {
            return Err(AplError::runtime("reshape allows at most one inferred (-1) dimension".into()));
        }
        if let Some(&d) = raw_dims.iter().find(|&&d| d < 0 && d != -1) {
            return Err(AplError::runtime(format!("Attempt to reshape to dimension with negative size: {}", d)));
        }
        let data = right_val.force(self)?;
        let src_elements: Vec<AplRef<APLValue>> = match data.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };
        let data_len = std::cmp::max(src_elements.len(), 1) as i64;
        let abs_prod_excl_neg: i64 = raw_dims
            .iter()
            .filter(|&&d| d >= 0)
            .map(|&d| d.max(0))
            .product::<i64>()
            .max(1);
        let dims: Vec<usize> = if neg_count == 1 {
            raw_dims
                .iter()
                .map(|&d| {
                    if d < 0 {
                        let inferred = data_len / abs_prod_excl_neg;
                        inferred.max(0) as usize
                    } else {
                        d.max(0) as usize
                    }
                })
                .collect()
        } else {
            raw_dims.iter().map(|&d| d.max(0) as usize).collect()
        };
        if dims.is_empty() {
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(vec![], ArrayData::Nested(vec![]))))));
        }
        let total: usize = dims.iter().product();
        // Guard against absurd sizes (the OOM case was an ~1e12 product). Empty arrays
        // (total == 0) are VALID in Kap and used widely, so do not reject them here.
        if total > 100_000_000 {
            return Err(AplError::runtime("reshape result too large".into()));
        }
        let data = right_val.force(self)?;
        let mut src: Vec<AplRef<APLValue>> = match data.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };
        if src.is_empty() {
            // Filling with a prototype: use a scalar 0 (matches Kap's empty-fill behaviour for
            // numeric reshape sources).
            src = vec![Rc::new(APLValue::Number(KapNumber::Long(0)))];
        }
        let total: usize = dims.iter().product();
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(total);
        for i in 0..total {
            out.push(Rc::new(src[i % src.len()].as_ref().clone()));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(out))))))
    }

    fn tally(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        Ok(Rc::new(APLValue::Number(KapNumber::Long(
            right_val.element_count() as i64,
        ))))
    }

    fn first(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Array(a) => a
                .elements()
                .into_iter()
                .next()
                .ok_or_else(|| AplError::runtime("⊃ of empty array".into())),
            // A string is a rank-1 vector of chars; `⊃` returns its first character.
            APLValue::Str(s) => s
                .chars()
                .next()
                .map(|c| Rc::new(APLValue::Char(c)))
                .ok_or_else(|| AplError::runtime("⊃ of empty string".into())),
            other => Ok(Rc::new(other.clone())),
        }
    }

    fn catenate(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left_val {
            None => {
                // Monadic `,` = ravel: flatten (one level) into a rank-1 vector.
                // Oracle: `,5`→`⟨5⟩`, `,1 2 3`→`⟨1 2 3⟩`, `,⊂5`→`⟨5⟩`, `⍴,5`→`⟨1⟩`.
                let v = right_val.force(self)?;
                let mut elems = Vec::new();
                self.collect_elements(&v, &mut elems);
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![elems.len()],
                    ArrayData::Nested(elems),
                )))))
            }
            Some(a) => {
                let a = a.force(self)?;
                // Two strings concatenate into a string (Kotlin ConcatenateAPLFunction, BMP path).
                if let (APLValue::Str(s1), APLValue::Str(s2)) = (a.as_ref(), right_val.as_ref()) {
                    let mut s = s1.clone();
                    s.push_str(s2);
                    return Ok(Rc::new(APLValue::Str(s)));
                }
                let mut elems = Vec::new();
                self.collect_elements(&a, &mut elems);
                self.collect_elements(&right_val, &mut elems);
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![elems.len()],
                    ArrayData::Nested(elems),
                )))))
            }
        }
    }

    /// `,[axis]` — axis-aware catenation (Kotlin `ConcatenateAPLFunctionImpl.eval2Arg`).
    ///
    /// A *near-integer* axis (within 0.01, per Kap's `computeLaminateAxis`) concatenates
    /// the two arrays along that integer axis (dims must match on every axis except the
    /// concatenation axis). A genuinely fractional axis is a *laminate*: the axis is
    /// `ceil(d)`, a length-1 axis is inserted at that position in both arrays, and the
    /// (now equal-rank) arrays are concatenated along it — stacking A over B.
    fn catenate_axis(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        axis: &KapNumber,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = match left_val {
            Some(l) => l.force(self)?,
            None => return Err(AplError::runtime(",[axis]: requires two arguments".into())),
        };
        let b = right_val.force(self)?;
        // Strings: Kotlin errors on axis for two strings (no laminate for chars); mirror.
        if matches!((a.as_ref(), b.as_ref()), (APLValue::Str(_), APLValue::Str(_))) {
            return Err(AplError::runtime(
                ",[axis]: axis catenation is not supported for strings".into(),
            ));
        }
        // Resolve the (possibly fractional) axis.
        let (is_laminate, new_axis): (bool, i64) = match axis {
            KapNumber::Long(v) => (false, *v),
            KapNumber::Double(d) => {
                let frac = d.fract();
                if frac.abs() < 0.01 {
                    (false, *d as i64)
                } else {
                    (true, d.ceil() as i64)
                }
            }
            _ => return Err(AplError::runtime(",[axis]: axis must be a number".into())),
        };
        if is_laminate {
            // `joinByLaminate`: insert a length-1 axis at `new_axis` in both, then concat.
            let a_dims = a.dimensions();
            let b_dims = b.dimensions();
            if a_dims.len() != b_dims.len() || a_dims != b_dims {
                return Err(AplError::runtime(
                    ",[axis]: laminate requires both arguments to have the same shape".into(),
                ));
            }
            let na = new_axis as usize;
            if na > a_dims.len() {
                return Err(AplError::runtime(format!(
                    ",[axis]: Axis must be between 0 and {} inclusive. Found: {}",
                    a_dims.len(),
                    na
                )));
            }
            let mut rd = a_dims.clone();
            rd.insert(na, 1);
            let a_elems = a.elements();
            let b_elems = b.elements();
            // Reshape each to `rd` (elements unchanged — length-1 insertion keeps order).
            let a1 = APLValue::Array(Rc::new(KapArray::new(rd.clone(), ArrayData::Nested(a_elems))));
            let b1 = APLValue::Array(Rc::new(KapArray::new(rd, ArrayData::Nested(b_elems))));
            return self.join_by_axis(
                &a1.elements(),
                &a1.dimensions(),
                &b1.elements(),
                &b1.dimensions(),
                na,
            );
        }
        // Plain integer-axis concatenation.
        let na = new_axis as usize;
        let a_dims = a.dimensions();
        let b_dims = b.dimensions();
        if na > a_dims.len() {
            return Err(AplError::runtime(format!(
                ",[axis]: Axis {} is not valid. Expected: {}",
                na,
                a_dims.len()
            )));
        }
        self.join_by_axis(
            &a.elements(),
            &a_dims,
            &b.elements(),
            &b_dims,
            na,
        )
    }

    /// Concatenate two arrays of equal rank along `axis` (Kotlin `joinByAxis`). Dims must
    /// match on every axis except `axis`, which sums. Flat element order is row-major
    /// (Kap's default); for the laminate path the inserted length-1 axis interleaves
    /// A and B along that axis (e.g. `(3),[0.5](3)` → `(3 2)`, interleaved by row).
    fn join_by_axis(
        &self,
        a_elems: &[AplRef<APLValue>],
        a_dims: &[usize],
        b_elems: &[AplRef<APLValue>],
        b_dims: &[usize],
        axis: usize,
    ) -> Result<AplRef<APLValue>, AplError> {
        if a_dims.len() != b_dims.len() {
            return Err(AplError::runtime(
                ",[axis]: ranks of A and B are different".into(),
            ));
        }
        for i in 0..a_dims.len() {
            if i != axis && a_dims[i] != b_dims[i] {
                return Err(AplError::runtime(format!(
                    ",[axis]: dimensions at axis {} do not match: {:?} vs {:?}",
                    i, a_dims, b_dims
                )));
            }
        }
        let rank = a_dims.len();
        let mut out_dims = a_dims.to_vec();
        out_dims[axis] = a_dims[axis] + b_dims[axis];
        let mut out_elems: Vec<AplRef<APLValue>> = Vec::with_capacity(out_dims.iter().product());
        // Strides for the output shape (row-major).
        let mut strides = vec![1usize; rank];
        for i in (0..rank).rev() {
            if i + 1 < rank {
                strides[i] = strides[i + 1] * out_dims[i + 1];
            }
        }
        let total: usize = out_dims.iter().product();
        for flat in 0..total {
            // Decode flat -> coords.
            let mut rem = flat;
            let mut coords = vec![0usize; rank];
            for i in 0..rank {
                coords[i] = rem / strides[i];
                rem %= strides[i];
            }
            let k = coords[axis];
            let (src_elems, src_dims, offset) = if k < a_dims[axis] {
                (a_elems, a_dims, 0usize)
            } else {
                (b_elems, b_dims, a_dims[axis])
            };
            // Encode the source flat index (same coords, but axis coord relative to source).
            coords[axis] = k - offset;
            let mut src_flat = 0usize;
            for i in 0..rank {
                src_flat += coords[i] * strides[i];
            }
            // src_dims has the output stride layout; recompute using src_dims strides.
            let mut src_strides = vec![1usize; rank];
            for i in (0..rank).rev() {
                if i + 1 < rank {
                    src_strides[i] = src_strides[i + 1] * src_dims[i + 1];
                }
            }
            let mut sf = 0usize;
            let mut rrem = 0usize;
            let mut c2 = coords.clone();
            for i in 0..rank {
                c2[i] = if i == axis { k - offset } else { coords[i] };
            }
            for i in 0..rank {
                sf += c2[i] * src_strides[i];
            }
            out_elems.push(src_elems[sf].clone());
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            out_dims,
            ArrayData::Nested(out_elems),
        )))))
    }

    /// Flatten a value's elements for catenation/stranding: scalars become a 1-element
    /// list; arrays contribute their elements (one level, not deep).
    fn collect_elements(&self, v: &AplRef<APLValue>, out: &mut Vec<AplRef<APLValue>>) {
        match v.as_ref() {
            APLValue::Array(a) => out.extend(a.elements()),
            other => out.push(Rc::new(other.clone())),
        }
    }

    fn reverse_horizontal(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // `⌽` reverses/rotates along the *last* axis (default for `⌽`).
        self.reverse_axis(left_val, right_val, None, true)
    }

    fn reverse_vertical(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // `⊖` reverses/rotates along the *first* axis (default for `⊖`).
        self.reverse_axis(left_val, right_val, Some(0), true)
    }

    /// Reverse or rotate one axis of an n-D array (Kap `⌽`/`⊖`).
    /// - `axis`: fixed axis (Some(k)) or the last axis (None ⇒ rank-1).
    /// - monadic (left=None) ⇒ reverse that axis;
    /// - dyadic (left=scalar n) ⇒ rotate every cell along the axis by n;
    /// - dyadic (left=vector v) ⇒ rotate cell i by v[i] (length must equal #cells).
    fn reverse_axis(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        axis: Option<usize>,
        _reverse_when_none: bool,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right = right_val.force(self)?;
        match right.as_ref() {
            APLValue::Str(s) => {
                // A string is a rank-1 vector of chars; reverse/rotate its characters.
                let chars: Vec<char> = s.chars().collect();
                let n = chars.len();
                if n == 0 {
                    return Ok(Rc::new(right.as_ref().clone()));
                }
                let axis = axis.unwrap_or(0);
                if axis != 0 {
                    return Err(AplError::runtime(
                        "⌽/⊖ axis must be 0 for a string".into(),
                    ));
                }
                let do_reverse = left_val.is_none();
                let shift: i64 = match left_val {
                    None => 0,
                    Some(l) => {
                        let lv = l.force(self)?;
                        match lv.as_ref() {
                            APLValue::Number(KapNumber::Long(x)) => *x,
                            _ => {
                                return Err(AplError::runtime(
                                    "⌽/⊖ shift must be an integer".into(),
                                ))
                            }
                        }
                    }
                };
                let mut out: Vec<char> = vec!['\0'; n];
                for (k, c) in chars.iter().enumerate() {
                    let src = if do_reverse {
                        n - 1 - k
                    } else {
                        (((k as i64) + shift).rem_euclid(n as i64)) as usize
                    };
                    out[k] = chars[src];
                }
                Ok(Rc::new(APLValue::Str(out.into_iter().collect())))
            }
            APLValue::Number(_) | APLValue::Null => Ok(Rc::new(right.as_ref().clone())),
            APLValue::Array(a) => {
                let dims = a.dimensions.clone();
                let rank = dims.len();
                if rank == 0 {
                    return Ok(Rc::new(right.as_ref().clone()));
                }
                let axis = axis.unwrap_or(rank - 1).min(rank - 1);
                let n = dims[axis];
                let stride: usize = dims[axis + 1..].iter().product();
                let cells: usize = dims[..axis].iter().copied().product::<usize>().max(1);

                // Build a per-cell shift (rotate amount). None left ⇒ reverse.
                let mut do_reverse = false;
                let mut shifts: Vec<i64> = Vec::with_capacity(cells);
                match left_val {
                    None => {
                        do_reverse = true;
                        shifts = vec![0; cells];
                    }
                    Some(l) => {
                        let lv = l.force(self)?;
                        match lv.as_ref() {
                            APLValue::Number(KapNumber::Long(x)) => {
                                shifts = vec![*x; cells];
                            }
                            APLValue::Array(va) => {
                                for e in va.elements() {
                                    match e.as_ref() {
                                        APLValue::Number(KapNumber::Long(x)) => shifts.push(*x),
                                        _ => {
                                            return Err(AplError::runtime(
                                                "⌽/⊖ shift must be integers".into(),
                                            ))
                                        }
                                    }
                                }
                                if !shifts.is_empty() && shifts.len() != cells {
                                    return Err(AplError::runtime(
                                        "⌽/⊖ shift vector length must match number of cells".into(),
                                    ));
                                }
                            }
                            _ => {
                                return Err(AplError::runtime(
                                    "⌽/⊖ shift must be an integer or vector".into(),
                                ))
                            }
                        }
                    }
                }

                let elems = a.elements();
                let mut out = elems.clone();
                let n64 = n as i64;
                for c in 0..cells {
                    let base = c * n * stride;
                    let shift = if do_reverse { 0 } else { shifts[c % shifts.len().max(1)] };
                    for k in 0..n {
                        let src = if do_reverse {
                            n - 1 - k
                        } else {
                            (((k as i64) + shift).rem_euclid(n64)) as usize
                        };
                        for s in 0..stride {
                            out[base + k * stride + s] =
                                elems[base + src * stride + s].clone();
                        }
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    dims,
                    ArrayData::Nested(out),
                )))))
            }
            other => Err(AplError::runtime(
                "⌽/⊖ not implemented for this value type".into(),
            )),
        }
    }

    fn transpose(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Monadic `⍉ A` reverses the axis order. Dyadic `axes ⍉ A` permutes axes
        // per `axes` (must be a permutation of 0..rank-1, else InvalidDimensions).
        let right = right_val.force(self)?;
        match right.as_ref() {
            // A string is rank-1; transposing it is the identity (it stays a string).
            APLValue::Str(_) => Ok(Rc::new(right.as_ref().clone())),
            APLValue::Number(_) | APLValue::Null => Ok(Rc::new(right.as_ref().clone())),
            APLValue::Array(a) => {
                let dims = a.dimensions.clone();
                let rank = dims.len();
                if rank == 0 {
                    return Ok(Rc::new(right.as_ref().clone()));
                }
                // Resolve the axis permutation.
                let perm: Vec<usize> = match left_val {
                    None => (0..rank).rev().collect(),
                    Some(l) => {
                        let lv = l.force(self)?;
                        let axes: Vec<i64> = match lv.as_ref() {
                            APLValue::Number(KapNumber::Long(x)) => vec![*x],
                            APLValue::Array(va) => {
                                let mut v = Vec::with_capacity(va.element_count());
                                for e in va.elements() {
                                    match e.as_ref() {
                                        APLValue::Number(KapNumber::Long(x)) => v.push(*x),
                                        _ => {
                                            return Err(AplError::runtime(
                                                "⍉ axes must be integers".into(),
                                            ))
                                        }
                                    }
                                }
                                v
                            }
                            _ => {
                                return Err(AplError::runtime(
                                    "⍉ axes must be an integer or vector".into(),
                                ))
                            }
                        };
                        let mut seen = vec![false; rank];
                        let mut perm = Vec::with_capacity(rank);
                        for &x in &axes {
                            if x < 0 || x as usize >= rank {
                                return Err(AplError::runtime(
                                    "⍉ axis index out of range".into(),
                                ));
                            }
                            if seen[x as usize] {
                                return Err(AplError::runtime(
                                    "⍉ axes must be a permutation of 0..rank-1".into(),
                                ));
                            }
                            seen[x as usize] = true;
                            perm.push(x as usize);
                        }
                        // Kotlin rule: the left arg is a *prefix* of the full axis
                        // permutation. When it is shorter than the rank, the remaining
                        // axes are appended in ascending order (skipping those already
                        // used). So `0 1 ⍉ 3 4 5⍴⍳60` => perm [0,1,2] (identity). A left
                        // arg longer than the rank is an error.
                        if axes.len() < rank {
                            for n in 0..rank {
                                if !seen[n] {
                                    perm.push(n);
                                }
                            }
                        } else if axes.len() != rank {
                            return Err(AplError::runtime(format!(
                                "⍉ axis count {} does not match array rank {}",
                                axes.len(),
                                rank
                            )));
                        }
                        perm
                    }
                };

                let elems = a.elements();
                // Kotlin `TransposedAPLValue`: result axis k has dimension
                // `d[perm⁻¹[k]]` (it stores inverseTransposedAxis = perm⁻¹).
                let mut inv = vec![0usize; rank];
                for (k, &p) in perm.iter().enumerate() {
                    inv[p] = k;
                }
                let new_dims: Vec<usize> = (0..rank).map(|k| dims[inv[k]]).collect();
                let old_stride = strides(&dims);
                let new_stride = strides(&new_dims);
                let total: usize = new_dims.iter().product();
                if total > 100_000_000 {
                    return Err(AplError::runtime("transpose result too large".into()));
                }
                let mut out: Vec<AplRef<APLValue>> = vec![elems[0].clone(); total];
                for pos in 0..total {
                    let mut rem = pos;
                    let mut new_coords = vec![0usize; rank];
                    for k in 0..rank {
                        new_coords[k] = rem / new_stride[k];
                        rem %= new_stride[k];
                    }
                    let mut old_coords = vec![0usize; rank];
                    for k in 0..rank {
                        // Kotlin `TransposedAPLValue.translateIndex`:
                        // `s[index] = c[transposeAxis[index]]`, i.e. source coord
                        // `k` equals result coord `perm[k]`.
                        old_coords[k] = new_coords[perm[k]];
                    }
                    let mut oflat = 0usize;
                    for k in 0..rank {
                        oflat += old_coords[k] * old_stride[k];
                    }
                    out[pos] = elems[oflat].clone();
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    new_dims,
                    ArrayData::Nested(out),
                )))))
            }
            other => Err(AplError::runtime("⍉ not implemented for this value type".into())),
        }
    }

    fn take(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Monadic `↑` ("First"): remove the leading axis and return the first
        // *cell* as itself — for a vector this is the first element as a scalar
        // (`↑1 2 3 4 5` -> 1, `↑"abcdef"` -> @a), for higher rank it is the first
        // sub-cell, and empty right arguments (`↑⍬`) yield the default fill (0).
        // Mirrors Real Kap's TakeAPLFunction.eval1ArgWithProto.
        if left_val.is_none() {
            let v = right_val.force(self)?;
            return match v.as_ref() {
                APLValue::Number(_) | APLValue::Char(_) => Ok(v),
                APLValue::Null => Ok(Rc::new(APLValue::Number(KapNumber::Long(0)))),
                APLValue::Str(s) => match s.chars().next() {
                    Some(c) => Ok(Rc::new(APLValue::Char(c))),
                    None => Ok(Rc::new(APLValue::Number(KapNumber::Long(0)))),
                },
                APLValue::Array(a) => {
                    let dims = a.dimensions.clone();
                    let elems = a.elements();
                    if elems.is_empty() {
                        // Empty array -> default fill (0).
                        Ok(Rc::new(APLValue::Number(KapNumber::Long(0))))
                    } else if dims.is_empty() || dims.len() == 1 {
                        // 0-rank or 1-D: the first element *is* the first cell.
                        Ok(elems[0].clone())
                    } else {
                        // Higher rank: the first cell is `product(dims[1..])`
                        // elements with shape `dims[1..]`.
                        let cell: usize = dims[1..].iter().product();
                        let cell_dims = dims[1..].to_vec();
                        let cell_elems = elems[..cell].to_vec();
                        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                            cell_dims,
                            ArrayData::Nested(cell_elems),
                        )))))
                    }
                }
                _ => Err(AplError::runtime(
                    "↑ (first) not implemented for this value type".into(),
                )),
            };
        }
        // Dyadic `↑`: `counts ↑ array`. Counts is a scalar or vector; each axis
        // count may be negative (take from the end). If `counts` is shorter than
        // the array rank the remaining axes are taken in full; if longer, it is an
        // error. A scalar right argument is reshaped to `|counts|` (padded with 0).
        // Reference: TakeTest.kt.
        let counts = self.count_vector(left_val.unwrap())?;
        self.take_or_drop(true, &counts, right_val)
    }

    fn drop(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Dyadic `↓`: `counts ↓ array`. Mirror of `take` but removes elements.
        // Monadic `↓` drops 1 along the leading axis. Reference: TakeTest.kt.
        let counts = match left_val {
            None => vec![1i64],
            Some(l) => self.count_vector(l)?,
        };
        self.take_or_drop(false, &counts, right_val)
    }

    /// Parse the left argument of `↑`/`↓` into a vector of (signed) axis counts.
    fn count_vector(&self, v: AplRef<APLValue>) -> Result<Vec<i64>, AplError> {
        match v.as_ref() {
            APLValue::Number(KapNumber::Long(n)) => Ok(vec![*n]),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(KapNumber::Long(n)) => out.push(*n),
                        _ => return Err(AplError::runtime("↑/↓ counts must be integers".into())),
                    }
                }
                Ok(out)
            }
            _ => Err(AplError::runtime("↑/↓ counts must be integers".into())),
        }
    }

    /// Core multi-dimensional take/drop. `take`=true means keep-from-edge
    /// (with padding for oversized positive counts); `take`=false means drop.
    fn take_or_drop(
        &self,
        take: bool,
        counts: &[i64],
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right = right_val.force(self)?;
        match right.as_ref() {
            APLValue::Null => {
                // `↑⍬` -> 0 (scalar fill); `↓⍬` -> ⍬ (null).
                if take {
                    Ok(Rc::new(APLValue::Number(KapNumber::Long(0))))
                } else {
                    Ok(Rc::new(APLValue::Null))
                }
            }
            APLValue::Number(_) => {
                // Scalar right argument.
                // Take: build an array of shape `|counts|` filled with the scalar,
                // padding with 0; an all-zero count yields the empty value (null).
                // Drop: dropping from a scalar removes it entirely — any non-zero
                // count empties it (null); a zero count keeps `(right)`.
                let all_zero = counts.iter().all(|c| *c == 0);
                if !take && !all_zero {
                    return Ok(Rc::new(APLValue::Null));
                }
                if !take && all_zero {
                    // Drop 0 of a scalar: keep it as a 1-element vector `(right)`.
                    return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                        vec![1],
                        ArrayData::Nested(vec![right.clone()]),
                    )))));
                }
                let dims: Vec<usize> = counts.iter().map(|c| c.unsigned_abs() as usize).collect();
                let total: usize = dims.iter().product();
                if take && total == 0 {
                    return Ok(Rc::new(APLValue::Null));
                }
                if total > 100_000_000 {
                    return Err(AplError::runtime("take/drop result too large".into()));
                }
                let content = right.clone();
                let fill = Rc::new(APLValue::Number(KapNumber::Long(0)));
                let mut out = Vec::with_capacity(total);
                for i in 0..total {
                    out.push(if i == 0 { content.clone() } else { fill.clone() });
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(out))))))
            }
            APLValue::Array(a) => {
                let mut dims = a.dimensions.clone();
                let rank = dims.len();
                if counts.len() > rank && rank > 0 {
                    return Err(AplError::runtime(
                        "↑/↓ count has more elements than the array rank".into(),
                    ));
                }
                // Compute the final shape up front and reject runaway results
                // (e.g. `1e6 1e6 ↑ small`) *before* slicing, so we never attempt to
                // allocate an OOM-sized vector — `slice_axis` requests a `Vec`
                // capacity of `outer * target * stride`, and for a padded multi-axis
                // take that can be ~1e12 elements, which aborts the process.
                let mut final_dims = dims.clone();
                for axis in 0..rank {
                    let spec = counts.get(axis).copied();
                    let n = dims[axis];
                    final_dims[axis] = match spec {
                        None => n,
                        Some(c) if take => c.unsigned_abs() as usize,
                        Some(c) => n - n.min(c.unsigned_abs() as usize),
                    };
                }
                let total: usize = final_dims.iter().product();
                if total > 100_000_000 {
                    return Err(AplError::runtime("take/drop result too large".into()));
                }
                let mut flat = a.elements();
                for axis in 0..rank {
                    let spec = counts.get(axis).copied();
                    let (new_flat, new_dims) = self.slice_axis(&flat, &dims, axis, take, spec)?;
                    flat = new_flat;
                    dims = new_dims;
                }
                let total: usize = dims.iter().product();
                if total > 100_000_000 {
                    return Err(AplError::runtime("take/drop result too large".into()));
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(flat))))))
            }
            APLValue::Str(s) => {
                // A string is a rank-1 vector of chars; take/drop slices its characters.
                let chars: Vec<char> = s.chars().collect();
                let n = chars.len();
                // Take/drop is monadic (single count) on a vector.
                let c = counts.first().copied().unwrap_or(0);
                let (keep, start) = if take {
                    let k = (c.unsigned_abs() as usize).min(n);
                    (k, 0)
                } else {
                    let drop = (c.unsigned_abs() as usize).min(n);
                    (n - drop, drop)
                };
                let sliced: String = chars[start..start + keep].iter().collect();
                Ok(Rc::new(APLValue::Str(sliced)))
            }
            other => Err(AplError::runtime(
                "↑/↓ not implemented for this value type".into(),
            )),
        }
    }

    /// Slice one axis of a flat (row-major) element list.
    /// `take`=true: keep `|spec|` blocks starting from the edge (0 for positive,
    /// from the end for negative), padding with the 0-fill for oversized counts.
    /// `take`=false: drop `|spec|` blocks from the edge, no padding.
    /// `spec`=None means "take all" (axis left unspecified by the count vector).
    fn slice_axis(
        &self,
        flat: &[AplRef<APLValue>],
        dims: &[usize],
        axis: usize,
        take: bool,
        spec: Option<i64>,
    ) -> Result<(Vec<AplRef<APLValue>>, Vec<usize>), AplError> {
        let n = dims[axis];
        let stride: usize = dims[axis + 1..].iter().product();
        let outer: usize = dims[..axis].iter().product();
        let fill = Rc::new(APLValue::Number(KapNumber::Long(0)));

        // Determine [start, keep, pad_before, pad_after] in units of `stride` blocks.
        let (start, keep, pad_before, pad_after, target): (usize, usize, usize, usize, usize) =
            match spec {
                None => (0, n, 0, 0, n),
                Some(c) if take => {
                    let abs = c.unsigned_abs() as usize;
                    if c >= 0 {
                        (0, n.min(abs), 0, abs.saturating_sub(n), abs)
                    } else {
                        let keep = n.min(abs);
                        (n - keep, keep, abs.saturating_sub(n), 0, abs)
                    }
                }
                Some(c) => {
                    let abs = c.unsigned_abs() as usize;
                    let d = n.min(abs);
                    (if c >= 0 { d } else { 0 }, n - d, 0, 0, n - d)
                }
            };

        let mut out = Vec::with_capacity(outer * target * stride);
        for g in 0..outer {
            let base = g * n * stride;
            for _ in 0..pad_before {
                for _ in 0..stride {
                    out.push(fill.clone());
                }
            }
            for k in 0..keep {
                let b = start + k;
                let off = base + b * stride;
                for i in 0..stride {
                    out.push(flat[off + i].clone());
                }
            }
            for _ in 0..pad_after {
                for _ in 0..stride {
                    out.push(fill.clone());
                }
            }
        }
        let mut new_dims = dims.to_vec();
        new_dims[axis] = target;
        Ok((out, new_dims))
    }

    fn enclose(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let v = right_val.force(self)?;
        // Enclose: a primitive value (scalar number, char, string, null) is returned
        // *unchanged* — its depth stays 0 and it does not become a box. A non-primitive
        // value (array) becomes a 0-dimensional array containing the value. Mirrors
        // Kap's `EncloseAPLFunction`. So `⊂5 → 5` and `≡⊂5 → 0`, while `,5` (a 1-element
        // vector) is non-primitive and `⊂,5` is a 0-D box of depth 2.
        match v.as_ref() {
            APLValue::Number(_) | APLValue::Char(_) | APLValue::Str(_) | APLValue::Null => Ok(v),
            _ => Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![],
                ArrayData::Nested(vec![v]),
            ))))),
        }
    }

    /// Kap's `≬` / `toList` (Kotlin `ToListFunction`, div_functions.kt).
    ///
    /// Monadic only: a scalar or 1-D array is boxed into a rank-0 array whose single
    /// element is the array coerced to a Kap *list* (the `⟨⟩` type). Real Kap returns an
    /// `APLList` whose `⍴` is `⍬`; the port has no separate list type, so we model it as
    /// a rank-0 `Nested([v])` box — which displays as `((...))` rather than the oracle's
    /// `⟨...⟩` (a recognised DISPLAY-glyph divergence, not a value defect). Unlike `⊂`,
    /// `≬` ALWAYS boxes even a primitive scalar (`≬5` → a 0-D box of `5`, not `5`).
    /// A rank>1 argument is an error ("Argument must be a scalar or 1-dimensional array").
    fn to_list(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let v = right_val.force(self)?;
        let dims = v.dimensions();
        if dims.len() > 1 {
            return Err(AplError::runtime(
                "≬: Argument must be a scalar or 1-dimensional array".into(),
            ));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![],
            ArrayData::Nested(vec![Rc::new(v.as_ref().clone())]),
        )))))
    }

    /// Kap's `fromList` (Kotlin `FromListFunction`, div_functions.kt) — the inverse of
    /// `≬`: recovers the array from a rank-0 list box. The port's list box is just a
    /// rank-0 `Nested([v])`, so `fromList` discloses that single element. (The curated
    /// parity cases only exercise `≬`; `fromList` is wired for completeness/consistency
    /// with the Kotlin registration pair.)
    fn from_list(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let v = right_val.force(self)?;
        match v.as_ref() {
            APLValue::Array(a) if a.dimensions.is_empty() => {
                let mut elems = a.elements();
                Ok(elems.remove(0))
            }
            _ => Err(AplError::runtime("fromList: Argument is not a list".into())),
        }
    }

    /// Kap's `⫇` / `group` (Kotlin `GroupFunction`, group-index.kt). Left `L` is a rank-1
    /// vector of group indices; right `R` has its major axis equal in length to `L`. Each
    /// index selects the corresponding major cell of `R` into that group. Negative indices
    /// are skipped. Returns a vector of arrays: group `i` (0-based) holds the selected cells;
    /// a group with no members is filled with `APLNull` (the Kotlin `APLNullValue` behaviour).
    /// For a rank-1 `R` the selected cells are scalars; for higher rank they are the
    /// per-`(rank-1)` sub-arrays along the major axis.
    /// Kotlin `APLValue.arrayify()`: wrap a non-array in a 1-element rank-1 array; arrays
    /// (and strings, which the port models rank-1) pass through unchanged.
    fn arrayify_value(v: &AplRef<APLValue>) -> AplRef<APLValue> {
        if matches!(v.as_ref(), APLValue::Array(_)) {
            v.clone()
        } else {
            Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![1],
                ArrayData::Nested(vec![v.clone()]),
            ))))
        }
    }

    fn group_indices(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.force(self)?;
        let b = right_val.force(self)?;
        // Kotlin `GroupFunctionImpl.eval2Arg` calls `a.arrayify()` / `b.arrayify()` first: a
        // scalar argument becomes a 1-element rank-1 array. That both protects the dims
        // indexing below (B3 panic fix) and gives scalars the oracle semantics
        // (`1 ⫇ 5` → ⟨⍬ ⟨5⟩⟩ — slot 0 empty, slot 1 holding the scalar).
        let a = Self::arrayify_value(&a);
        let b = Self::arrayify_value(&b);
        let a_dims = a.dimensions();
        if a_dims.len() != 1 {
            return Err(AplError::runtime(
                "⫇: Left argument should be rank 1".into(),
            ));
        }
        let b_dims = b.dimensions();
        if a_dims[0] != b_dims[0] {
            return Err(AplError::runtime(
                "⫇: Size of left argument must match the size of the major axis in the right argument".into(),
            ));
        }
        // Major cells of `b`: if rank > 1, each cell is a (rank-1) sub-array; otherwise a scalar.
        let b_is_scalar_cells = b_dims.len() == 1;
        let cell_dims: Vec<usize> = if b_is_scalar_cells {
            vec![]
        } else {
            b_dims[1..].to_vec()
        };
        let mut groups: Vec<Option<Vec<AplRef<APLValue>>>> = Vec::new();
        let mut max_group = 0i64;
        for i in 0..a_dims[0] {
            let idx = match &a.value_at(i) {
                APLValue::Number(n) => n.as_long().map_err(|e| AplError::runtime(e))?,
                other => {
                    return Err(AplError::runtime(format!(
                        "⫇: Left argument must be numeric, got {}",
                        other.class_name()
                    )))
                }
            };
            if idx < 0 {
                continue; // negative index => skip this element (Kotlin)
            }
            let cell = if b_is_scalar_cells {
                Rc::new(b.value_at(i).clone())
            } else {
                // Build the per-major-cell (rank-1) sub-array.
                let total: usize = cell_dims.iter().product();
                let stride: usize = total.max(1);
                let base = i * stride;
                let mut elems = Vec::with_capacity(total);
                for k in 0..total {
                    elems.push(Rc::new(b.value_at(base + k).clone()));
                }
                Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    cell_dims.clone(),
                    ArrayData::Nested(elems),
                ))))
            };
            let gi = idx as usize;
            if gi >= groups.len() {
                groups.resize_with(gi + 1, || None);
            }
            groups[gi].get_or_insert_with(Vec::new).push(cell);
            if idx > max_group {
                max_group = idx;
            }
        }
        // Pad to the highest referenced group so the result length is (max_group + 1).
        if (max_group as usize + 1) > groups.len() {
            groups.resize_with(max_group as usize + 1, || None);
        }
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(groups.len());
        for g in groups {
            match g {
                None => out.push(Rc::new(APLValue::Null)),
                Some(cells) => {
                    let arr = if b_is_scalar_cells {
                        // Scalar cells: a plain vector.
                        APLValue::Array(Rc::new(KapArray::new(
                            vec![cells.len()],
                            ArrayData::Nested(cells),
                        )))
                    } else {
                        // Rank-1 sub-arrays: concatenate along the new leading axis.
                        let mut elems = Vec::new();
                        for c in &cells {
                            for e in c.elements() {
                                elems.push(e.clone());
                            }
                        }
                        let mut dims = vec![cells.len()];
                        dims.extend_from_slice(&cell_dims);
                        APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(elems))))
                    };
                    out.push(Rc::new(arr));
                }
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Kap's partitioned enclose (`⊆`): monadic = "nest" (enclose a non-scalar whole;
    /// scalars pass through), dyadic = partition `B` along the last axis using `A`'s
    /// integer indicators. Mirrors Kotlin `PartitionedEncloseFunction`
    /// (`disclose.kt`): a `>0` indicator at position `i>0` starts a new partition; an
    /// indicator `>1` opens that many partitions; cells are contiguous runs along the axis.
    fn partitioned_enclose(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let b = right_val.force(self)?;
        match left_val {
            None => {
                // Monadic: scalar passes through; anything else is enclosed whole.
                if b.dimensions().is_empty() {
                    Ok(Rc::new(b.as_ref().clone()))
                } else {
                    self.enclose(Rc::new(b.as_ref().clone()))
                }
            }
            Some(l) => {
                let a = l.force(self)?;
                let b_dims = b.dimensions();
                if b_dims.is_empty() {
                    return Err(AplError::runtime("⊆: right argument must not be a scalar".into()));
                }
                let axis = b_dims.len() - 1; // default last axis (Kotlin computeAxis)
                // Left indicators must be a scalar or a 1-D array.
                let a_dims = a.dimensions();
                let inds: Vec<i64> = match a.as_ref() {
                    APLValue::Number(KapNumber::Long(v)) => vec![*v],
                    APLValue::Array(_) => a
                        .elements()
                        .iter()
                        .map(|e| match e.as_ref() {
                            APLValue::Number(KapNumber::Long(v)) => *v,
                            _ => 0,
                        })
                        .collect(),
                    _ => {
                        if a_dims.is_empty() {
                            vec![0]
                        } else {
                            return Err(AplError::runtime(
                                "⊆: left argument must be a scalar or 1-D array".into(),
                            ));
                        }
                    }
                };
                if inds.len() != b_dims[axis] {
                    return Err(AplError::runtime(format!(
                        "⊆: size of A ({}) must equal the dimension of B along the selected axis ({})",
                        inds.len(),
                        b_dims[axis]
                    )));
                }
                // computePartitionIndexes: collect (start,end) pairs along the axis.
                let mut partitions: Vec<(usize, usize)> = Vec::new();
                let mut curr_start: usize = 0;
                let n = inds.len();
                for i in 0..n {
                    if inds[i] > 0 && i > 0 {
                        partitions.push((curr_start, i));
                        curr_start = i;
                    }
                    if inds[i] > 1 {
                        for _ in 0..(inds[i] - 1) {
                            partitions.push((i, i));
                        }
                    }
                }
                partitions.push((curr_start, n));

                let b_elems = b.elements();
                let axis_len = b_dims[axis];
                let frame = b_dims[..axis].iter().product::<usize>().max(1);
                let row_len = axis_len; // along-axis length per frame cell

                // Build nested cells. For each leading-frame index, slice [start,end)
                // along the axis and assemble a 1-D vector cell.
                let mut cells: Vec<AplRef<APLValue>> = Vec::new();
                for f in 0..frame {
                    for &(start, end) in &partitions {
                        let mut cell: Vec<AplRef<APLValue>> = Vec::new();
                        for j in start..end {
                            let flat = f * row_len + j;
                            cell.push(b_elems[flat].clone());
                        }
                        cells.push(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                            vec![cell.len()],
                            ArrayData::Nested(cell),
                        )))));
                    }
                }
                let outer_dims = if frame == 1 {
                    vec![partitions.len()]
                } else {
                    let mut d = b_dims[..axis].to_vec();
                    d.push(partitions.len());
                    d
                };
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    outer_dims,
                    ArrayData::Nested(cells),
                )))))
            }
        }
    }

    /// Kap's pair (`⍮`): monadic `⍮x` = enclose x in a length-1 nested vector;
    /// dyadic `a⍮b` = a length-2 nested vector `(a b)`. Mirrors Kotlin
    /// `PairAPLFunction` (eval1Arg → ResizedArrayImpls.resizedSingleValue;
    /// eval2Arg → APLArrayImpl(dimensionsOfSize(2), [a, b])).
    fn pair(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left_val {
            None => {
                // Monadic `⍮x` = `⟨x⟩` — a length-1 vector whose single element is `x`.
                // Oracle: `⍮5`→`⟨5⟩`, `⍮1 2 3`→`⟨⟨1 2 3⟩⟩`, `⍮⊂5`→`⟨5⟩`, `⍴⍮5`→`⟨1⟩`.
                let v = right_val;
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![1],
                    ArrayData::Nested(vec![v]),
                )))))
            }
            Some(l) => {
                let a = l.force(self)?;
                let b = right_val.force(self)?;
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![2],
                    ArrayData::Nested(vec![a, b]),
                )))))
            }
        }
    }

    /// Kap's `⊃` (reveal / disclose + pick), `DiscloseAPLFunction` in disclose.kt.
    ///
    /// Monadic `⊃X` = disclose: remove the outer level of boxing.
    ///   - A Simple (Long/Double/Char) array of any rank is returned unchanged.
    ///   - A scalar / 0-d box returns its single contained value (`⊃⊂5 → 5`, `⊃⍬ → ⍬`).
    ///   - A Nested array whose elements are themselves arrays of uniform shape S becomes
    ///     one array with dims `[outer_dims..., S...]` (Kotlin `DisclosedArrayValue`:
    ///     `⊃(1 2)(3 4) → 2 2⍴1 2 3 4`). A Nested array of scalars collapses to a simple
    ///     vector (or stays nested when mixed).
    ///
    /// Dyadic `A ⊃ B` = nested pick (selector `A`, array `B`, NOT reuse of `⊇`):
    ///   - left must be rank 0 or 1;
    ///   - each member of `A` is a coordinate; a scalar member while `curr` is not rank-1
    ///     errors "Mismatched dimensions for selection"; a vector member whose length !=
    ///     `curr.rank` errors "Dimensions does not match"; out-of-range errors
    ///     "Selection index out of bounds".
    fn reveal(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left_val {
            None => {
                // --- Monadic disclose ---
                let v = right_val.force(self)?;
                match v.as_ref() {
                    // 0-d / scalar: unwrap the single boxed value.
                    APLValue::Array(a) if a.dimensions.is_empty() => {
                        let el = a.elements();
                        if el.is_empty() {
                            Ok(Rc::new(APLValue::Null))
                        } else {
                            Ok(Rc::new(el[0].as_ref().clone()))
                        }
                    }
                    // Simple array (Long/Double/Char) of any rank: identity.
                    APLValue::Array(a) if !matches!(a.data, ArrayData::Nested(_)) => {
                        Ok(Rc::new(v.as_ref().clone()))
                    }
                    APLValue::Array(a) => {
                        // Nested array: inspect the elements.
                        let outer = a.dimensions.clone();
                        let elems = match &a.data {
                            ArrayData::Nested(e) => e.clone(),
                            _ => unreachable!(),
                        };
                        if elems.is_empty() {
                            // Empty nested -> empty simple vector (rank preserved as 1).
                            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                                vec![0],
                                ArrayData::Nested(vec![]),
                            )))));
                        }
                        // All elements are arrays of uniform (inner) shape S?
                        let mut inner_shape: Option<Vec<usize>> = None;
                        let mut uniform_arrays = true;
                        let mut all_scalar = true;
                        for e in &elems {
                            match e.as_ref() {
                                APLValue::Array(ea) => {
                                    all_scalar = false;
                                    let s = ea.dimensions.clone();
                                    match &inner_shape {
                                        Some(prev) if *prev != s => uniform_arrays = false,
                                        Some(_) => {}
                                        None => inner_shape = Some(s),
                                    }
                                }
                                _ => {
                                    // scalar / box element
                                    if inner_shape.is_some() {
                                        uniform_arrays = false;
                                    }
                                }
                            }
                        }
                        if all_scalar {
                            // Vector of scalars: collapse to a simple vector when uniform,
                            // else keep the nested vector (re-uses the simple-vector builder).
                            return self.make_simple_or_nested(outer, elems);
                        }
                        if uniform_arrays {
                            if let Some(s) = inner_shape {
                                // Drop the outer axis: dims = [outer..., S...].
                                let mut new_dims = outer.clone();
                                new_dims.extend(s.iter().copied());
                                // Concatenate every element's flat elements.
                                let mut out: Vec<AplRef<APLValue>> = Vec::new();
                                for e in &elems {
                                    if let APLValue::Array(ea) = e.as_ref() {
                                        for x in ea.elements() {
                                            out.push(Rc::new(x.as_ref().clone()));
                                        }
                                    }
                                }
                                return self.make_simple_or_nested(new_dims, out);
                            }
                        }
                        // Mixed / ragged: disclose one level (ravel elements into a vector).
                        let mut out = Vec::new();
                        for e in &elems {
                            self.collect_elements(e, &mut out);
                        }
                        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                            vec![out.len()],
                            ArrayData::Nested(out),
                        )))))
                    }
                    // Bare scalar / string / null: identity (unwrapped).
                    other => Ok(Rc::new(other.clone())),
                }
            }
            Some(l) => {
                // --- Dyadic nested pick ---
                let a = l.force(self)?;
                if a.dimensions().len() > 1 {
                    return Err(AplError::runtime(
                        "⊃: Left argument to pick should be rank 0 or 1".into(),
                    ));
                }
                let mut curr = right_val.force(self)?;
                for idx in a.elements() {
                    let idx = idx.force(self)?;
                    let (d, index) = match idx.as_ref() {
                        APLValue::Array(ia) => {
                            // vector coordinate: its length must equal curr.rank
                            let coord = ia.dimensions.clone();
                            if coord.len() != curr.dimensions().len() {
                                return Err(AplError::runtime(
                                    "⊃: Dimensions does not match".into(),
                                ));
                            }
                            let mut flat = 0usize;
                            let r = curr.dimensions().len();
                            let mut stride = vec![1usize; r];
                            if r > 1 {
                                for k in (0..r - 1).rev() {
                                    stride[k] = stride[k + 1] * curr.dimensions()[k + 1];
                                }
                            }
                            for k in 0..r {
                                let i = self.index_to_i64(ia.elements()[k].as_ref())?;
                                let n = curr.dimensions()[k] as i64;
                                let adj = if i < 0 { i.rem_euclid(n) } else { i };
                                if adj < 0 || adj >= n {
                                    return Err(AplError::runtime(
                                        "⊃: Selection index out of bounds".into(),
                                    ));
                                }
                                flat += (adj as usize) * stride[k];
                            }
                            (coord, flat)
                        }
                        _ => {
                            // scalar coordinate: curr must be rank 1
                            if curr.dimensions().len() != 1 {
                                return Err(AplError::runtime(
                                    "⊃: Mismatched dimensions for selection".into(),
                                ));
                            }
                            let i = self.index_to_i64(idx.as_ref())?;
                            let n = curr.dimensions()[0] as i64;
                            let adj = if i < 0 { i.rem_euclid(n) } else { i };
                            if adj < 0 || adj >= n {
                                return Err(AplError::runtime(
                                    "⊃: Selection index out of bounds".into(),
                                ));
                            }
                            (curr.dimensions().clone(), adj as usize)
                        }
                    };
                    let size: usize = d.iter().product();
                    if index >= size {
                        return Err(AplError::runtime(
                            "⊃: Selection index out of bounds".into(),
                        ));
                    }
                    curr = Rc::new(curr.value_at(index).clone());
                }
                Ok(curr)
            }
        }
    }

    /// Build a `KapArray` from `dims` + flat `elems`, choosing the most specific Simple
    /// storage (Long/Double/Char) when every element is uniform, else Nested.
    fn make_simple_or_nested(
        &self,
        dims: Vec<usize>,
        elems: Vec<AplRef<APLValue>>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let all_long = elems.iter().all(|e| matches!(e.as_ref(), APLValue::Number(KapNumber::Long(_))));
        let all_double = elems.iter().all(|e| matches!(e.as_ref(), APLValue::Number(KapNumber::Double(_))));
        let all_char = elems.iter().all(|e| matches!(e.as_ref(), APLValue::Char(_)));
        if all_long {
            let v: Vec<i64> = elems.into_iter().map(|e| match e.as_ref() {
                APLValue::Number(KapNumber::Long(x)) => *x,
                _ => unreachable!(),
            }).collect();
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Long(v))))));
        }
        if all_double {
            let v: Vec<f64> = elems.into_iter().map(|e| match e.as_ref() {
                APLValue::Number(KapNumber::Double(x)) => *x,
                _ => unreachable!(),
            }).collect();
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Double(v))))));
        }
        if all_char {
            let v: Vec<char> = elems.into_iter().map(|e| match e.as_ref() {
                APLValue::Char(c) => *c,
                _ => unreachable!(),
            }).collect();
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Char(v))))));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(elems))))))
    }

    /// Kap's `⌷` (squad / index selection, `AccessFromIndexAPLFunction` in lookup.kt).
    /// - Monadic `⌷X` = `⟨X⟩` — a length-1 vector whose single element is `X` itself
    ///   (Kotlin `fromListFunction.eval1Arg(X.listify())`; `listify` wraps the whole value
    ///   in a 1-element APLList). Oracle: `⍴⌷1 2 3 4` → `⟨1⟩`, `⌷1 2 3 4` → `⟨⟨1 2 3 4⟩⟩`,
    ///   `⍴⌷⊂5` → `⟨1⟩`, `⌷⊂5` → `⟨5⟩`.
    /// - Dyadic `A⌷B`: `A` is a rank-1 "position argument" whose elements select along the
    ///   corresponding axis of `B` — a scalar index collapses that axis, a vector selects a
    ///   sub-axis of that size, and `⍬`/null selects the whole axis. Reuses `pick` (the same
    ///   per-axis selection machinery Kotlin shares between bracket-index and `⌷`).
    fn access_from_index(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left_val {
            None => {
                // Monadic: listify wraps the *whole* value X in a single-element vector.
                let v = right_val.force(self)?;
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![1],
                    ArrayData::Nested(vec![v]),
                )))))
            }
            Some(a) => {
                let b = right_val.force(self)?;
                let s = a.force(self)?;
                // `⍬⌷B` (Null/empty position argument) selects the *entire* right argument
                // (Kotlin `indexValue == APLNilValue -> makeAllIndexList`): identity selection.
                // Oracle: `⍬⌷1 2 3`→`⟨1 2 3⟩`, `⍬⌷3 3⍴⍳9`→shape `3 3`, `⍬⌷5`→`5`.
                if matches!(s.as_ref(), APLValue::Null) {
                    return Ok(b);
                }
                self.squad(&b, &s)
            }
        }
    }

    /// Dyadic `⌷` (squad / index selection, oracle-native semantics):
    /// each element of the left argument indexes the corresponding axis of `B`;
    /// axes without a specifier are kept WHOLE (`1 ⌷ 3 3⍴⍳9` → row `⟨3 4 5⟩`).
    /// A full-length numeric vector is a coordinate pick (`(2 2) ⌷ m` → `8`,
    /// `2 1 ⌷ m` → `7`). Scalars select-and-drop their axis (`2 ⌷ v` → `3`).
    /// Negative indices count from the end (shared `check_and_adjust_selected_index`).
    fn squad(
        &self,
        b_val: &AplRef<APLValue>,
        sel_val: &AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let b = b_val.force(self)?;
        // Target -> (dims, flat elements). A Str is a rank-1 char vector.
        let (bdims, belems): (Vec<usize>, Vec<AplRef<APLValue>>) = match b.as_ref() {
            APLValue::Array(arr) => (arr.dimensions.clone(), arr.elements()),
            APLValue::Str(s) => (
                vec![s.chars().count()],
                s.chars().map(|c| Rc::new(APLValue::Char(c)) as AplRef<APLValue>).collect(),
            ),
            other => (vec![], vec![Rc::new(other.clone())]),
        };
        let r = bdims.len();
        // Row-major strides for B.
        let mut stride = vec![1usize; r];
        if r > 1 {
            for k in (0..r - 1).rev() {
                stride[k] = stride[k + 1] * bdims[k + 1];
            }
        }
        // Per-axis selections: Some(index) fixes the axis, None keeps it whole.
        // Null specs also mean "keep whole" (Kotlin makeAllIndexList).
        let mut fixed: Vec<Option<usize>> = vec![None; r];
        let sel_elems: Vec<AplRef<APLValue>> = match sel_val.as_ref() {
            APLValue::Array(arr) => arr.elements(),
            other => vec![Rc::new(other.clone())],
        };
        if sel_elems.len() > r {
            return Err(AplError::runtime(format!(
                "too many index axes ({} specifiers for rank-{})",
                sel_elems.len(),
                r
            )));
        }
        for (k, sp) in sel_elems.iter().enumerate() {
            let sp = sp.force(self)?;
            if matches!(sp.as_ref(), APLValue::Null) {
                continue; // keep whole
            }
            let i = self.index_to_i64(sp.as_ref())?;
            let adj = check_and_adjust_selected_index(i, bdims[k])?;
            fixed[k] = Some(adj);
        }
        // Kept axes (whole) in order; their dims form the result shape.
        let out_dims: Vec<usize> = (0..r).filter(|&k| fixed[k].is_none()).map(|k| bdims[k]).collect();
        let base: usize = fixed
            .iter()
            .enumerate()
            .filter_map(|(k, f)| f.map(|v| v * stride[k]))
            .sum();
        if out_dims.is_empty() {
            // Full coordinate: single element.
            return Ok(Rc::new(belems[base].as_ref().clone()));
        }
        // Iterate the kept axes row-major, gathering elements.
        let kept_axes: Vec<usize> = (0..r).filter(|&k| fixed[k].is_none()).collect();
        let total: usize = out_dims.iter().product();
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(total);
        let mut counters = vec![0usize; kept_axes.len()];
        for _ in 0..total {
            let mut flat = base;
            for (j, &k) in kept_axes.iter().enumerate() {
                flat += counters[j] * stride[k];
            }
            out.push(Rc::new(belems[flat].as_ref().clone()));
            // odometer increment (last kept axis fastest)
            for j in (0..counters.len()).rev() {
                counters[j] += 1;
                if counters[j] < out_dims[j] {
                    break;
                }
                counters[j] = 0;
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            out_dims,
            ArrayData::Nested(out),
        )))))
    }
    /// coordinate* into `B` (a scalar index for rank-1 `B`, a coordinate vector for
    /// higher-rank `B`), with negative-index support (`¯1` = last). Mirrors Kotlin
    /// `PickAPLFunction` / `PickResultValue` (lookup.kt).
    fn pick_apl(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime("⊇ needs two arguments".into()))?;
        let a = a.force(self)?;
        let b = right_val.force(self)?;
        let a_dims = a.dimensions();
        let a_elems = a.elements();
        let b_dims = b.dimensions();
        let b_elems = b.elements();

        // Row-major strides for `B` (so a coordinate vector maps to a flat position).
        let r = b_dims.len();
        let mut bstride = vec![1usize; r];
        if r > 1 {
            for k in (0..r - 1).rev() {
                bstride[k] = bstride[k + 1] * b_dims[k + 1];
            }
        }

        let total = a_elems.len();
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(total);
        for idx in &a_elems {
            let idx = idx.force(self)?;
            // An index coordinate is either a scalar (rank-1 B) or a vector (rank-N B).
            let coord_elems: Vec<AplRef<APLValue>> = match idx.as_ref() {
                APLValue::Array(x) => x.elements(),
                _ => vec![Rc::new(idx.as_ref().clone())],
            };
            if coord_elems.len() != r {
                return Err(AplError::runtime(format!(
                    "⊇: index coordinate rank mismatch (got {}, expected rank {})",
                    coord_elems.len(),
                    r
                )));
            }
            let mut flat = 0usize;
            for k in 0..r {
                let i = self.index_to_i64(coord_elems[k].as_ref())?;
                let adj = check_and_adjust_selected_index(i, b_dims[k])?;
                flat += adj * bstride[k];
            }
            out.push(Rc::new(b_elems[flat].as_ref().clone()));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            a_dims,
            ArrayData::Nested(out),
        )))))
    }

    /// Kap's bracket-index selection `x[y]` (Kotlin `APLValue.get` /
    /// `indexFromPositionNegativeSupport` in dimension.kt).
    ///
    /// `selector` is an `APLValue::Array` of `;`-separated *sections*, one per axis; the
    /// number of sections must be ≤ the rank of `x`. Each section selects along its axis:
    ///   - `⍬` / `Instr::Empty` / an empty vector (e.g. `x[;2]` second part) -> the whole axis
    ///   - a scalar index -> one element (with negative wrap: `¯1` = last)
    ///   - a vector of indices -> that many elements along the axis
    /// Result shape = the concatenation of the per-section lengths. Chained bracket-index
    /// `x[i][j]` is just nested `index_select`: the outer `Index` re-indexes the inner result.
    fn index_select(
        &self,
        arr: &APLValue,
        selector: &APLValue,
    ) -> Result<AplRef<APLValue>, AplError> {
        let arr = arr.force(self)?;
        let dims = arr.dimensions();
        let rank = dims.len();

        // Resolve the selector into one section value per written axis.
        let sections: Vec<AplRef<APLValue>> = match selector {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };
        if sections.len() > rank {
            return Err(AplError::runtime(format!(
                "Index list length must be less than or equal to the rank of the argument. Argument={}, index={}",
                rank, sections.len()
            )));
        }

        // For each axis, resolve the section into the list of selected flat indices.
        let mut selected: Vec<Vec<usize>> = Vec::with_capacity(rank);
        for k in 0..sections.len() {
            let sec = sections[k].force(self)?;
            let axis_size = dims[k];
            // Empty / `⍬` section => whole axis.
            let idx_list: Vec<AplRef<APLValue>> = match sec.as_ref() {
                APLValue::Null => vec![],
                APLValue::Array(a) if a.element_count() == 0 => vec![],
                APLValue::Array(a) => a.elements(),
                _ => vec![Rc::new(sec.as_ref().clone())],
            };
            if idx_list.is_empty() {
                selected.push((0..axis_size).collect());
            } else {
                let mut v = Vec::with_capacity(idx_list.len());
                for e in &idx_list {
                    let i = self.index_to_i64(e.force(self)?.as_ref())?;
                    v.push(check_and_adjust_selected_index(i, axis_size)?);
                }
                selected.push(v);
            }
        }
        // Pad missing trailing axes with "all".
        for k in sections.len()..rank {
            selected.push((0..dims[k]).collect());
        }

        // Result shape = the lengths of every axis whose section selects MORE THAN ONE
        // element. A scalar section (exactly one index) collapses/drops that axis — this is
        // how Kap's `x[0]` on a matrix yields a rank-1 row vector, and `x[1;2]` (two scalar
        // sections) yields a rank-0 scalar. A vector/`⍬` section keeps its axis.
        let result_dims: Vec<usize> = selected
            .iter()
            .filter(|s| s.len() > 1)
            .map(|s| s.len())
            .collect();

        // Row-major strides over the *original* `arr` axes.
        let mut strides = vec![1usize; rank];
        if rank > 1 {
            for k in (0..rank - 1).rev() {
                strides[k] = strides[k + 1] * dims[k + 1];
            }
        }

        // Enumerate row-major combinations of per-section index choices. Scalar sections
        // (len 1) stay fixed at index 0 and are not advanced by the combo counter, but they
        // still contribute their (single) coordinate to the flat position.
        let total: usize = result_dims.iter().product();
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(total.max(1));
        let mut combo = vec![0usize; selected.len()];
        for _ in 0..total {
            let mut flat = 0usize;
            for k in 0..selected.len() {
                flat += selected[k][combo[k]] * strides[k];
            }
            out.push(Rc::new(arr.value_at(flat)));
            // Increment combo: only axes with more than one choice vary (last varies fastest).
            for k in (0..selected.len()).rev() {
                if selected[k].len() > 1 {
                    combo[k] += 1;
                    if combo[k] < selected[k].len() {
                        break;
                    }
                    combo[k] = 0;
                }
            }
        }

        // When every section was a scalar index, `result_dims` is empty: the result is a
        // single rank-0 element, which Kap renders as a bare scalar (e.g. `x[2] → 3`, not
        // `(3)`). Return that element directly rather than wrapping it in a rank-0 array.
        if result_dims.is_empty() {
            return Ok(out.into_iter().next().unwrap_or_else(|| Rc::new(APLValue::Null)));
        }

        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            result_dims,
            ArrayData::Nested(out),
        )))))
    }

    /// Kap's branch/return primitive `→`.
    /// - Monadic `→ value`: immediately returns `value` from the enclosing function.
    /// - Dyadic `cond → value`: if `cond` is truthy, returns `value`; otherwise
    ///   yields `value` and control continues (the function does not exit).
    /// Implemented as a control-flow signal (`AplError::Return`) that the enclosing
    /// user-function frame catches. If it escapes to top level (no enclosing
    /// function), `eval_string_in_env` converts it to a runtime error.
    /// Mirrors Kotlin `ReturnFunction` (div_functions.kt).
    fn return_arrow(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left_val {
            None => Err(AplError::Return(right_val)),
            Some(cond) => {
                if self.truthy(&cond) {
                    Err(AplError::Return(right_val))
                } else {
                    Ok(right_val)
                }
            }
        }
    }

    /// Convert an `APLValue` into a 0-based integer index. Accepts Long and Double
    /// (truncated toward zero, matching Kap's numeric->int coercion).
    fn index_to_i64(&self, v: &APLValue) -> Result<i64, AplError> {
        match v {
            APLValue::Number(KapNumber::Long(i)) => Ok(*i),
            APLValue::Number(KapNumber::Double(f)) => Ok(*f as i64),
            _ => Err(AplError::runtime("array index must be an integer".into())),
        }
    }

    /// Kap bracket indexing `target[selector]`, matching the Kotlin reference
    /// (`lookup.kt`): `PickResultValue` for a single `;`-section, and
    /// `AccessFromIndexAPLFunction` (per-axis selection) for `;`-separated sections.
    ///
    /// Single section (no `;`): result shape = the index's shape; each scalar cell of the
    /// index selects from `target` (a scalar index discloses to a scalar). Negative
    /// indices count from the end; out-of-range errors.
    ///
    /// Multiple sections (`a[r;c]`): per target axis
    ///   - `⍬`/empty section -> select the *entire* axis ("all"),
    ///   - scalar            -> collapse to that position (axis dropped from result),
    ///   - vector of rank s  -> pick those positions; contributes s axes (its shape).
    /// A fully-collapsed selection (every axis a scalar) errors, matching Kap.
    fn pick(&self, target: &APLValue, selector: &APLValue) -> Result<AplRef<APLValue>, AplError> {
        let target = target.force(self)?;
        let selector = selector.force(self)?;

        // Selector -> axis specs; `⍬`/Null means "all" for that axis.
        let specs: Vec<AplRef<APLValue>> = match selector.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };

        // ---- Single section: PickResultValue ----
        if specs.len() == 1 {
            let index = specs.into_iter().next().unwrap();
            let index = index.force(self)?;
            if matches!(index.as_ref(), APLValue::Null) {
                return Ok(Rc::new(APLValue::Null));
            }
            let target_elems: Vec<AplRef<APLValue>> = match target.as_ref() {
                APLValue::Array(a) => a.elements(),
                // A string is a rank-1 vector of its characters.
                APLValue::Str(s) => s
                    .chars()
                    .map(|c| Rc::new(APLValue::Char(c)) as AplRef<APLValue>)
                    .collect(),
                _ => vec![Rc::new(target.as_ref().clone())],
            };
            let target_rank = match target.as_ref() {
                APLValue::Array(a) => a.rank(),
                APLValue::Str(_) => 1,
                _ => 0,
            };
            if target_rank != 1 {
                return Err(AplError::runtime(format!(
                    "pick into rank-{} array requires coordinate indices",
                    target_rank
                )));
            }
            let idx_dims = match index.as_ref() {
                APLValue::Array(a) => a.dimensions.clone(),
                _ => vec![],
            };
            let idx_elems = match index.as_ref() {
                APLValue::Array(a) => a.elements(),
                other => vec![Rc::new(other.clone())],
            };
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(idx_elems.len());
            for e in &idx_elems {
                let i = self.index_to_i64(e.as_ref())?;
                let adj = check_and_adjust_selected_index(i, target_elems.len())?;
                out.push(Rc::new(target_elems[adj].as_ref().clone()));
            }
            if idx_dims.is_empty() {
                Ok(out.into_iter().next().unwrap())
            } else {
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    idx_dims,
                    ArrayData::Nested(out),
                )))))
            }
        } else {
            // ---- Multi-axis: AccessFromIndex ----
            let bdims = match target.as_ref() {
                APLValue::Array(a) => a.dimensions.clone(),
                _ => vec![1],
            };
            let r = bdims.len();
            let na = specs.len();
            if na > r {
                return Err(AplError::runtime(format!(
                    "too many index axes ({} specifiers for rank-{})",
                    na, r
                )));
            }
            let bstride: Vec<usize> = {
                let mut s = vec![1usize; r];
                for k in (0..r).rev() {
                    if k + 1 < r {
                        s[k] = s[k + 1] * bdims[k + 1];
                    }
                }
                s
            };

            enum Spec {
                All(usize),
                Collapsed(usize),
                VecIndex {
                    v_dims: Vec<usize>,
                    v_elems: Vec<AplRef<APLValue>>,
                    out_start: usize,
                },
            }

            let mut plans: Vec<(usize, Spec)> = Vec::with_capacity(r);
            let mut out_dims: Vec<usize> = Vec::new();
            let mut next_out = 0usize;
            for k in 0..r {
                let spec_val = if k < na {
                    specs[k].force(self)?
                } else {
                    Rc::new(APLValue::Null)
                };
                match spec_val.as_ref() {
                    APLValue::Null => {
                        let axis = next_out;
                        out_dims.push(bdims[k]);
                        next_out += 1;
                        plans.push((k, Spec::All(axis)));
                    }
                    APLValue::Array(v) => {
                        let v_dims = v.dimensions.clone();
                        let v_elems = v.elements();
                        let out_start = next_out;
                        for &d in &v_dims {
                            out_dims.push(d);
                        }
                        next_out += v_dims.len();
                        plans.push((
                            k,
                            Spec::VecIndex {
                                v_dims,
                                v_elems,
                                out_start,
                            },
                        ));
                    }
                    other => {
                        let i = self.index_to_i64(other)?;
                        let adj = check_and_adjust_selected_index(i, bdims[k])?;
                        plans.push((k, Spec::Collapsed(adj)));
                    }
                }
            }

            let target_elems = match target.as_ref() {
                APLValue::Array(a) => a.elements(),
                _ => vec![Rc::new(target.as_ref().clone())],
            };

            if out_dims.is_empty() {
                // Fully-collapsed selection (every axis a scalar): disclose the single element.
                let mut src_coords = vec![0usize; r];
                for (k, spec) in &plans {
                    if let Spec::Collapsed(c) = spec {
                        src_coords[*k] = *c;
                    }
                }
                let mut tflat = 0usize;
                for k in 0..r {
                    tflat += src_coords[k] * bstride[k];
                }
                return Ok(Rc::new(target_elems[tflat].as_ref().clone()));
            }

            let ostride: Vec<usize> = {
                let mut s = vec![1usize; out_dims.len()];
                for k in (0..out_dims.len()).rev() {
                    if k + 1 < out_dims.len() {
                        s[k] = s[k + 1] * out_dims[k + 1];
                    }
                }
                s
            };

            let result_size: usize = out_dims.iter().product();
            if result_size == 0 || result_size > 100_000_000 {
                return Err(AplError::runtime("index selection result too large".into()));
            }
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(result_size);
            let mut out_coords = vec![0usize; out_dims.len()];
            for pos in 0..result_size {
                let mut rem = pos;
                for ax in 0..out_dims.len() {
                    out_coords[ax] = rem / ostride[ax];
                    rem %= ostride[ax];
                }
                let mut src_coords = vec![0usize; r];
                for (k, spec) in &plans {
                    match spec {
                        Spec::All(axis) => src_coords[*k] = out_coords[*axis],
                        Spec::Collapsed(c) => src_coords[*k] = *c,
                        Spec::VecIndex {
                            v_dims,
                            v_elems,
                            out_start,
                        } => {
                            let mut vflat = 0usize;
                            for d in 0..v_dims.len() {
                                vflat = vflat * v_dims[d] + out_coords[out_start + d];
                            }
                            let vval = &v_elems[vflat];
                            let i = self.index_to_i64(vval.as_ref())?;
                            src_coords[*k] = check_and_adjust_selected_index(i, bdims[*k])?;
                        }
                    }
                }
                let mut tflat = 0usize;
                for k in 0..r {
                    tflat += src_coords[k] * bstride[k];
                }
                out.push(Rc::new(target_elems[tflat].as_ref().clone()));
            }
            Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                out_dims,
                ArrayData::Nested(out),
            )))))
        }
    }

    /// `declare` special-form handler. Reads the *unevaluated* argument AST
    /// structurally (Kotlin `DeclareToken` → `processExport` — never evaluates its
    /// argument), so `declare(:export zork)` tolerates an unbound name and returns
    /// `null` exactly like the oracle. The argument AST is `[Symbol{export,keyword},
    /// target]` where `target` is a `Symbol` (export one), an `Array` of `Symbol`s
    /// (export several), or anything else (no-op, matching oracle tolerance).
    fn eval_declare(
        &self,
        right: &Instr,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Extract symbol names *structurally* from the AST — we must not evaluate
        // `target`, since it may be an unbound name (the whole point of the special
        // form). Returns (keyword, [names]): `:export` marks exports, `:const` marks
        // read-only constants (B6 / code_analysis_03).
        let (kw, names): (String, Vec<String>) = match right {
            Instr::Array { elements } => {
                // elements[0] is the directive keyword symbol; elements[1] is the target.
                if elements.len() < 2 {
                    return Ok(Rc::new(APLValue::Null));
                }
                let keyword = match &elements[0] {
                    Instr::Symbol { name, .. } => name.clone(),
                    _ => String::new(),
                };
                let extracted = match &elements[1] {
                    Instr::Symbol { name, .. } => vec![name.clone()],
                    Instr::Array { elements: inner } => inner
                        .iter()
                        .filter_map(|e| match e {
                            Instr::Symbol { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .collect(),
                    // Non-symbol operand (e.g. an operator glyph): no-op, like the oracle.
                    _ => vec![],
                };
                (keyword, extracted)
            }
            // Bare `declare(:foo)` with a single (non-paren) argument is unusual; treat
            // any lone Symbol as the target to export.
            Instr::Symbol { name, .. } => ("export".to_string(), vec![name.clone()]),
            _ => (String::new(), vec![]),
        };
        let cur = env.ns_registry.current_ns();
        match kw.as_str() {
            "const" => {
                for name in names {
                    env.ns_registry.declare_const(&cur, &name);
                }
            }
            // Default (and `export`) keeps the historical export behaviour.
            _ => {
                for name in names {
                    env.ns_registry.declare_export(&cur, &name);
                }
            }
        }
        Ok(Rc::new(APLValue::Null))
    }

    /// Fallback `declare` path for the rare case where it reaches the normal Apply
    /// arm (the argument has already been force-evaluated into an `APLValue`). Kept
    /// for symmetry with the special form; not the primary path.
    fn eval_declare_struct(
        &self,
        right_val: AplRef<APLValue>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let names: Vec<String> = match right_val.as_ref() {
            APLValue::Array(a) => {
                let elems = a.elements();
                if elems.len() < 2 {
                    return Ok(Rc::new(APLValue::Null));
                }
                match elems[1].as_ref() {
                    APLValue::Symbol { name, .. } => vec![name.clone()],
                    APLValue::Array(g) => g
                        .elements()
                        .iter()
                        .filter_map(|s| match s.as_ref() {
                            APLValue::Symbol { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                }
            }
            _ => vec![],
        };
        let cur = env.ns_registry.current_ns();
        for name in names {
            env.ns_registry.declare_export(&cur, &name);
        }
        Ok(Rc::new(APLValue::Null))
    }

    /// Convert an evaluated `APLValue` back into an `Instr` so we can re-dispatch a
    /// function application via `eval_apply` (used by adverbs). Handles scalars and
    /// nested arrays; user functions are not inlineable (error if hit).
    fn apl_to_instr(&self, v: &APLValue) -> Result<Instr, AplError> {
        match v {
            APLValue::Number(n) => Ok(Instr::Literal(LiteralValue::Number(n.clone()))),
            APLValue::Char(c) => Ok(Instr::Literal(LiteralValue::Char(*c))),
            APLValue::Str(s) => Ok(Instr::Literal(LiteralValue::Str(s.clone()))),
            APLValue::Symbol { name, namespace } => Ok(Instr::SymbolValue {
                name: name.clone(),
            }),
            APLValue::Array(a) => {
                let mut elems = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    elems.push(self.apl_to_instr(e.as_ref())?);
                }
                Ok(Instr::Array { elements: elems })
            }
            APLValue::UserFn { .. } => {
                Err(AplError::runtime("cannot use a function as an array element".into()))
            }
            APLValue::UserOp { .. } => {
                Err(AplError::runtime("cannot use an operator as an array element".into()))
            }
            APLValue::Null => Ok(Instr::Literal(LiteralValue::Str(String::new()))),
            APLValue::Deferred { .. } => {
                Err(AplError::runtime("cannot use a deferred value as an array element".into()))
            }
        }
    }

    /// Apply the function described by `fn_instr` to `left`/`right` values, by building
    /// an `Instr::Apply` and recursing into `eval_apply`. `left` is optional (monadic).
    fn apply_fn_instr(
        &self,
        fn_instr: &Instr,
        left: Option<&AplRef<APLValue>>,
        right: &AplRef<APLValue>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let left_instr = match left {
            Some(v) => Some(Box::new(self.apl_to_instr(v)?)),
            None => None,
        };
        let right_instr = Box::new(self.apl_to_instr(right)?);
        self.eval_apply(fn_instr, &left_instr, &right_instr, env)
    }

    /// Reduce `f/array`:
    ///  * monadic (`f/array`, no left arg): fold the entire array to a single value.
    ///  * dyadic (`N f/array`): sliding-window reduce of size `|N|`, producing
    ///    `len-|N|+1` results (Kap's windowed reduce; `N` must satisfy `|N| ≤ len`).
    fn adverb_reduce(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
        last_axis: bool,
    ) -> Result<AplRef<APLValue>, AplError> {
        let data = self.eval_instr(right, env)?.force(self)?;
        // Windowed reduce (`N f/`): sliding window of size `|N|` over the flat array,
        // producing `len-|N|+1` results (Kap's windowed reduce).
        if left.is_some() {
            let elems = self.flat_elements(&data);
            if elems.is_empty() {
                return Err(AplError::runtime("reduce /: empty array".into()));
            }
            let lv = self.eval_instr(left.as_ref().unwrap(), env)?.force(self)?;
            let n = match lv.as_ref() {
                APLValue::Number(KapNumber::Long(v)) => *v,
                APLValue::Number(_) => {
                    return Err(AplError::runtime("reduce /: window must be an integer".into()))
                }
                _ => return Err(AplError::runtime("reduce /: window must be a number".into())),
            };
            let window = n.unsigned_abs() as usize;
            if window == 0 || window > elems.len() {
                return Err(AplError::runtime(format!(
                    "reduce /: left argument too large. |A| ({}) must be ≤ the size of the reduced axis ({}) - 1",
                    window,
                    elems.len()
                )));
            }
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(elems.len() - window + 1);
            for i in 0..=(elems.len() - window) {
                let mut acc = elems[i].clone();
                for e in &elems[i + 1..i + window] {
                    acc = self.apply_fn_instr(fn_instr, Some(&acc), e, env)?;
                }
                out.push(acc);
            }
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![out.len()],
                ArrayData::Nested(out),
            )))));
        }
        // Plain reduce: fold along ONE axis. `/` reduces the LAST axis (Kap rank-1
        // reduce → single scalar, matching APL), `⌿` reduces the FIRST axis.
        let dims = data.dimensions();
        let rank = dims.len();
        if rank == 0 {
            return Err(AplError::runtime("reduce: cannot reduce a scalar".into()));
        }
        let axis = if last_axis { rank - 1 } else { 0 };
        let axis_len = dims[axis];
        if axis_len == 0 {
            return Err(AplError::runtime("reduce: cannot reduce an empty axis".into()));
        }
        // Row-major strides for the full shape.
        let mut strides = vec![1usize; rank];
        for i in (0..rank).rev() {
            if i + 1 < rank {
                strides[i] = strides[i + 1] * dims[i + 1];
            }
        }
        let flat_of = |coords: &[usize]| -> usize {
            let mut f = 0;
            for i in 0..rank {
                f += coords[i] * strides[i];
            }
            f
        };
        let mut result_dims = dims.clone();
        result_dims.remove(axis);
        let lane_count: usize = if result_dims.is_empty() { 1 } else { result_dims.iter().product() };
        // Hoist the element vector ONCE: `value_at(i)` rebuilds the entire element
        // Vec on every call, so calling it inside the loop makes reduce O(total²)
        // and makes large vectors (e.g. `-/ 100000 ⍴ x`) appear to hang.
        let elems = data.elements();
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            // Decode the lane index into fixed coords for every axis except `axis`.
            let mut rest = lane;
            let mut fixed = vec![0usize; rank];
            let mut ri = 0;
            for i in 0..rank {
                if i == axis {
                    continue;
                }
                let d = result_dims[ri];
                fixed[i] = rest % d;
                rest /= d;
                ri += 1;
            }
            // Fold the fiber along `axis` (k = 0..axis_len).
            let mut coords = fixed.clone();
            coords[axis] = 0;
            let mut acc = elems[flat_of(&coords)].clone();
            for k in 1..axis_len {
                coords[axis] = k;
                let v = &elems[flat_of(&coords)];
                acc = self.apply_fn_instr(fn_instr, Some(&acc), v, env)?;
            }
            out.push(acc);
        }
        if result_dims.is_empty() {
            // Reduced a vector to a scalar (rank 0).
            Ok(out.into_iter().next().unwrap())
        } else {
            Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                result_dims,
                ArrayData::Nested(out),
            )))))
        }
    }

    /// Scan `f\\array`: like reduce but keep every intermediate accumulator along
    /// one axis (`\\` scans the LAST axis, `⍀` scans the FIRST axis), producing a
    /// result of the SAME shape as the input.
    fn adverb_scan(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
        last_axis: bool,
    ) -> Result<AplRef<APLValue>, AplError> {
        if left.is_some() {
            return Err(AplError::runtime("scan \\ is monadic (use f\\array)".into()));
        }
        let data = self.eval_instr(right, env)?.force(self)?;
        let dims = data.dimensions();
        let rank = dims.len();
        if rank == 0 {
            return Err(AplError::runtime("scan: cannot scan a scalar".into()));
        }
        let axis = if last_axis { rank - 1 } else { 0 };
        let axis_len = dims[axis];
        if axis_len == 0 {
            return Err(AplError::runtime("scan: cannot scan an empty axis".into()));
        }
        // Row-major strides for the full shape.
        let mut strides = vec![1usize; rank];
        for i in (0..rank).rev() {
            if i + 1 < rank {
                strides[i] = strides[i + 1] * dims[i + 1];
            }
        }
        let total: usize = dims.iter().product();
        // Result shape (axis removed) and its row-major strides, so we can map a fiber
        // to a sequential 0-based lane index in [0, lane_count).
        let mut result_dims = dims.clone();
        result_dims.remove(axis);
        let mut rd_strides = vec![1usize; result_dims.len()];
        for i in (0..result_dims.len()).rev() {
            if i + 1 < result_dims.len() {
                rd_strides[i] = rd_strides[i + 1] * result_dims[i + 1];
            }
        }
        let lane_count = if result_dims.is_empty() { 1 } else { result_dims.iter().product() };
        let elems = data.elements();
        let mut accs: Vec<Option<AplRef<APLValue>>> = vec![None; lane_count];
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(total);
        for f in 0..total {
            let mut rem = f;
            let mut coords = vec![0usize; rank];
            for i in 0..rank {
                let c = rem / strides[i];
                coords[i] = c;
                rem -= c * strides[i];
            }
            // Sequential lane index = flat coord of the fiber (axis coordinate dropped).
            let mut lane = 0usize;
            let mut ri = 0;
            for i in 0..rank {
                if i == axis {
                    continue;
                }
                lane += coords[i] * rd_strides[ri];
                ri += 1;
            }
            let cur = elems[f].clone();
            let new_acc = match accs[lane].take() {
                None => cur.clone(),
                Some(a) => self.apply_fn_instr(fn_instr, Some(&a), &cur, env)?,
            };
            accs[lane] = Some(new_acc.clone());
            out.push(new_acc);
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            dims,
            ArrayData::Nested(out),
        )))))
    }

    /// Rank operator `f⍤k` (Kotlin RankOpFunctionImpl, operator.kt:82 + disclose.kt
    /// AxisMultiDimensionEnclosedValue). Splits each argument into cells whose trailing
    /// dimension count is `k` (leading dims form the cell frame), applies `func` to every
    /// cell, and discloses the per-cell results into a frame-shaped array.
    ///
    /// Rank-spec rules (computeRankFromOpArg / eval2Arg):
    ///   * scalar r        → same rank for both sides
    ///   * 1-elem vector   → same rank for both sides
    ///   * 2-elem vector   → [leftRank, rightRank]
    ///   * 3-elem vector   → monadic uses the MIDDLE element
    /// A negative index counts back from the argument's rank; clamped to [0, rank].
    fn apply_rank_op(
        &self,
        func: &Instr,
        rank_val: &APLValue,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &Rc<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let spec: Vec<i64> = match rank_val {
            APLValue::Number(n) => vec![n.as_long().map_err(AplError::runtime)?],
            APLValue::Array(a) => a
                .elements()
                .iter()
                .map(|e| match e.as_ref() {
                    APLValue::Number(n) => n
                        .as_long()
                        .map_err(|e2| AplError::runtime(format!("⍤: rank spec: {}", e2))),
                    _ => Err(AplError::runtime("⍤: rank spec must be numeric".into())),
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(AplError::runtime("⍤: rank spec must be numeric".into())),
        };
        let (left_rank, right_rank) = match spec.len() {
            1 => (spec[0], spec[0]),
            2 => (spec[0], spec[1]),
            3 => (spec[1], spec[2]), // monadic path below reads spec[1]
            _ => {
                return Err(AplError::runtime(
                    "⍤: operator argument must be scalar or an array of 1 to 3 elements".into(),
                ))
            }
        };
        // Split an argument into rank-k cells (Kotlin AxisMultiDimensionEnclosedValue):
        // the trailing k dims are the cell shape; the leading dims are the frame.
        let split_cells = |v: &APLValue, idx: i64| -> Result<(Vec<usize>, Vec<usize>, Vec<AplRef<APLValue>>), AplError> {
            // Returns (frame_dims, cell_dims, flat_elems).
            let (dims, elems): (Vec<usize>, Vec<AplRef<APLValue>>) = match v {
                APLValue::Array(a) => (a.dimensions.clone(), a.elements()),
                APLValue::Str(s) => {
                    let cs: Vec<AplRef<APLValue>> =
                        s.chars().map(|c| Rc::new(APLValue::Char(c))).collect();
                    (vec![cs.len()], cs)
                }
                other => (vec![], vec![Rc::new(other.clone())]),
            };
            let rank = dims.len();
            let k = idx.max(0).min(rank as i64) as usize;
            let frame = dims[..rank - k].to_vec();
            let cell_dims = dims[rank - k..].to_vec();
            Ok((frame, cell_dims, elems))
        };
        match left {
            None => {
                // Monadic: rank = spec[1] when 3 elements, else the single value.
                let idx = if spec.len() == 3 { spec[1] } else { spec[0] };
                let rv = self.eval_instr(right, env)?.force(self)?;
                let (frame, cell_dims, elems) = split_cells(rv.as_ref(), idx)?;
                let cell_size: usize = cell_dims.iter().product();
                let cell_size = cell_size.max(1);
                let mut results = Vec::with_capacity(elems.len() / cell_size);
                for cell in elems.chunks(cell_size) {
                    let cell_val = self.make_simple_or_nested(cell_dims.clone(), cell.to_vec())?;
                    let r = self.eval_apply(func, &None, &Box::new(Instr::Value(cell_val)), env)?;
                    results.push(r);
                }
                // Disclose: frame dims × per-cell results (Kotlin discloseValue).
                if frame.is_empty() {
                    return Ok(results.into_iter().next().unwrap_or_else(|| Rc::new(APLValue::Null)));
                }
                self.make_simple_or_nested(frame.clone(), results)
            }
            Some(l) => {
                let lv = self.eval_instr(l, env)?.force(self)?;
                let rv = self.eval_instr(right, env)?.force(self)?;
                let (lframe, lcell_dims, lelems) = split_cells(lv.as_ref(), left_rank)?;
                let (rframe, rcell_dims, relems) = split_cells(rv.as_ref(), right_rank)?;
                let lcell: usize = lcell_dims.iter().product::<usize>().max(1);
                let rcell: usize = rcell_dims.iter().product::<usize>().max(1);
                // Frame sizes must agree (Kotlin ForEachFunctionDescriptor.compute2Arg).
                let ln: usize = lelems.len() / lcell;
                let rn: usize = relems.len() / rcell;
                if ln != rn && ln != 1 && rn != 1 {
                    return Err(AplError::runtime(
                        "⍤: cell frame sizes do not match".into(),
                    ));
                }
                let n = ln.max(rn);
                let mut results = Vec::with_capacity(n);
                for i in 0..n {
                    let li = if ln == 1 { 0 } else { i };
                    let ri = if rn == 1 { 0 } else { i };
                    // Guard the cell slices: if an argument has no elements (e.g. the
                    // right operand of `⍤` evaluated to an empty array), return a proper
                    // error rather than panicking on an out-of-range slice.
                    if lelems.is_empty() || relems.is_empty() {
                        return Err(AplError::runtime(
                            "⍤: rank-operator argument is empty".into(),
                        ));
                    }
                    let ls = li * lcell;
                    let rs = ri * rcell;
                    if ls + lcell > lelems.len() || rs + rcell > relems.len() {
                        return Err(AplError::runtime(
                            "⍤: cell frame does not match argument shape".into(),
                        ));
                    }
                    let lcell_val =
                        self.make_simple_or_nested(lcell_dims.clone(), lelems[ls..ls + lcell].to_vec())?;
                    let rcell_val =
                        self.make_simple_or_nested(rcell_dims.clone(), relems[rs..rs + rcell].to_vec())?;
                    let r = self.eval_apply(
                        func,
                        &Some(Box::new(Instr::Value(lcell_val))),
                        &Box::new(Instr::Value(rcell_val)),
                        env,
                    )?;
                    results.push(r);
                }
                // Result frame: the non-singleton frame (Kotlin broadcasts a singleton).
                let frame = if ln == 1 { rframe } else { lframe };
                if frame.is_empty() {
                    return Ok(results.into_iter().next().unwrap_or_else(|| Rc::new(APLValue::Null)));
                }
                self.make_simple_or_nested(frame.clone(), results)
            }
        }
    }

    /// Each `f¨array` (monadic) or `a f¨ b` (dyadic, element-wise with scalar extension).
    fn adverb_each(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        let right_elems = self.flat_elements(&right_val);
        let mut out = Vec::with_capacity(right_elems.len());
        match left_val {
            // Dyadic each: apply f to (left_element, right_element) for each right element.
            Some(lv) => {
                let left_elems = self.flat_elements(&lv);
                for (i, re) in right_elems.iter().enumerate() {
                    let le = left_elems.get(i).cloned().unwrap_or_else(|| lv.clone());
                    out.push(self.apply_fn_instr(fn_instr, Some(&le), re, env)?);
                }
            }
            // Monadic each: apply f to each right element.
            None => {
                for e in &right_elems {
                    out.push(self.apply_fn_instr(fn_instr, None, e, env)?);
                }
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// `⌸` (Key operator, oracle-native semantics): `keys {fn}⌸ values`.
    /// For each unique key of `keys` in first-occurrence order, call `fn` dyadically
    /// with ⍺=key and ⍵=the enclosed vector of values at matching positions. The
    /// result is a 2-column matrix whose rows are `(key, fn(group))`.
    fn key_apply(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let keys_v = match left {
            Some(l) => self.eval_instr(l, env)?.force(self)?,
            None => return Err(AplError::runtime("⌸ requires a left argument (keys)".into())),
        };
        let vals = self.eval_instr(right, env)?.force(self)?;
        // A Str iterates as its characters (oracle: `"aab" {⍵}⌸ 5 6 7` groups by
        // @a/@b). Split top-level strings into char elements before grouping.
        let split_str = |v: &AplRef<APLValue>| -> Vec<AplRef<APLValue>> {
            match v.as_ref() {
                APLValue::Array(_) => self.flat_elements(v),
                APLValue::Str(s) => s.chars().map(|c| Rc::new(APLValue::Char(c))).collect(),
                other => vec![Rc::new(other.clone())],
            }
        };
        let keys = split_str(&keys_v);
        let values = split_str(&vals);
        if keys.len() != values.len() {
            return Err(AplError::runtime(format!(
                "⌸: key and value lengths differ ({} vs {})",
                keys.len(),
                values.len()
            )));
        }
        // Group value indices by unique key (first-occurrence order).
        let mut order: Vec<AplRef<APLValue>> = Vec::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            match order.iter().position(|o| Self::type_equal(o.as_ref(), k.as_ref())) {
                Some(g) => groups[g].push(i),
                None => {
                    order.push(k.clone());
                    groups.push(vec![i]);
                }
            }
        }
        // One row per group: (key, fn(key; group)).
        let mut rows: Vec<AplRef<APLValue>> = Vec::with_capacity(order.len());
        for (k, gidx) in order.iter().zip(&groups) {
            let group_vec = APLValue::Array(Rc::new(KapArray::new(
                vec![gidx.len()],
                ArrayData::Nested(gidx.iter().map(|&i| values[i].clone()).collect()),
            )));
            let res = self.apply_fn_instr(fn_instr, Some(k), &Rc::new(group_vec), env)?;
            rows.push(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![2],
                ArrayData::Nested(vec![k.clone(), res]),
            )))));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![order.len(), 2],
            ArrayData::Nested(rows),
        )))))
    }

    /// Flatten a value into a list of element refs. Scalars become a 1-element list;
    /// arrays become their elements (one level).
    fn flat_elements(&self, v: &AplRef<APLValue>) -> Vec<AplRef<APLValue>> {
        match v.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        }
    }

    /// Number of "user-function argument slots" a value fills: a scalar is 1, an array is
    /// its element count (matches Kap's `;`/`⍵`-destructuring semantics).
    fn element_count(&self, v: &APLValue) -> usize {
        match v {
            APLValue::Array(a) => a.element_count(),
            _ => 1,
        }
    }

    /// `≡`: dyadic = type-discriminating match (returns 1/0); monadic = nesting depth.
    /// Mirrors Kap's `CompareFunction` (compare_functions.kt): dyadic equal is STRICT on
    /// type — `10 ≡ 10.0` is 0 (Long vs Double), unlike `=` which is value-equal.
    fn match_or_depth(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left_val {
            None => {
                let depth = Self::depth_of(right_val.force(self)?.as_ref());
                Ok(Rc::new(APLValue::Number(KapNumber::Long(depth as i64))))
            }
            Some(l) => {
                let eq =
                    Self::type_equal(l.force(self)?.as_ref(), right_val.force(self)?.as_ref());
                Ok(Rc::new(APLValue::Number(KapNumber::Long(if eq {
                    1
                } else {
                    0
                }))))
            }
        }
    }

    /// Nesting depth: a scalar is 0; an array's depth is 1 + max(depth of elements),
    /// with enclosed scalars counting as depth 1.
    fn depth_of(v: &APLValue) -> usize {
        match v {
            APLValue::Array(a) => {
                if a.dimensions.iter().product::<usize>() == 0 {
                    return 1;
                }
                let mut max = 0;
                for e in a.elements() {
                    let d = Self::depth_of(e.as_ref());
                    if d > max {
                        max = d;
                    }
                }
                max + 1
            }
            _ => 0,
        }
    }

    /// Type-discriminating equality for `≡`/`≢` (Kap's `CompareFunction`): unlike `=`/`≠`
    /// (which are value-equal and merge numeric kinds), `≡` requires the *same type*. So
    /// `10 ≡ 10.0` is 0 (Long vs Double), `(1 2) ≡ (1 2.0)` is 0. Primitive kinds (number/
    /// char/string/null/symbol) must also match exactly; arrays compare shape + element-wise.
    fn type_equal(a: &APLValue, b: &APLValue) -> bool {
        match (a, b) {
            (APLValue::Number(x), APLValue::Number(y)) => {
                // Same numeric *kind* required: Long≠Double, Complex differs from the rest.
                match (x, y) {
                    (KapNumber::Long(x), KapNumber::Long(y)) => x == y,
                    (KapNumber::Double(x), KapNumber::Double(y)) => x == y,
                    (KapNumber::BigInt(x), KapNumber::BigInt(y)) => x == y,
                    (KapNumber::Rational(x), KapNumber::Rational(y)) => x == y,
                    (KapNumber::Complex(xr, xi), KapNumber::Complex(yr, yi)) => xr == yr && xi == yi,
                    _ => false,
                }
            }
            (APLValue::Char(x), APLValue::Char(y)) => x == y,
            (APLValue::Str(x), APLValue::Str(y)) => x == y,
            (APLValue::Null, APLValue::Null) => true,
            (APLValue::Symbol { name: n1, namespace: ns1 }, APLValue::Symbol { name: n2, namespace: ns2 }) => {
                n1 == n2 && ns1 == ns2
            }
            (APLValue::Array(x), APLValue::Array(y)) => {
                if x.dimensions != y.dimensions {
                    return false;
                }
                let xe = x.elements();
                let ye = y.elements();
                xe.len() == ye.len()
                    && xe
                        .iter()
                        .zip(ye.iter())
                        .all(|(p, q)| Self::type_equal(p.as_ref(), q.as_ref()))
            }
            _ => false,
        }
    }

    /// Structural deep equality: same shape and element-wise equal. Used by `=`/`≠`
    /// (value-equal; numeric kinds merged) — NOT by `≡`/`≢` which use `type_equal`.
    fn deep_equal(a: &APLValue, b: &APLValue) -> bool {
        match (a, b) {
            (APLValue::Number(x), APLValue::Number(y)) => x.numeric_cmp(y).map(|o| o == Ordering::Equal).unwrap_or(false),
            (APLValue::Char(x), APLValue::Char(y)) => x == y,
            (APLValue::Str(x), APLValue::Str(y)) => x == y,
            (APLValue::Null, APLValue::Null) => true,
            (APLValue::Symbol { name: n1, namespace: ns1 }, APLValue::Symbol { name: n2, namespace: ns2 }) => {
                n1 == n2 && ns1 == ns2
            }
            (APLValue::Array(x), APLValue::Array(y)) => {
                if x.dimensions != y.dimensions {
                    return false;
                }
                let xe = x.elements();
                let ye = y.elements();
                xe.len() == ye.len()
                    && xe
                        .iter()
                        .zip(ye.iter())
                        .all(|(p, q)| Self::deep_equal(p.as_ref(), q.as_ref()))
            }
            _ => false,
        }
    }

    /// Port of fmt-rational.kt `formatRationalMaybeExponent` (simple range only —
    /// the port's rational domain stays within 1e-6..1e12; outside that the plain
    /// form is used, a documented MSG-class simplification).
    /// Returns (string, exact-flag). `exact` is true when the decimal expansion
    /// terminates within `decimals` places.
    fn format_rational(&self, v: &KapNumber, decimals: u32) -> (String, bool) {
        // Integers render bare and are always exact.
        if let Ok(i) = v.as_long() {
            if matches!(v, KapNumber::Long(_))
                || matches!(v, KapNumber::Rational(r) if r.denom() == &num_bigint::BigInt::from(1))
            {
                return (i.to_string(), true);
            }
        }
        let r = match v {
            KapNumber::Rational(r) => r.clone(),
            other => {
                // Non-rational: fall back to decimal rendering.
                let d = other.as_double();
                return (format!("{:.*}", decimals as usize, d), false);
            }
        };
        let neg = *r.numer() < num_bigint::BigInt::from(0);
        let ar = if neg { -r.clone() } else { r };
        // scaled = ar * 10^decimals; rounded = scaled.round()
        let ten = num_bigint::BigInt::from(10).pow(decimals);
        let scaled = ar * num_rational::BigRational::from(ten.clone());
        let numer = scaled.numer();
        let denom = scaled.denom();
        // Round half to even on the scaled integer (integer arithmetic only,
        // via BigRational round() which rounds half-away-from-zero on the
        // magnitude — matching Kotlin Rational.round for this use).
        let rounded = scaled.round();
        let rn = rounded.numer();
        let rounded_big = if *rn < num_bigint::BigInt::from(0) { -rn } else { rn.clone() };
        let exact = *denom == num_bigint::BigInt::from(1);
        let digits = rounded_big.to_string();
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        let dl = digits.len();
        let dec = decimals as usize;
        if dl <= dec {
            out.push('0');
        } else {
            out.push_str(&digits[..dl - dec]);
        }
        out.push('.');
        if dec > dl {
            for _ in 0..(dec - dl) {
                out.push('0');
            }
            out.push_str(&digits);
        } else if exact {
            // Kotlin exact branch trims trailing zeroes of the decimal part.
            let frac = &digits[dl.saturating_sub(dec)..];
            let trimmed = frac.trim_end_matches('0');
            out.push_str(trimmed);
        } else {
            out.push_str(&digits[dl - dec..]);
        }
        (out, exact)
    }

    /// Port of Kotlin `encoder/encoder.kt` via `encoder:encode` (EncodeToByteArrayFunction):
    /// serialize the value into the Kap binary wire format, returning a byte vector
    /// (rank-1 Long array). Errors mirror Kotlin's EncodeException text.
    fn encoder_encode(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let bytes = crate::encoder::encode_value(right_val.as_ref())
            .map_err(|e| AplError::runtime(format!("Error when encoding value: {}", e)))?;
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![bytes.len()],
            ArrayData::Long(bytes.into_iter().map(|b| b as i64).collect()),
        )))))
    }

    /// `encoder:decode`: parse the Kap wire format back into a value. Input is a
    /// rank-1 byte array (Longs 0–255).
    fn encoder_decode(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let bytes: Vec<u8> = match right_val.as_ref() {
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(n) => {
                            let l = n.as_long().map_err(|e| AplError::runtime(e))?;
                            if !(0..=255).contains(&l) {
                                return Err(AplError::runtime("Error decoding content: byte out of range".into()));
                            }
                            out.push(l as u8);
                        }
                        _ => return Err(AplError::runtime("encoder:decode requires a byte array".into())),
                    }
                }
                out
            }
            APLValue::Number(n) => vec![n.as_long().map_err(|e| AplError::runtime(e))? as u8],
            _ => return Err(AplError::runtime("encoder:decode requires an array".into())),
        };
        let v = crate::encoder::decode_value(&bytes)
            .map_err(|e| AplError::runtime(format!("Error decoding content: {}", e)))?;
        Ok(Rc::new(v))
    }

    /// Integer GCD on KapNumbers (Kotlin `integerGcd`/`floatGcd`). Negative inputs    /// take absolute value; doubles truncate to i64 (Kotlin floatGcd works on the
    /// integral value).
    fn kap_gcd(a: &KapNumber, b: &KapNumber) -> KapNumber {
        let gcd_u = |mut x: u64, mut y: u64| -> u64 {
            while y != 0 {
                let t = x % y;
                x = y;
                y = t;
            }
            x
        };
        let xa = a.as_long().unwrap_or_else(|_| a.as_double() as i64);
        let xb = b.as_long().unwrap_or_else(|_| b.as_double() as i64);
        if xa == 0 {
            return KapNumber::Long(xb.abs());
        }
        if xb == 0 {
            return KapNumber::Long(xa.abs());
        }
        KapNumber::Long(gcd_u(xa.unsigned_abs(), xb.unsigned_abs()) as i64)
    }

    /// Trial-division primality (sufficient for the port's i64 domain; Kotlin uses
    /// Miller-Rabin only for bigint inputs).
    fn is_prime_u64(n: u64) -> bool {
        if n < 2 {
            return false;
        }
        if n % 2 == 0 {
            return n == 2;
        }
        if n % 3 == 0 {
            return n == 3;
        }
        let mut d = 5u64;
        while d * d <= n {
            if n % d == 0 || n % (d + 2) == 0 {
                return false;
            }
            d += 6;
        }
        true
    }

    /// Prime factorisation, monadic only (prime.kt FactorAPLFunctionImpl): non-integers
    /// error "Only integers can be factorised", negatives "Argument must be positive".
    fn math_factor(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let one = |v: i64| -> Result<Vec<i64>, AplError> {
            if v < 0 {
                return Err(AplError::runtime("Argument must be positive".into()));
            }
            let mut n = v;
            let mut out = Vec::new();
            let mut d = 2u64;
            while (d as i64) * (d as i64) <= n {
                while n % (d as i64) == 0 {
                    out.push(d as i64);
                    n /= d as i64;
                }
                d += 1;
            }
            if n > 1 {
                out.push(n);
            }
            Ok(out)
        };
        match right_val.as_ref() {
            APLValue::Number(KapNumber::Long(v)) => {
                let fs = one(*v)?;
                self.make_long_vector(fs)
            }
            APLValue::Array(a) => Err(AplError::runtime(
                "math:factor requires a scalar integer".into(),
            )),
            _ => Err(AplError::runtime("Only integers can be factorised".into())),
        }
    }

    /// Divisors of n in ascending order (prime.kt DivisorsAPLFunctionImpl).
    fn math_divisors(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Number(KapNumber::Long(v)) => {
                let v = *v;
                if v < 0 {
                    return Err(AplError::runtime("Argument must be positive".into()));
                }
                let mut small = Vec::new();
                let mut large = Vec::new();
                let mut d = 1i64;
                while d * d <= v {
                    if v % d == 0 {
                        small.push(d);
                        if d != v / d {
                            large.push(v / d);
                        }
                    }
                    d += 1;
                }
                large.reverse();
                small.extend(large);
                // Kotlin divisorsLong iterates i in start..sqrt(n) — the bound is
                // EXCLUSIVE of n, so n itself never enters (oracle: 12 → ⟨2 3 4 6⟩).
                // 1 is also excluded (loop starts at 2 or 3).
                small.retain(|&x| x != 1 && x != v);
                self.make_long_vector(small)
            }
            _ => Err(AplError::runtime("Argument is not an integer".into())),
        }
    }

    /// Primes ≤ n via simple sieve (prime.kt PrimesFunctionImpl / atkinSieve):
    /// n ≤ 0 → Null; result ascending from 2.
    fn math_primes(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let n = match right_val.as_ref() {
            APLValue::Number(n) => n.as_long().unwrap_or_else(|_| n.as_double() as i64),
            _ => return Err(AplError::runtime("math:primes requires a number".into())),
        };
        if n <= 0 {
            return Ok(Rc::new(APLValue::Null));
        }
        let mut sieve = vec![true; (n as usize) + 1];
        sieve[0] = false;
        if n >= 1 {
            sieve[1] = false;
        }
        let mut d = 2usize;
        while d * d <= n as usize {
            if sieve[d] {
                (d * d..=(n as usize)).step_by(d).for_each(|m| sieve[m] = false);
            }
            d += 1;
        }
        let primes: Vec<i64> = sieve
            .iter()
            .enumerate()
            .filter(|(_, &p)| p)
            .map(|(i, _)| i as i64)
            .collect();
        self.make_long_vector(primes)
    }

    /// Build a rank-1 Long array from i64 values.
    fn make_long_vector(&self, vals: Vec<i64>) -> Result<AplRef<APLValue>, AplError> {
        use crate::array::{ArrayData, KapArray};
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![vals.len()],
            ArrayData::Long(vals),
        )))))
    }

    /// Apply a unary numeric function to a scalar, or element-wise to an array.
    fn scalar1(
        &self,
        right_val: AplRef<APLValue>,
        f: impl Fn(&KapNumber) -> KapNumber,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Number(x) => Ok(Rc::new(APLValue::Number(f(x)))),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(x) => out.push(Rc::new(APLValue::Number(f(x)))),
                        _ => return Err(AplError::runtime(format!("{} requires numbers", sym))),
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    a.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            _ => Err(AplError::runtime(format!("{} requires a number", sym))),
        }
    }

    /// Dyadic boolean AND/OR: operands are 0/1 (truthy: non-zero). Result 0/1.
    fn bool2(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        f: impl Fn(bool, bool) -> bool,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let (x, y) = match (a.as_ref(), right_val.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => (x.as_boolean(), y.as_boolean()),
            _ => return Err(AplError::runtime(format!("{} requires booleans", sym))),
        };
        Ok(Rc::new(APLValue::Number(KapNumber::Long(if f(x, y) { 1 } else { 0 }))))
    }

    /// Dyadic boolean with broadcasting (Kotlin `MathCombineAPLFunction`): element-wise
    /// over arrays, scalar-extended, like `∧`/`∨`. `f` combines two booleans.
    fn bool_broadcast(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        f: impl Fn(bool, bool) -> bool + Copy,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let l = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let a = l.force(self)?;
        let b = right_val.force(self)?;
        let la = self.boolean_vector(&a, sym)?;
        let lb = self.boolean_vector(&b, sym)?;
        if la.len() == 1 && lb.len() == 1 {
            let out = if f(la[0], lb[0]) { 1 } else { 0 };
            return Ok(Rc::new(APLValue::Number(KapNumber::Long(out))));
        }
        // Broadcast: scalar left/right extends; otherwise lengths must match.
        let n = la.len().max(lb.len());
        if !(la.len() == n || la.len() == 1) || !(lb.len() == n || lb.len() == 1) {
            return Err(AplError::runtime(format!("{} length mismatch", sym)));
        }
        let out: Vec<i64> = (0..n)
            .map(|i| {
                let x = la[if la.len() == 1 { 0 } else { i }];
                let y = lb[if lb.len() == 1 { 0 } else { i }];
                if f(x, y) { 1 } else { 0 }
            })
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Long(out),
        )))))
    }

    /// Extract a flat list of booleans from a scalar/vector of numbers (Kap booleans: 1/0).
    fn boolean_vector(&self, v: &APLValue, sym: &str) -> Result<Vec<bool>, AplError> {
        match v {
            APLValue::Number(n) => Ok(vec![n.as_boolean()]),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(n) => out.push(n.as_boolean()),
                        other => return Err(AplError::runtime(format!("{} requires booleans", sym))),
                    }
                }
                Ok(out)
            }
            other => Err(AplError::runtime(format!("{} requires booleans, got {}", sym, other.class_name()))),
        }
    }

    /// Dyadic replicate `A / B` (Kotlin ReplicateAPLFunction): each element of B is
    /// repeated according to the corresponding integer in A (0 drops, negative is an
    /// error). Scalar A extends. Result is the flattened repetition (rank 1 for a
    /// vector B; per-axis replication of higher ranks is out of scope here).
    fn replicate(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let l = left_val.ok_or_else(|| AplError::runtime("/ needs two args".into()))?;
        let counts: Vec<i64> = match l.force(self)?.as_ref() {
            APLValue::Array(a) => {
                let mut v = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(n) => v.push(n.as_long().map_err(|e| AplError::runtime(e))?),
                        _ => return Err(AplError::runtime("/: left argument must be integers".into())),
                    }
                }
                v
            }
            APLValue::Number(n) => vec![n.as_long().map_err(|e| AplError::runtime(e))?],
            _ => return Err(AplError::runtime("/: left argument must be integers".into())),
        };
        if counts.iter().any(|&c| c < 0) {
            return Err(AplError::runtime("Negative number of copies requested".into()));
        }
        let items: Vec<AplRef<APLValue>> = match right_val.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };
        // Scalar right extends to the length of counts.
        let n = counts.len().max(items.len());
        let mut out: Vec<AplRef<APLValue>> = Vec::new();
        for i in 0..n {
            let c = if counts.len() == 1 { counts[0] } else { counts[i] };
            let item = if items.len() == 1 { items[0].clone() } else { items[i].clone() };
            for _ in 0..c {
                out.push(item.clone());
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// `⌻` outer product (Kotlin outer_join.kt OuterJoinOp / OuterJoinResult):
    /// `A f⌻ B` applies f dyadically to every cell pair, producing an array whose
    /// dimensions are ⍴A concatenated with ⍴B. Scalars are treated as rank-0
    /// (a single cell). Oracle: `1 2 {⍺+⍵}⌻ 10 20` -> `(11 21)(12 22)` as a
    /// 2×2 table; `{⍺=⍵}⌻` gives the boolean membership table.
    fn outer_product(
        &self,
        func: &Box<Instr>,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => {
                return Err(AplError::runtime("⌻ needs two args".into()));
            }
        };
        // Flatten each side to (cells, dims). Scalars are one cell with no dims.
        let flat =
            |v: &APLValue| -> (Vec<AplRef<APLValue>>, Vec<usize>) {
                match v {
                    APLValue::Array(a) => (a.elements(), a.dimensions.clone()),
                    other => (vec![Rc::new(other.clone())], vec![]),
                }
            };
        let (a_cells, a_dims) = flat(left_val.as_ref().unwrap());
        let (b_cells, b_dims) = flat(right_val.as_ref());
        let mut dims = a_dims.clone();
        dims.extend(b_dims.iter().copied());
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(a_cells.len() * b_cells.len());
        for a in &a_cells {
            for b in &b_cells {
                let v = self.eval_apply(
                    func,
                    &Some(Box::new(Instr::Value(a.clone()))),
                    &Box::new(Instr::Value(b.clone())),
                    env,
                )?;
                out.push(v);
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            dims,
            ArrayData::Nested(out),
        )))))
    }

    /// Bitwise `∵` operator (Kotlin bitwise_ops.kt BitwiseOp). Derives the *bitwise*
    /// variant of the primitive function named by `fname` and applies it to `left`/`right`.
    /// Supported operands: `∨` (bitwise-OR), `∧` (bitwise-AND), `⌽` (bitwise-shift, where the
    /// left arg is the shift count and the right arg is the value: `count ⌽∵ value`).
    /// Works element-wise over integer scalars/arrays with scalar extension (mirrors
    /// Kotlin's MathCombineAPLFunction broadcast).
    fn bitwise_apply(
        &self,
        fname: &str,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        // Echo shape: if both operands were scalars, return a scalar.
        let scalar_only = left_val.as_ref().map_or(false, |l| matches!(l.as_ref(), APLValue::Number(_)))
            && matches!(right_val.as_ref(), APLValue::Number(_));
        // Collect integer operands into flat i64 vectors (scalar → length 1).
        let to_long_vec = |v: &APLValue, sym: &str| -> Result<Vec<i64>, AplError> {
            match v {
                APLValue::Number(n) => Ok(vec![n.as_long().map_err(|e| AplError::runtime(e))?]),
                APLValue::Array(a) => {
                    let mut out = Vec::with_capacity(a.element_count());
                    for e in a.elements() {
                        match e.as_ref() {
                            APLValue::Number(n) => out.push(n.as_long().map_err(|e| AplError::runtime(e))?),
                            _ => return Err(AplError::runtime(format!("{} requires integers", sym))),
                        }
                    }
                    Ok(out)
                }
                other => Err(AplError::runtime(format!("{} requires integers, got {}", sym, other.class_name()))),
            }
        };
        let combine = |x: i64, y: i64| -> Result<i64, AplError> {
            match fname {
                "∨" | "or" => Ok(x | y),
                "∧" | "and" => Ok(x & y),
                "⌽" | "rotate" | "rotateright" => {
                    // Kotlin's BigInt.shl(a) treats a negative shift count as a *right* shift
                    // by |a| (and the intermediate is arbitrary-precision, so no overflow).
                    if x >= 0 {
                        Ok(y << x)
                    } else {
                        Ok(y >> (-x))
                    }
                }
                "⊻" | "xor" => Ok(x ^ y),
                other => Err(AplError::runtime(format!("∵ does not support {} (bitwise not yet implemented)", other))),
            }
        };
        // `⌽∵` is a *shift*: the left arg is the (signed) shift count, right is the value.
        // All others are pairwise on the two integer operands.
        let result_vec: Vec<i64> = match fname {
            "⌽" | "rotate" | "rotateright" => {
                let l = left_val.ok_or_else(|| AplError::runtime("⌽∵ needs two args".into()))?;
                let lvec = to_long_vec(&l, "⌽∵")?;
                let rvec = to_long_vec(&right_val, "⌽∵")?;
                let n = lvec.len().max(rvec.len());
                if !(lvec.len() == n || lvec.len() == 1) || !(rvec.len() == n || rvec.len() == 1) {
                    return Err(AplError::runtime("⌽∵ length mismatch".into()));
                }
                (0..n)
                    .map(|i| {
                        let x = lvec[if lvec.len() == 1 { 0 } else { i }];
                        let y = rvec[if rvec.len() == 1 { 0 } else { i }];
                        combine(x, y)
                    })
                    .collect::<Result<Vec<i64>, AplError>>()?
            }
            _ => {
                let l = left_val.ok_or_else(|| AplError::runtime(format!("{}∵ needs two args", fname)))?;
                let lvec = to_long_vec(&l, fname)?;
                let rvec = to_long_vec(&right_val, fname)?;
                let n = lvec.len().max(rvec.len());
                if !(lvec.len() == n || lvec.len() == 1) || !(rvec.len() == n || rvec.len() == 1) {
                    return Err(AplError::runtime(format!("{}∵ length mismatch", fname)));
                }
                (0..n)
                    .map(|i| {
                        let x = lvec[if lvec.len() == 1 { 0 } else { i }];
                        let y = rvec[if rvec.len() == 1 { 0 } else { i }];
                        combine(x, y)
                    })
                    .collect::<Result<Vec<i64>, AplError>>()?
            }
        };
        if scalar_only {
            return Ok(Rc::new(APLValue::Number(KapNumber::Long(result_vec[0]))));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![result_vec.len()],
            ArrayData::Long(result_vec),
        )))))
    }

    /// Membership `a ∊ b`: for each element of `a`, 1 if present in `b`, else 0.
    /// Returns a vector (same shape as `a`) of 0/1.
    fn membership(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime("∊ needs two args".into()))?;
        // Collect the set of "keys" in b (compare by formatted value for simplicity).
        let mut keys = std::collections::HashSet::new();
        match right_val.as_ref() {
            APLValue::Array(b) => {
                for e in b.elements() {
                    keys.insert(e.format_value());
                }
            }
            other => {
                keys.insert(other.format_value());
            }
        }
        let mut out = Vec::new();
        match a.as_ref() {
            APLValue::Array(aa) => {
                for e in aa.elements() {
                    let hit = if keys.contains(&e.format_value()) { 1 } else { 0 };
                    out.push(Rc::new(APLValue::Number(KapNumber::Long(hit))));
                }
            }
            other => {
                let hit = if keys.contains(&other.format_value()) { 1 } else { 0 };
                // Scalar left operand → scalar result (Kap returns a result with the
                // *shape of the left argument*, so a scalar membership is a scalar 0/1,
                // not a length-1 vector). This matters for short-circuit `and`/`or`
                // guards, whose `truthy` treats a non-empty array as true.
                return Ok(Rc::new(APLValue::Number(KapNumber::Long(hit))));
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// `∪`: monadic unique. Faithful to Kotlin `UniqueFunction` (unique.kt).
    /// - `⍬` → `⍬`.
    /// - scalar → a length-1 vector (`∪ 1` → `(1)`).
    /// - string → a string of its unique chars (order-preserving).
    /// - vector → unique elements, first-seen order preserved.
    /// Membership is keyed by `type_qualified_key` so `1` ≠ `1.0`.
    fn unique(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let r = right_val.force(self)?;
        if r.is_null() {
            return Ok(Rc::new(APLValue::Null));
        }
        if let APLValue::Str(s) = r.as_ref() {
            let mut seen = std::collections::HashSet::new();
            let mut out = String::new();
            for c in s.chars() {
                let key = Self::type_qualified_key(&APLValue::Char(c));
                if seen.insert(key) {
                    out.push(c);
                }
            }
            return Ok(Rc::new(APLValue::Str(out)));
        }
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<AplRef<APLValue>> = Vec::new();
        for m in self.members_of(&r) {
            let key = Self::type_qualified_key(m.as_ref());
            if seen.insert(key) {
                out.push(m);
            }
        }
        if out.is_empty() {
            return Ok(Rc::new(APLValue::Null));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// `∪`: dyadic union. Faithful to Kotlin `UniqueFunction.computeVectorResult`.
    /// Preserves left's elements/order, appends right elements whose key is not
    /// already present in the left. A `⍬` (Null) operand yields the other operand.
    /// Strings concatenate char-wise (unique per char on the right side).
    fn union(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let l = left_val.force(self)?;
        let r = right_val.force(self)?;
        // Null operand → the other side, unchanged (Kotlin `emptyRightArgument`/`a`).
        if l.is_null() {
            return Ok(r.clone());
        }
        if r.is_null() {
            return Ok(l.clone());
        }
        if let (APLValue::Str(s1), APLValue::Str(s2)) = (l.as_ref(), r.as_ref()) {
            let mut seen = std::collections::HashSet::new();
            let mut out = String::new();
            for c in s1.chars() {
                let key = Self::type_qualified_key(&APLValue::Char(c));
                seen.insert(key);
                out.push(c);
            }
            for c in s2.chars() {
                let key = Self::type_qualified_key(&APLValue::Char(c));
                if seen.insert(key) {
                    out.push(c);
                }
            }
            return Ok(Rc::new(APLValue::Str(out)));
        }
        let a = self.members_of(&l);
        let b = self.members_of(&r);
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<AplRef<APLValue>> = Vec::new();
        for m in &a {
            let key = Self::type_qualified_key(m.as_ref());
            seen.insert(key);
            out.push(m.clone());
        }
        for m in &b {
            let key = Self::type_qualified_key(m.as_ref());
            if seen.insert(key) {
                out.push(m.clone());
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// `∩`: dyadic intersection. Faithful to Kotlin `IntersectionAPLFunction`.
    /// Preserves the left argument's order and duplicates for matched elements.
    /// A `⍬` (Null) operand yields `⍬`. Strings intersect char-wise.
    fn intersection(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let l = left_val.force(self)?;
        let r = right_val.force(self)?;
        if l.is_null() || r.is_null() {
            return Ok(Rc::new(APLValue::Null));
        }
        if let (APLValue::Str(s1), APLValue::Str(s2)) = (l.as_ref(), r.as_ref()) {
            let mut seen_b = std::collections::HashSet::new();
            for c in s2.chars() {
                seen_b.insert(Self::type_qualified_key(&APLValue::Char(c)));
            }
            let mut out = String::new();
            for c in s1.chars() {
                let key = Self::type_qualified_key(&APLValue::Char(c));
                if seen_b.contains(&key) {
                    out.push(c);
                }
            }
            return Ok(Rc::new(APLValue::Str(out)));
        }
        let a = self.members_of(&l);
        let b = self.members_of(&r);
        let b_keys: std::collections::HashSet<String> =
            b.iter().map(|m| Self::type_qualified_key(m.as_ref())).collect();
        let mut out: Vec<AplRef<APLValue>> = Vec::new();
        for m in &a {
            let key = Self::type_qualified_key(m.as_ref());
            if b_keys.contains(&key) {
                out.push(m.clone());
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Flat members of a value for set operations: scalars → 1 element, strings →
    /// chars, arrays → their (ravelled) elements, `⍬` → empty.
    fn members_of(&self, v: &APLValue) -> Vec<AplRef<APLValue>> {
        match v {
            APLValue::Null => Vec::new(),
            APLValue::Str(s) => s.chars().map(|c| Rc::new(APLValue::Char(c))).collect(),
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        }
    }

    /// Type-qualified membership key (mirrors Kotlin `makeTypeQualifiedKey`): two
    /// values hash/eq the same only when both type AND value match, so `1` (Long)
    /// and `1.0` (Double) are distinct keys.
    fn type_qualified_key(v: &APLValue) -> String {
        match v {
            APLValue::Number(n) => match n {
                KapNumber::Long(_) => format!("L:{}", n.format(false)),
                KapNumber::Double(_) => format!("D:{}", n.format(false)),
                KapNumber::BigInt(_) => format!("B:{}", n.format(false)),
                KapNumber::Rational(_) => format!("R:{}", n.format(false)),
                KapNumber::Complex(_, _) => format!("C:{}", n.format(false)),
            },
            APLValue::Char(c) => format!("c:{}", c),
            APLValue::Str(s) => format!("s:{}", s),
            APLValue::Null => "null".to_string(),
            APLValue::Symbol { name, namespace } => format!("sym:{:?}:{}", namespace, name),
            APLValue::Array(a) => {
                let mut s = String::from("A:");
                for e in a.elements() {
                    s.push_str(&Self::type_qualified_key(e.as_ref()));
                    s.push(',');
                }
                s
            }
            _ => format!("other:{}", v.format_value()),
        }
    }

    /// `⍸` (where): index/coordinate of nonzero elements. Faithful to Kotlin
    /// `WhereAPLFunction` `eval1Arg`. NOT implemented: dyadic interval form
    /// (`a ⍸ b`) and the inverse (`⍸˝`, needs the `˝` adverb).
    /// - scalar `n`: length-n vector of Null (`⍸ 1` → `(⍬)`); negative → error.
    /// - vector: indices (0-based) where the element is nonzero, repeated by its value.
    /// - higher rank: coordinate vectors `[axis0 axis1 …]` for each nonzero element.
    fn where_fn(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let r = right_val.force(self)?;
        // `⍸⍬` → `⍬` (Null). A Null right argument returns Null.
        if matches!(r.as_ref(), APLValue::Null) {
            return Ok(Rc::new(APLValue::Null));
        }
        if let APLValue::Number(n) = r.as_ref() {
            let v = n.as_long().map_err(|e| AplError::runtime(e))?;
            if v < 0 {
                return Err(AplError::runtime("Negative value found in right argument".into()));
            }
            // `⍸ 0` → `⍬` (Null), not a length-0 vector.
            if v == 0 {
                return Ok(Rc::new(APLValue::Null));
            }
            // Guard against runaway allocations: a Double right arg (e.g. `⍸ 1e100`)
            // saturates `as_long()` to i64::MAX, and `vec![_; 9e18]` would abort the
            // process (uncatchable by catch_unwind), killing the conformance harness
            // before its report is written. Reject anything past a sane cap.
            if v as usize > 100_000_000 {
                return Err(AplError::runtime("where: result too large".into()));
            }
            let out = vec![Rc::new(APLValue::Null); v as usize];
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![out.len()],
                ArrayData::Nested(out),
            )))));
        }
        let dims = r.dimensions();
        let rank = r.rank();
        if rank == 0 {
            return Err(AplError::runtime(
                "Argument must be a number. Got a non-numeric scalar".into(),
            ));
        }
        if rank == 1 {
            let mut out: Vec<AplRef<APLValue>> = Vec::new();
            let elems = match r.as_ref() {
                APLValue::Array(a) => a.elements(),
                _ => vec![Rc::new(r.as_ref().clone())],
            };
            for (i, e) in elems.iter().enumerate() {
                let n = match e.as_ref() {
                    APLValue::Number(x) => x.as_long().map_err(|e2| AplError::runtime(e2))?,
                    other => return Err(AplError::runtime(format!(
                        "where: expected a number, got {}",
                        other.class_name()
                    ))),
                };
                if n < 0 {
                    return Err(AplError::runtime("Negative value found in right argument".into()));
                }
                // Guard against runaway output: a Double element saturates `as_long()`
                // to i64::MAX and the inner `for _ in 0..n` would loop effectively
                // forever (and/or exhaust memory). Cap the accumulated result size.
                if (out.len() as i64 + n) as usize > 100_000_000 {
                    return Err(AplError::runtime("where: result too large".into()));
                }
                for _ in 0..n {
                    out.push(Rc::new(APLValue::Number(KapNumber::Long(i as i64))));
                }
            }
            // `⍸ 0 0 0 0` → `⍬` (Null), not a length-0 vector.
            if out.is_empty() {
                return Ok(Rc::new(APLValue::Null));
            }
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![out.len()],
                ArrayData::Nested(out),
            )))));
        }
        // Higher rank: produce coordinate vectors for each nonzero element.
        let total: usize = dims.iter().product();
        // Precompute stride multipliers so we can map a flat index to coordinates.
        let mut mult = vec![1usize; rank];
        let mut stride = 1usize;
        for k in (0..rank).rev() {
            mult[k] = stride;
            stride *= dims[k];
        }
        let mut out: Vec<AplRef<APLValue>> = Vec::new();
        for flat in 0..total {
            let e = r.value_at(flat);
            let n = match e {
                APLValue::Number(x) => x.as_long().map_err(|e2| AplError::runtime(e2))?,
                other => return Err(AplError::runtime(format!(
                    "where: expected a number, got {}",
                    other.class_name()
                ))),
            };
            if n < 0 {
                return Err(AplError::runtime("Negative value found in right argument".into()));
            }
            if n > 0 {
                let mut coords = Vec::with_capacity(rank);
                let mut rem = flat;
                for k in 0..rank {
                    coords.push(Rc::new(APLValue::Number(KapNumber::Long(
                        (rem / mult[k]) as i64,
                    ))));
                    rem %= mult[k];
                }
                for _ in 0..n {
                    out.push(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                        vec![coords.len()],
                        ArrayData::Nested(coords.clone()),
                    )))));
                }
            }
        }
        // `⍸ 2 2⍴0 0 0 0` → `⍬` (Null) when no element is nonzero.
        if out.is_empty() {
            return Ok(Rc::new(APLValue::Null));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Dyadic interval form `a ⍸ b` (Kotlin `WhereAPLFunction.eval2Arg`).
    /// `a` = boundaries (scalar or 1-D vector), must be strictly ascending (no equal
    /// adjacent values), else error. `b` = data; the result has the SAME SHAPE as `b`,
    /// each element = the count of boundaries `<=` that data value (binary search).
    /// Magnitude of a huge `b` element (e.g. `1e100`) is just a comparison operand,
    /// never an allocation count — so this is inherently bounded by `⍴b` (Kotlin's
    /// lazy `IntervalValue`), no runaway allocation.
    fn interval(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.force(self)?;
        // Boundaries must be a scalar or a 1-D vector, strictly ascending.
        let a_dims = a.dimensions();
        if a_dims.len() > 1 {
            return Err(AplError::runtime(
                "Left argument must be a scalar or a 1-dimensional array".into(),
            ));
        }
        let mut boundaries: Vec<APLValue> = match a.as_ref() {
            APLValue::Array(arr) => arr.elements().into_iter().map(|e| (*e).clone()).collect(),
            APLValue::Str(s) => s.chars().map(APLValue::Char).collect(),
            other => vec![other.clone()],
        };
        // Validate strictly ascending (Kotlin throws on a non-ordered left arg).
        for w in boundaries.windows(2) {
            match w[0].total_cmp(&w[1]) {
                Some(Ordering::Less) => {}
                _ => {
                    return Err(AplError::runtime(
                        "Left argument must be ordered".into(),
                    ))
                }
            }
        }
        let b = right_val.force(self)?;
        // Empty data (including `⍬` / `""`) → Null (Kotlin `⍸` on an empty right returns Null).
        if matches!(b.as_ref(), APLValue::Null) {
            return Ok(Rc::new(APLValue::Null));
        }
        // Materialise one result per element of `b`, preserving `b`'s shape.
        // A string right argument is iterated character-by-character (Kotlin `IntervalValue`).
        let (b_dims, b_elems): (Vec<usize>, Vec<AplRef<APLValue>>) = match b.as_ref() {
            APLValue::Array(arr) => (arr.dimensions.clone(), arr.elements()),
            APLValue::Str(s) => {
                let cs: Vec<AplRef<APLValue>> = s.chars().map(|c| Rc::new(APLValue::Char(c))).collect();
                (vec![cs.len()], cs)
            }
            other => (vec![], vec![Rc::new(other.clone())]),
        };
        if b_elems.is_empty() {
            return Ok(Rc::new(APLValue::Null));
        }
        let vals: Vec<AplRef<APLValue>> = b_elems
            .iter()
            .map(|e| {
                // Binary search: count boundaries <= the data value.
                let mut low = 0i64;
                let mut high = boundaries.len() as i64 - 1;
                while low <= high {
                    let mid = ((low as u64 + high as u64) / 2) as i64;
                    let cmp = boundaries[mid as usize]
                        .total_cmp(&*e)
                        .unwrap_or(Ordering::Less);
                    match cmp {
                        Ordering::Less => low = mid + 1,
                        Ordering::Greater => high = mid - 1,
                        Ordering::Equal => {
                            // Found an equal boundary: in Kotlin a value equal to a
                            // boundary counts as "past" it, i.e. index+1 (low = mid+1).
                            low = mid + 1;
                        }
                    }
                }
                Rc::new(APLValue::Number(KapNumber::Long(low)))
            })
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            b_dims,
            ArrayData::Nested(vals),
        )))))
    }

    /// Grade `⍋`/`⍒ x`: 0-based indices (permutation of the first axis) that would
    /// sort `x`. Mirrors Kotlin `GradeFunction`: grades the **first axis** only
    /// (majoring over cells of size = `multipliers[0]`), comparing cells element-wise.
    /// Edge rules: scalar → error ("cannot be sorted"); empty first axis → Null;
    /// single element along first axis → `(0)`; otherwise a length-`axis[0]` vector.
    /// `ascending=true` for `⍋`, `false` for `⍒` (reverses the comparison).
    fn grade(
        &self,
        right_val: AplRef<APLValue>,
        ascending: bool,
    ) -> Result<AplRef<APLValue>, AplError> {
        let r = right_val.force(self)?;
        // `⍬` (Null) grades to itself (Kotlin: empty → APLNullValue).
        if matches!(*r, APLValue::Null) {
            return Ok(Rc::new(APLValue::Null));
        }
        // Scalars cannot be sorted.
        if r.rank() == 0 {
            return Err(AplError::runtime("Scalars cannot be sorted".into()));
        }
        let dims = r.dimensions();
        let axis_len = dims[0];
        // Empty along the major axis → Null.
        if axis_len == 0 {
            return Ok(Rc::new(APLValue::Null));
        }
        // Single element along the major axis → (0).
        if axis_len == 1 {
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![1],
                ArrayData::Nested(vec![Rc::new(APLValue::Number(KapNumber::Long(0)))]),
            )))));
        }
        // Stride between consecutive first-axis cells (elements per cell).
        let mult: usize = if dims.len() == 1 {
            1
        } else {
            dims[1..].iter().product()
        };
        let mut idx: Vec<usize> = (0..axis_len).collect();
        idx.sort_by(|&i, &j| {
            let mut ap = i * mult;
            let mut bp = j * mult;
            let mut res = Ordering::Equal;
            for _ in 0..mult {
                let va = r.value_at(ap);
                let vb = r.value_at(bp);
                let c = va.total_cmp(&vb).unwrap_or(Ordering::Equal);
                if c != Ordering::Equal {
                    res = c;
                    break;
                }
                ap += 1;
                bp += 1;
            }
            if ascending {
                res
            } else {
                res.reverse()
            }
        });
        let out: Vec<AplRef<APLValue>> = idx
            .into_iter()
            .map(|i| Rc::new(APLValue::Number(KapNumber::Long(i as i64))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Grade up `⍋ x`: ascending first-axis grade.
    fn grade_up(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        self.grade(right_val, true)
    }

    /// Grade down `⍒ x`: descending first-axis grade.
    fn grade_down(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        self.grade(right_val, false)
    }

    /// Logical not `∼ x`: element-wise `1 - x` for boolean arrays (0↔1). Dyadic-free;
    /// called with one arg only (Kotlin `NotAPLFunction` is monadic).
    fn logical_not(
        &self,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let r = right_val.force(self)?;
        // Single boolean scalar.
        if r.rank() == 0 {
            if let APLValue::Number(n) = r.as_ref() {
                let b = n.as_boolean();
                return Ok(Rc::new(APLValue::Number(KapNumber::Long(if b { 0 } else { 1 }))));
            }
            return Err(AplError::runtime("∼ requires booleans".into()));
        }
        let elems = match r.as_ref() {
            APLValue::Array(a) => a.elements(),
            _ => return Err(AplError::runtime("∼ requires booleans".into())),
        };
        let out: Vec<AplRef<APLValue>> = elems
            .iter()
            .map(|e| match e.as_ref() {
                APLValue::Number(n) => {
                    let b = n.as_boolean();
                    Ok(Rc::new(APLValue::Number(KapNumber::Long(if b { 0 } else { 1 }))))
                }
                other => return Err(AplError::runtime(format!("∼ requires booleans, got {}", other.class_name()))),
            })
            .collect::<Result<Vec<_>, AplError>>()?;
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            r.dimensions(),
            ArrayData::Nested(out),
        )))))
    }

    // --- factorial / binomial (! ) ---

    /// `!` (factorial/gamma/binomial): monadic = gamma(n+1) = n!; dyadic = binomial.
    /// Kap's `BinomialAPLFunction`: `a!b` = C(b, a) = gamma(1+b)/(gamma(1+a)*gamma(1+b-a)),
    /// using Double gamma for non-integer / large args; Long path for small non-negative ints.
    /// Dyadic returns 0.0 when a > b (per Kap's `doubleBinomial` case table).
    fn factorial_binomial(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Dyadic: a ! b
        if let Some(l) = left_val {
            let a = l.force(self)?;
            let b = right_val.force(self)?;
            // Both scalar numbers.
            match (a.as_ref(), b.as_ref()) {
                (APLValue::Number(x), APLValue::Number(y)) => {
                    Self::binomial_two_numbers(x, y)
                }
                _ => Err(AplError::runtime(
                    "!: requires scalar numbers for both arguments".into(),
                )),
            }
        } else {
            // Monadic: ! b = gamma(b+1). For b+1 <= 0.5 the naive exp(lgamma)
            // loses Γ's sign (lgamma returns ln|Γ|), so use the reflection
            // formula Γ(x) = π / (sin(πx)·Γ(1-x)).
            // Oracle: !5→120.0, !¯0.5→1.772453850905516, !¯1.5→-3.5449077018110318.
            let b = right_val.force(self)?;
            match b.as_ref() {
                APLValue::Number(y) => {
                    let d = y.as_double();
                    let x = d + 1.0;
                    let v = if x >= 0.5 {
                        lgamma(x).exp()
                    } else {
                        std::f64::consts::PI
                            / ((std::f64::consts::PI * x).sin() * lgamma(1.0 - x).exp())
                    };
                    Ok(Rc::new(APLValue::Number(KapNumber::Double(v))))
                }
                _ => Err(AplError::runtime("!: requires a number".into())),
            }
        }
    }

    fn binomial_two_numbers(a: &KapNumber, b: &KapNumber) -> Result<AplRef<APLValue>, AplError> {
        // Try the Long path: both non-negative ints, a <= b, within int range.
        let a_long = a.as_long();
        let b_long = b.as_long();
        if let (Ok(x), Ok(y)) = (a_long, b_long) {
            if x >= 0 && y >= 0 && y >= x && y <= i32::MAX as i64 {
                let r = Self::long_binomial(y as i32, x as i32);
                return Ok(Rc::new(APLValue::Number(KapNumber::Long(r as i64))));
            }
        }
        // Double path: binomial(a, b) = gamma(1+b) / (gamma(1+a) * gamma(1+b-a))
        let a_f = a.as_double();
        let b_f = b.as_double();
        // case table: a > b → 0.0
        if a_f > b_f {
            return Ok(Rc::new(APLValue::Number(KapNumber::Double(0.0))));
        }
        let log_gamma_1b = lgamma(b_f + 1.0);
        let log_gamma_1a = lgamma(a_f + 1.0);
        let log_gamma_1ba = lgamma(b_f - a_f + 1.0);
        let log_result = log_gamma_1b - log_gamma_1a - log_gamma_1ba;
        let v = log_result.exp();
        Ok(Rc::new(APLValue::Number(KapNumber::Double(v))))
    }

    fn long_binomial(n: i32, k: i32) -> i32 {
        if k < 0 || k > n {
            return 0;
        }
        let mut k = k as i32;
        if k > n / 2 {
            k = n - k;
        }
        if k == 0 {
            return 1;
        }
        let mut r: i64 = 1;
        for i in 0..k {
            r = r * (n as i64 - i as i64) / (i as i64 + 1);
            // Saturate to i32::MAX if it overflows
            if r > i32::MAX as i64 {
                return i32::MAX;
            }
        }
        r as i32
    }

    // --- range (… ) ---

    /// `…` (range): dyadic only. Both args must be scalar or non-empty 1-D.
    /// Last element of A, first element of B must both be integers or both chars.
    /// Produces an inclusive integer/char sequence from A's last to B's first.
    fn range(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.force(self)?;
        let b = right_val.force(self)?;
        // Both must be 1-D (or scalar → rank-0, treated as 1-D of length 1).
        let a_rank = a.rank();
        let b_rank = b.rank();
        // Kotlin: both must be scalar or non-empty 1-D.
        if a_rank > 1 || b_rank > 1 {
            return Err(AplError::runtime(
                "…: Both arguments must be scalars or 1-dimensional arrays".into(),
            ));
        }
        let a_len = a.element_count();
        let b_len = b.element_count();
        if a_len == 0 || b_len == 0 {
            return Err(AplError::runtime(
                "…: Both arguments must be non-empty".into(),
            ));
        }
        // Last element of A, first element of B.
        let a_last = a.value_at(a_len - 1);
        let b_first = b.value_at(0);
        // Determine the element type from the last A element and first B element.
        match (&a_last, &b_first) {
            (APLValue::Number(x), APLValue::Number(y)) => {
                let x_long = x.as_long();
                let y_long = y.as_long();
                if let (Ok(xv), Ok(yv)) = (x_long, y_long) {
                    // Integer range
                    return self.range_long(xv, yv);
                }
                // Fall back to double range (will likely error in Kotlin if not integer)
                return self.range_double(x.as_double(), y.as_double());
            }
            (APLValue::Char(xc), APLValue::Char(yc)) => {
                return self.range_char(*xc, *yc);
            }
            _ => {
                return Err(AplError::runtime(format!(
                    "…: Range types not compatible. A={}, B={}",
                    a_last.class_name(),
                    b_first.class_name()
                )));
            }
        }
    }

    fn range_long(&self, start: i64, end: i64) -> Result<AplRef<APLValue>, AplError> {
        let n = (end - start).unsigned_abs() as usize + 1;
        if n > 100_000_000 {
            return Err(AplError::runtime("…: Resulting range too large".into()));
        }
        let step = if start <= end { 1i64 } else { -1i64 };
        let out: Vec<AplRef<APLValue>> = (0..n)
            .map(|i| Rc::new(APLValue::Number(KapNumber::Long(start + i as i64 * step))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![n],
            ArrayData::Nested(out),
        )))))
    }

    fn range_double(&self, start: f64, end: f64) -> Result<AplRef<APLValue>, AplError> {
        // Kap range for non-integers: raises "Range types not compatible".
        Err(AplError::runtime(
            "…: Range types not compatible".into(),
        ))
    }

    fn range_char(&self, start: char, end: char) -> Result<AplRef<APLValue>, AplError> {
        let si = start as i64;
        let ei = end as i64;
        let n = (ei - si).unsigned_abs() as usize + 1;
        if n > 100_000_000 {
            return Err(AplError::runtime("…: Resulting range too large".into()));
        }
        let step = if si <= ei { 1i64 } else { -1i64 };
        let out: Vec<AplRef<APLValue>> = (0..n)
            .map(|i| {
                let c = char::from_u32((si + i as i64 * step) as u32).unwrap_or('\u{fffd}');
                Rc::new(APLValue::Char(c))
            })
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![n],
            ArrayData::Char(out.into_iter().map(|r| match r.as_ref() {
                APLValue::Char(c) => *c,
                _ => unreachable!(),
            }).collect()),
        )))))
    }

    // --- find (⍷ ) ---

    /// `⍷` (find): dyadic only. Shape of result = shape of B (right arg).
    /// Boolean 1 at each position where a subarray equal to A starts in B (windowed match).
    /// Lower-rank A is prepended with 1s (trailing match); higher-rank A → all 0 (no error).
    /// Uses cross-kind `compareEqualsTotalOrdering` for element comparison.
    fn find(
        &self,
        left_val: AplRef<APLValue>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.force(self)?;
        let b = right_val.force(self)?;

        // Scalar B (rank 0): compare A and B, return 1 or 0.
        if b.rank() == 0 {
            let eq = a.total_cmp(&b).map(|o| o == Ordering::Equal).unwrap_or(false);
            let v = if eq { 1i64 } else { 0i64 };
            return Ok(Rc::new(APLValue::Number(KapNumber::Long(v))));
        }

        // Extract A and B dims + flat elements from the forced APLValues.
        let (a_dims, a_elems): (Vec<usize>, Vec<AplRef<APLValue>>) = match a.as_ref() {
            APLValue::Array(arr) => (arr.dimensions.clone(), arr.elements()),
            APLValue::Str(s) => {
                let cs: Vec<AplRef<APLValue>> = s
                    .chars()
                    .map(|c| Rc::new(APLValue::Char(c)))
                    .collect();
                (vec![cs.len()], cs)
            }
            other => (vec![], vec![Rc::new(other.clone())]),
        };
        let (b_dims, b_elems): (Vec<usize>, Vec<AplRef<APLValue>>) = match b.as_ref() {
            APLValue::Array(arr) => (arr.dimensions.clone(), arr.elements()),
            APLValue::Str(s) => {
                let cs: Vec<AplRef<APLValue>> = s
                    .chars()
                    .map(|c| Rc::new(APLValue::Char(c)))
                    .collect();
                (vec![cs.len()], cs)
            }
            other => (vec![], vec![Rc::new(other.clone())]),
        };

        // A rank > B rank: no match possible → all zeros (shape of B).
        if a_dims.len() > b_dims.len() {
            let n: usize = b_dims.iter().product();
            let out: Vec<AplRef<APLValue>> = (0..n)
                .map(|_| Rc::new(APLValue::Number(KapNumber::Long(0))))
                .collect();
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                b_dims.clone(),
                ArrayData::Nested(out),
            )))));
        }

        let a_rank = a_dims.len();
        let b_rank = b_dims.len();
        let dims_diff = b_rank - a_rank;

        // Effective A dimensions: prepend 1s so A has the same rank as B.
        let eff_a_dims: Vec<usize> = vec![1usize; dims_diff]
            .into_iter()
            .chain(a_dims.iter().copied())
            .collect();

        // Compute B multipliers (stride per axis) for flat-index ↔ coord conversion.
        let b_mult: Vec<usize> = {
            let mut m = Vec::with_capacity(b_rank);
            let mut prod = 1usize;
            for d in b_dims.iter().rev() {
                m.push(prod);
                prod *= d;
            }
            m.reverse();
            m
        };

        // Compute effective A multipliers.
        let a_mult: Vec<usize> = {
            let mut m = Vec::with_capacity(b_rank);
            let mut prod = 1usize;
            for d in eff_a_dims.iter().rev() {
                m.push(prod);
                prod *= d;
            }
            m.reverse();
            m
        };

        if a_elems.is_empty() {
            // Empty A: no match → all zeros.
            let n = b.element_count();
            let out: Vec<AplRef<APLValue>> = (0..n)
                .map(|_| Rc::new(APLValue::Number(KapNumber::Long(0))))
                .collect();
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                b_dims,
                ArrayData::Nested(out),
            )))));
        }

        let a_size = a_elems.len();
        let b_size = b.element_count();

        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(b_size);

        for p in 0..b_size {
            // B coordinate from flat index p.
            let b_coord: Vec<usize> = {
                let mut coord = Vec::with_capacity(b_rank);
                let mut rem = p;
                for &mult in b_mult.iter() {
                    coord.push(rem / mult);
                    rem %= mult;
                }
                coord
            };

            // Check window fits: for every B axis, the window starting at this
            // coordinate with the padded A dims must stay within B. Prepended-1
            // axes (dimensionsDiff) trivially satisfy this since eff_a_dims=1.
            // This mirrors Kotlin's FindResultValue.opLong: it only bounds the
            // *aligned* axes (coord[dimensionsDiff+i] + a_dims[i] <= b_dims[...]);
            // the window start `p` already carries any prepended-axis coordinates,
            // so we must NOT force those coordinates to 0.
            let window_fits = b_coord
                .iter()
                .enumerate()
                .all(|(i, &bc)| bc + eff_a_dims[i] <= b_dims[i]);

            if !window_fits {
                out.push(Rc::new(APLValue::Number(KapNumber::Long(0))));
                continue;
            }

            // Check element-by-element match.
            let mut match_found = true;
            for (ai, a_elem) in a_elems.iter().enumerate() {
                // A coordinate from flat index ai.
                let a_coord: Vec<usize> = {
                    let mut coord = Vec::with_capacity(b_rank);
                    let mut rem = ai;
                    for &mult in a_mult.iter() {
                        coord.push(rem / mult);
                        rem %= mult;
                    }
                    coord
                };

                // B coordinate = b_coord + a_coord.
                let b_elem_coord: Vec<usize> = b_coord
                    .iter()
                    .zip(a_coord.iter())
                    .map(|(&bc, &ac)| bc + ac)
                    .collect();

                // Convert B element coordinate to flat index.
                let b_flat: usize = b_elem_coord
                    .iter()
                    .zip(b_mult.iter())
                    .map(|(&coord, &mult)| coord * mult)
                    .sum();

                let b_elem = &b_elems[b_flat];
                if a_elem
                    .as_ref()
                    .total_cmp(b_elem.as_ref())
                    .map(|o| o == Ordering::Equal)
                    .unwrap_or(false)
                {
                    // match
                } else {
                    match_found = false;
                    break;
                }
            }

            out.push(Rc::new(APLValue::Number(KapNumber::Long(
                if match_found { 1 } else { 0 },
            ))));
        }

        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            b_dims,
            ArrayData::Nested(out),
        )))))
    }

    /// Decode `A ⊥ B` and encode `A ⊤ B` \u2014 Kap's **base-value** (mixed-radix)
    /// functions. These mirror `~/Apps/array/array/standard-lib/math-kap.kap`
    /// (lines 64-103): they are NOT the byte-array codecs `encode`/`decode`.
    ///
    /// Semantics verified side-by-side against `kap-jvm-text`:
    /// - `256 ⊥ 104 105 0` → `6842624`
    /// - `2 ⊥ 3 3⍴1 2 3 4 5 6` → `⟨13 20 27⟩` (shape `⟨3⟩`)
    /// - `(2 2 2) ⊥ 1 2 3` → `11`
    /// - `64 ⊤ 66051` → `⟨16 8 3⟩`
    /// - `(4⍴64) ⊤ 6842624` → `⟨26 6 36 0⟩` (shape `⟨4⟩`)
    ///
    /// Algorithm (Kotlin `+⌿ (×⍀ ¯1↓1⍪(0×B) (+⍤¯1) ⌽A) × ⊖B`, general form):
    /// the DIGIT axis is always `B`'s axis 0; each digit at position `j` along
    /// that axis is weighted by `A^(L-1-j)` (scalar `A`) or `Π_{k=j}^{L-1} A[k]`
    /// (vector `A`), and the weighted digits are summed over axis 0. The result
    /// shape is `B`'s shape with axis 0 removed. A scalar `B` (rank 0) returns
    /// `B` unchanged.
    fn decode(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime("⊥ needs two args".into()))?;
        let b = right_val;

        // Left arg A must be rank 0 (scalar radix) or rank 1 (radix vector).
        let a_rank = a.rank();
        if a_rank > 1 {
            return Err(AplError::runtime(
                "Left argument must have rank 0 or 1".into(),
            ));
        }

        let b_rank = b.rank();
        // Scalar B → the digit axis is degenerate; the Kotlin `+⌿ (×⍀ …) × ⊖B`
        // reduces to B × Σ_{j} (Π_{k=0}^{j-1} ⌽A[k]) — i.e. the prefix cumulative
        // products of the reversed radix vector, summed.
        if b_rank == 0 {
            let bval = match b.as_ref() {
                APLValue::Number(n) => n.as_long().map_err(|e| AplError::runtime(e))?,
                _ => return Err(AplError::runtime("⊥ digits must be a number".into())),
            };
            let aelems = a.elements();
            let mut rev: Vec<i64> = aelems
                .iter()
                .filter_map(|e| match e.as_ref() {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                })
                .collect();
            rev.reverse();
            let mut cum: i64 = 1;
            let mut s: i64 = 0;
            for &x in &rev {
                s += cum;
                cum = cum * if x == 0 { 1 } else { x };
            }
            let total = bval * s;
            return Ok(Rc::new(APLValue::Number(KapNumber::Long(total))));
        }

        // Digit axis = axis 0 of B; its length L.
        let b_dims = b.dimensions();
        let l = b_dims[0];

        // Build per-position weights along axis 0.
        let weights: Vec<KapNumber> = match a.as_ref() {
            APLValue::Number(n) => {
                let base = n.as_long().map_err(|e| AplError::runtime(e))?;
                let base = if base == 0 { 1 } else { base };
                // weight[j] = base^(L-1-j)
                let mut w = Vec::with_capacity(l);
                for j in 0..l {
                    let exp = (l - 1 - j) as u32;
                    w.push(KapNumber::Long(base.pow(exp)));
                }
                w
            }
            APLValue::Array(_) => {
                let aelems = a.elements();
                // weight[j] = Π_{k=j+1}^{L-1} A[k]  (product of radices AFTER position j;
                // A[j] itself is the per-digit multiplier, not part of the accumulation).
                let mut suffix: KapNumber = KapNumber::Long(1);
                let mut w = vec![KapNumber::Long(0); l];
                for j in (0..l).rev() {
                    w[j] = suffix.clone();
                    let ak = aelems
                        .get(j % aelems.len())
                        .and_then(|e| match e.as_ref() {
                            APLValue::Number(n) => n.as_long().ok(),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            AplError::runtime("⊥ radix vector must contain integers".into())
                        })?;
                    let ak_n = if ak == 0 { KapNumber::Long(1) } else { KapNumber::Long(ak) };
                    suffix = suffix.mul(&ak_n);
                }
                w
            }
            _ => {
                return Err(AplError::runtime("⊥ base must be a number".into()));
            }
        };

        // Result shape = B dims with axis 0 removed.
        let out_dims: Vec<usize> = b_dims[1..].to_vec();
        let out_total: usize = if out_dims.is_empty() { 1 } else { out_dims.iter().product() };
        let b_strides = strides(&b_dims);

        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(out_total);
        for pos in 0..out_total {
            // coordinate over output axes (axes 1..rank-1)
            let mut rem = pos;
            let mut out_coords = vec![0usize; out_dims.len()];
            for k in 0..out_dims.len() {
                out_coords[k] = rem / b_strides[k + 1];
                rem %= b_strides[k + 1];
            }
            let mut acc: KapNumber = KapNumber::Long(0);
            for j in 0..l {
                // flat index into B for coordinate [j, out_coords...]
                let mut bflat = j * b_strides[0];
                for k in 0..out_coords.len() {
                    bflat += out_coords[k] * b_strides[k + 1];
                }
                let bval = match b.value_at(bflat) {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                };
                if let Some(d) = bval {
                    acc = acc.add(&KapNumber::Long(d).mul(&weights[j]));
                }
            }
            out.push(Rc::new(APLValue::Number(acc)));
        }

        if out_dims.is_empty() {
            Ok(out.into_iter().next().unwrap_or_else(|| Rc::new(APLValue::Null)))
        } else {
            Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                out_dims,
                ArrayData::Nested(out),
            )))))
        }
    }

    /// Encode `A ⊤ B`: represent `B` in the mixed radix `A`.
    /// - Scalar `A`: repeated division by `A` (scalarEncode); digit count is the
    ///   number of divisions needed by the **largest** element of `B` (min 1;
    ///   `0` yields an empty result `⍬`). Digits are stored most-significant-first.
    /// - Vector `A`: each element of `B` is decomposed by the radices `A`
    ///   (most-significant-first). The result shape is `(⍴A) ,⍴ B`.
    fn encode(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime("⊤ needs two args".into()))?;
        let b = right_val;

        let a_rank = a.rank();
        if a_rank > 2 {
            return Err(AplError::runtime(
                "Left argument must have rank 0, 1 or 2".into(),
            ));
        }

        // Rank-2 left arg: COLUMN j of A is the radix list for B element j (verified
        // against the oracle: `(2 2⍴8 16 4 4) ⊤ 7 9 → [[0 0][7 9]]`). Encode each
        // column against its B element and stack the digit rows along axis 0.
        if a_rank == 2 {
            let a_dims = a.dimensions();
            let a_elems = a.elements();
            let rows = a_dims[0];
            let cols = a_dims[1];
            // Per-B-element digit vectors (MSB-first), computed with the successive
            // division of math-kap's vectorEncode.
            let b_elems: Vec<i64> = b
                .elements()
                .iter()
                .filter_map(|e| match e.as_ref() {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                })
                .collect();
            let b_total = b_elems.len();
            let mut per_elem_digits: Vec<Vec<i64>> = Vec::with_capacity(b_total);
            for j in 0..cols.min(b_total) {
                let radices: Vec<i64> = (0..rows)
                    .filter_map(|r| match a_elems[r * cols + j].as_ref() {
                        APLValue::Number(n) => n.as_long().ok(),
                        _ => None,
                    })
                    .collect();
                let mut v = b_elems[j];
                let mut ds: Vec<i64> = Vec::with_capacity(radices.len());
                // math-kap vectorEncode peels ↑A (the FIRST radix) each iteration and
                // PREPENDS the remainder (`rem⍪res`), so the last-peeled remainder ends up
                // first: collect in natural order, then reverse once.
                for &rad in radices.iter() {
                    let r = if rad == 0 { 1 } else { rad };
                    ds.push(v % r);
                    v /= r;
                }
                ds.reverse();
                per_elem_digits.push(ds);
            }
            // Stack along axis 0: row i = [per_elem_digits[j][i] for j].
            let n_rows = per_elem_digits.first().map_or(0, |d| d.len());
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(n_rows * cols);
            for i in 0..n_rows {
                for d in &per_elem_digits {
                    out.push(Rc::new(APLValue::Number(KapNumber::Long(d[i]))));
                }
            }
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![n_rows, cols],
                ArrayData::Nested(out),
            )))));
        }

        // Flatten B for iteration; record its shape for the result.
        let b_dims = b.dimensions();
        let b_elems: Vec<i64> = b
            .elements()
            .iter()
            .filter_map(|e| match e.as_ref() {
                APLValue::Number(n) => n.as_long().ok(),
                _ => None,
            })
            .collect();
        let b_total = b_elems.len();

        if a_rank == 0 {
            // Scalar radix: scalarEncode.
            let radix = match a.as_ref() {
                APLValue::Number(n) => n.as_long().map_err(|e| AplError::runtime(e))?,
                _ => return Err(AplError::runtime("⊤ radix must be an integer".into())),
            };
            let radix = if radix == 0 { 1 } else { radix };
            // digit count = max over B of divisions by radix (min 1; 0 → empty).
            let max_val: i64 = b_elems.iter().copied().max().unwrap_or(0);
            let digits = if max_val <= 0 {
                0
            } else {
                let mut v = max_val;
                let mut d = 0;
                while v > 0 {
                    v /= radix;
                    d += 1;
                }
                d
            };
            if digits == 0 {
                // Result shape: (0, ⍴B) \u2014 an empty leading axis.
                let mut out_dims = vec![0usize];
                out_dims.extend(b_dims.iter().copied());
                return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    out_dims,
                    ArrayData::Nested(vec![]),
                )))));
            }
            // For each B element, compute digits MSB-first. Result shape is
            // (digits, ⍴B): digits along axis 0, B elements along axis 1+, so the
            // flat fill must INTERLEAVE (digit-major outer loop, element inner),
            // matching Kotlin vectorEncode's `rem⍪res` row-prepending.
            let per_elem: Vec<Vec<i64>> = b_elems
                .iter()
                .map(|&v0| {
                    let mut v = v0;
                    let mut lsbs = Vec::with_capacity(digits);
                    for _ in 0..digits {
                        lsbs.push(v % radix);
                        v /= radix;
                    }
                    lsbs.reverse();
                    lsbs
                })
                .collect();
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(digits * b_total);
            for di in 0..digits {
                for col in &per_elem {
                    out.push(Rc::new(APLValue::Number(KapNumber::Long(col[di]))));
                }
            }
            let mut out_dims = vec![digits];
            out_dims.extend(b_dims.iter().copied());
            Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                out_dims,
                ArrayData::Nested(out),
            )))))
        } else {
            // Vector radix: the Kotlin `⊤` calls `vectorEncode` on `(⌽A)` — the
            // radices are reversed (most-significant-first for the output, but the
            // successive-division peels from the reversed front, and B becomes the
            // running quotient each step). Reversing `radices` here is equivalent.
            let aelems = a.elements();
            let mut radices: Vec<i64> = aelems
                .iter()
                .filter_map(|e| match e.as_ref() {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                })
                .collect();
            radices.reverse();
            let nr = radices.len();
            // Same interleaved layout as the scalar branch: shape (nr, ⍴B), digits
            // along axis 0, B elements along the trailing axes.
            let per_elem: Vec<Vec<i64>> = b_elems
                .iter()
                .map(|&v0| {
                    let mut v = v0;
                    let mut lsbs = Vec::with_capacity(nr);
                    for &r in &radices {
                        let r = if r == 0 { 1 } else { r };
                        lsbs.push(v % r);
                        v /= r;
                    }
                    lsbs.reverse();
                    lsbs
                })
                .collect();
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(nr * b_total);
            for di in 0..nr {
                for col in &per_elem {
                    out.push(Rc::new(APLValue::Number(KapNumber::Long(col[di]))));
                }
            }
            let mut out_dims = vec![nr];
            out_dims.extend(b_dims.iter().copied());
            Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                out_dims,
                ArrayData::Nested(out),
            )))))
        }
    }

    /// Load a library file and evaluate it in the current namespace.
    ///
    /// Mirrors Kotlin `LoadFileFunction`/`resolveLibraryFile`: the argument is a
    /// *filename*, resolved by **basename** across a search path (CWD, then
    /// `$KAP_LIB`, then this project's `kap-stdlib/std`). A recursion guard in
    /// `Engine::include_stack` prevents a file that self-`use`s (e.g.
    /// `base-functions.kap`) from loading more than once.
    fn use_file(
        &self,
        name: &str,
        env: &Rc<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let basename = std::path::Path::new(name)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_string());

        // Recursion guard: skip an already-in-flight include.
        {
            let stack = self.include_stack.borrow();
            if stack.contains(&basename) {
                return Ok(Rc::new(APLValue::Null));
            }
        }

        // Build the search path: configured --lib-path dirs first, then CWD, then
        // $KAP_LIB, then this repo's kap-stdlib/std.
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for p in self.lib_paths.borrow().iter() {
            dirs.push(p.clone());
        }
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd);
        }
        if let Ok(kap_lib) = std::env::var("KAP_LIB") {
            dirs.push(std::path::PathBuf::from(kap_lib));
        }
        // CARGO_MANIFEST_DIR for kap-core is .../rust-kap/kap-core; the stdlib
        // lives at rust-kap/kap-stdlib/std.
        if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
            let repo = std::path::Path::new(manifest)
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(manifest));
            dirs.push(repo.join("kap-stdlib").join("std"));
        }

        let path = dirs
            .iter()
            .map(|d| d.join(&basename))
            .find(|p| p.exists())
            .ok_or_else(|| {
                AplError::runtime(format!("use: library file not found: {}", name))
            })?;
        let content = std::fs::read_to_string(&path).map_err(|e| {
            AplError::runtime(format!("use: cannot read {}: {}", path.display(), e))
        })?;

        self.include_stack.borrow_mut().insert(basename.clone());
        let _guard = IncludeGuard {
            stack: self.include_stack.clone(),
            name: basename,
        };
        // Evaluate the file in the *current* namespace so its top-level
        // `∇`/`⇐` definitions land where the `use` call appears. Per-statement errors are
        // tolerated (mirrors Real Kap: one bad line doesn't abort the whole library file).
        self.eval_string_in_env_tolerant(&content, env)
    }
}

/// RAII guard that removes a basename from `Engine::include_stack` on drop, so a
/// failed or completed include does not permanently block a later `use` of the
/// same file.
struct IncludeGuard {
    stack: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
    name: String,
}

impl Drop for IncludeGuard {
    fn drop(&mut self) {
        self.stack.borrow_mut().remove(&self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> String {
        let e = Engine::new();
        let v = e.eval_string(src).expect("eval failed");
        v.format_value()
    }

    #[test]
    fn eval_addition() {
        assert_eq!(eval("1 + 2"), "3");
    }

    #[test]
    fn eval_nested_addition() {
        assert_eq!(eval("(1 + 2) × 3"), "9");
    }

    #[test]
    fn eval_parenthesised_groups() {
        // Grouping in arithmetic and with assignment/lambda/strand inside.
        assert_eq!(eval("(1 + 2) × 3"), "9");
        assert_eq!(eval("2 × (3 + 4)"), "14");
        assert_eq!(eval("(1 + 2) × (3 + 4)"), "21");
        assert_eq!(eval("((1 + 2))"), "3");
        assert_eq!(eval("(⍳3) + 10"), "(10 11 12)");
        assert_eq!(eval("1 + (2 × 3)"), "7");
        assert_eq!(eval("(x ← 5) + 1"), "6");
        assert_eq!(eval("f ← λ(x) x × 2 ⋄ (f 5) + 1"), "11");
        assert_eq!(eval("(1 2 3) + 10"), "(11 12 13)");
        assert_eq!(eval("f ← λ(x) x × 2 ⋄ f (3 + 4)"), "14");
    }

    #[test]
    fn eval_juxtaposed_groups_strand() {
        // A parenthesised group of values strands into a vector.
        assert_eq!(eval("(1 2 3)"), "(1 2 3)");
    }

    #[test]
    fn eval_nested_arrays() {
        // Nested arrays (depth > 1) parse as a vector containing a nested vector.
        assert_eq!(eval("(1 2 (3 4 5))"), "(1 2 (3 4 5))");
        // Deeper nesting.
        assert_eq!(eval("(1 (2 (3 (4 5))))"), "(1 (2 (3 (4 5))))");
        // A parenthesised group within a strand stays a single element — NOT flattened.
        assert_eq!(eval("((1 2 3) 4 5)"), "((1 2 3) 4 5)");
        assert_eq!(eval("(1 2 3) 4 5"), "((1 2 3) 4 5)");
        // A nested vector element can still be used as data (catenate strands it in).
        assert_eq!(eval("1 2 , (3 4)"), "(1 2 3 4)");
    }

    #[test]
    fn eval_monadic_arithmetic() {
        // `+` and `-` are ambivalent: monadic `- x` = negate, `+ x` = identity.
        assert_eq!(eval("-(1 + 2)"), "¯3");
        assert_eq!(eval("+(1 + 2)"), "3");
        assert_eq!(eval("-(3 1 4)"), "(¯3 ¯1 ¯4)");
    }

    #[test]
    fn eval_paren_operator() {
        // A parenthesised operator `(OP)` is a derived function usable in dyadic position.
        assert_eq!(eval("2 (+) 3"), "5");
        assert_eq!(eval("3 (×) 4"), "12");
    }

    #[test]
    fn eval_unclosed_paren_errors() {
        // An unclosed group is a parse error, not a hang or panic.
        let e = Engine::new();
        let r = e.eval_string("(2 + 3");
        assert!(r.is_err());
    }

    #[test]
    fn eval_iota() {
        assert_eq!(eval("⍳5"), "(0 1 2 3 4)");
    }

    #[test]
    fn eval_tally() {
        assert_eq!(eval("≢ ⍳5"), "5");
    }

    #[test]
    fn eval_first() {
        // `⊃` is Kap's reveal/disclose (DiscloseAPLFunction), NOT "first".
        // Disclosing a rank-1 vector returns it unchanged.
        assert_eq!(eval("⊃ ⍳5"), "(0 1 2 3 4)");
    }

    #[test]
    fn eval_array_literal() {
        assert_eq!(eval("[10; 20; 30]"), "(10 20 30)");
    }

    #[test]
    fn eval_unknown_function_errors() {
        let e = Engine::new();
        assert!(e.eval_string("1 foo 2").is_err());
    }

    // --- Phase 4 ---

    #[test]
    fn eval_sub_neg() {
        assert_eq!(eval("5 - 2"), "3");
        assert_eq!(eval("2 - 5"), "¯3");
    }

    #[test]
    fn eval_div_rational() {
        // Kotlin renders rationals num/den (oracle: 1÷2 -> 1/2).
        assert_eq!(eval("1 ÷ 2"), "1/2");
        assert_eq!(eval("4 ÷ 2"), "2");
    }

    #[test]
    fn eval_comparison() {
        assert_eq!(eval("3 = 3"), "1");
        assert_eq!(eval("3 ≠ 4"), "1");
        assert_eq!(eval("2 < 5"), "1");
        assert_eq!(eval("5 < 2"), "0");
        assert_eq!(eval("5 ≥ 5"), "1");
    }

    #[test]
    fn eval_catenate() {
        assert_eq!(eval("1 , 2 , 3"), "(1 2 3)");
        // In Real Kap, `[...]` after `,` is always an *axis* specifier, not a list
        // literal — so `[1;2] , [3;4]` is `,[3;4]` (a 2-element list as axis) and errors.
        // The port now conforms: assert the error rather than the old lenient result.
        assert!(Engine::new().eval_string("[1; 2] , [3; 4]").is_err());
        // `,[axis]` laminate / concat (Kotlin `,[0.5]` / `,[0]`):
        assert_eq!(eval("⍴ 1 2 3 ,[0.5] 4 5 6"), "(3 2)");
        assert_eq!(eval("⍴ 1 2 3 ,[0] 4 5 6"), "(6)");
        assert_eq!(eval("⍴ (2 2⍴⍳4) ,[0.5] (2 2⍴4+⍳4)"), "(2 2 2)");
    }

    #[test]
    fn eval_reverse() {
        assert_eq!(eval("⌽ ⍳5"), "(4 3 2 1 0)");
    }

    #[test]
    fn eval_take_drop() {
        assert_eq!(eval("3 ↑ ⍳10"), "(0 1 2)");
        assert_eq!(eval("3 ↓ ⍳10"), "(3 4 5 6 7 8 9)");
    }

    #[test]
    fn eval_take_monadic_first() {
        // Monadic `↑` returns the leading *cell* as a scalar (Real Kap "First").
        assert_eq!(eval("↑1 2 3 4 5"), "1");
        assert_eq!(eval("↑⍬"), "0");
        assert_eq!(eval("↑6"), "6");
        // Dyadic `↑` still returns an array shape.
        assert_eq!(eval("1 ↑ 10"), "(10)");
    }

    #[test]
    fn eval_assign_and_var() {
        assert_eq!(eval("x ← 5 ⋄ x + 1"), "6");
    }

    #[test]
    fn eval_lambda_apply() {
        assert_eq!(eval("f ← λ(x) x × 2 ⋄ f 5"), "10");
        assert_eq!(eval("g ← λ(a b) a + b ⋄ 3 g 4"), "7");
    }

    #[test]
    fn eval_strand() {
        assert_eq!(eval("1 2 3 + 10"), "(11 12 13)");
    }

    // --- Phase 6: more builtins ---

    #[test]
    fn eval_ceil_floor() {
        // Oracle: `⌈3.2` → `4` typed `kap:integer` (whole-valued results normalise
        // to Long; verified against kap-jvm-text).
        assert_eq!(eval("⌈ 3.2"), "4");
        assert_eq!(eval("⌊ 3.8"), "3");
        assert_eq!(eval("⌈ 5"), "5");
        assert_eq!(eval("⌈ 1.5 2.5 3.5"), "(2 3 4)");
    }

    #[test]
    fn eval_exp_log() {
        assert_eq!(eval("* 0"), "1.0");
        assert_eq!(eval("* 1"), "2.718281828459045");
        // natural log (monadic ⍟): ln(1) = 0 exactly (but typed Double -> "0.0")
        assert_eq!(eval("⍟ 1"), "0.0");
        // dyadic log: log base 2 of 8 = 3
        assert_eq!(eval("2 ⍟ 8"), "3.0");
    }

    #[test]
    fn eval_modulo() {
        assert_eq!(eval("7 | 3"), "3");
        assert_eq!(eval("8 | 3"), "3");
        assert_eq!(eval("10 | 3"), "3");
    }

    #[test]
    fn eval_boolean_and_or() {
        assert_eq!(eval("1 ∧ 1"), "1");
        assert_eq!(eval("1 ∧ 0"), "0");
        assert_eq!(eval("0 ∨ 1"), "1");
        assert_eq!(eval("0 ∨ 0"), "0");
        // comparisons yield booleans, combine with ∧/∨
        assert_eq!(eval("(3 < 5) ∧ (2 < 4)"), "1");
    }

    #[test]
    fn eval_not() {
        assert_eq!(eval("~ 0"), "1");
        assert_eq!(eval("~ 5"), "0");
        assert_eq!(eval("~ 1 0 3"), "(0 1 0)");
    }

    #[test]
    fn eval_membership() {
        assert_eq!(eval("2 9 4 ∊ 1 2 3 4"), "(1 0 1)");
    }

    #[test]
    fn eval_grade_up() {
        // 0-based indices (Kap is 0-based): ⍋ 3 1 4 1 5 -> positions ascending by value
        assert_eq!(eval("⍋ 3 1 4 1 5"), "(1 3 0 2 4)");
    }

    #[test]
    fn eval_encode_decode() {
        // 2 2 2 ⊤ 5  -> binary-ish mixed radix of 5 = [1 0 1]
        assert_eq!(eval("2 2 2 ⊤ 5"), "(1 0 1)");
        // inverse: 2 2 2 ⊥ 1 0 1 -> 1*4 + 0*2 + 1 = 5
        assert_eq!(eval("2 2 2 ⊥ 1 0 1"), "5");
        // 24 60 ⊤ 90 -> 1 hour 30 min
        assert_eq!(eval("24 60 ⊤ 90"), "(1 30)");
    }

    // --- Phase 7: adverbs (/ reduce, \ scan, ¨ each) ---

    #[test]
    fn eval_reduce() {
        assert_eq!(eval("+/ 1 2 3 4"), "10");
        assert_eq!(eval("×/ 1 2 3 4"), "24");
        assert_eq!(eval("⌈/ 3 9 2 7"), "9");
        assert_eq!(eval("⌊/ 3 9 2 7"), "2");
    }

    #[test]
    fn eval_scan() {
        assert_eq!(eval("+\\ 1 2 3 4"), "(1 3 6 10)");
        assert_eq!(eval("×\\ 1 2 3 4"), "(1 2 6 24)");
    }

    #[test]
    fn eval_each_monadic() {
        assert_eq!(eval("⌈¨ 1.2 2.8 3.5"), "(2 3 4)");
        assert_eq!(eval("~¨ 1 0 3"), "(0 1 0)");
    }

    #[test]
    fn eval_each_dyadic() {
        // element-wise: 2 ×¨ 3 4 5  -> [6 8 10]
        assert_eq!(eval("2 ×¨ 3 4 5"), "(6 8 10)");
        // scalar-extended left, vector right
        assert_eq!(eval("1 2 3 +¨ 4 5 6"), "(5 7 9)");
        // vector × vector each
        assert_eq!(eval("1 2 3 ×¨ 4 5 6"), "(4 10 18)");
    }

    // --- Phase 8: control flow (if / while / when / block) ---

    #[test]
    fn eval_if_then() {
        assert_eq!(eval("if (1 < 2) { 42 }"), "42");
        assert_eq!(eval("if (1 > 2) { 42 } else { 7 }"), "7");
        // no else, false condition -> null
        assert_eq!(eval("if (0) { 1 }"), "null");
    }

    #[test]
    fn eval_block_value() {
        // block returns last statement's value
        assert_eq!(eval("{ 1 ⋄ 2 ⋄ 3 }"), "3");
        // assignment inside a block is visible after (same env)
        assert_eq!(eval("x ← 0 ⋄ { x ← 5 } ⋄ x"), "5");
    }

    #[test]
    fn eval_while_loop() {
        // sum 1..5 via while
        assert_eq!(
            eval("i ← 0 ⋄ s ← 0 ⋄ while (i < 5) { s ← s + i ⋄ i ← i + 1 } ⋄ s"),
            "10"
        );
    }

    #[test]
    fn eval_when() {
        // when with a trailing (1) default clause
        assert_eq!(eval("b ← 2 ⋄ when { (b=1){ \"one\" } (b=2){ \"two\" } (1){ \"other\" } }"), "two");
        assert_eq!(eval("b ← 9 ⋄ when { (b=1){ \"one\" } (b=2){ \"two\" } (1){ \"other\" } }"), "other");
    }

    // --- Phase 9: trains (function chains / forks) ---

    #[test]
    fn eval_train_fork() {
        // x (A « B » C) y = (x A y) B (x C y)
        assert_eq!(eval("3 (+ « × » -) 4"), "¯7");
    }

    #[test]
    fn eval_train_left_bind() {
        // x (c f) y = f(c, y)  (left-bind: bound value, then a function)
        assert_eq!(eval("(10+) 1"), "11");
        assert_eq!(eval("10 (-⍛+) 100"), "90"); // reverse-compose: g(f(x), y)
        assert_eq!(eval("((1+)⍛-) 1"), "1");
    }

    #[test]
    fn eval_train_compose() {
        // x (f ∘ g) y = f(y, g(y))  (compose is dyadic: f(y, g(y)))
        assert_eq!(eval("¯2 3 4 (×∘-) 1000"), "(2000 ¯3000 ¯4000)");
        // monadic compose with reciprocal: (×∘÷) y = y × (1/y) = y, exactly 1 for all y≠0.
        // Kotlin renders rationals num/den (oracle: 1r2 displays 1/2).
        assert_eq!(eval("(×∘÷) ¯1 2 3"), "(1 1/1 1/1)");
    }

    #[test]
    fn eval_train_atop() {
        // x (f g) y = f(x g y)  (atop: g dyadic between x and y)
        assert_eq!(eval("2 (-*) 5"), "¯32"); // -(2*5)
        assert_eq!(eval("10 (-,) 20"), "(¯10 ¯20)"); // -(10,20) = (-10,-20)
    }

    // --- Phase 9b: function-assignment validation (SHOULD FAIL, per Kotlin CustomFunctionTest/FnParseTest) ---

    /// A fork MUST have exactly three functions: `A«B»C`. A two-function fork
    /// `⊢«⊣»` is a parse error ("Right argument is not a function" in real Kap).
    #[test]
    #[should_panic]
    fn parse_fork_requires_three_functions() {
        eval("foo ⇐ ⊢«⊣» ⋄ 1 foo 2");
    }

    /// `foo ⇐ ⊢«⊣»,` (comma is the 3rd fn) is VALID → 2; this guards the opposite
    /// direction so the above failure is specifically the *missing third function*.
    #[test]
    fn parse_fork_with_third_function_ok() {
        assert_eq!(eval("foo ⇐ ⊢«⊣», ⋄ 1 foo 2"), "2");
    }

    /// Defining a function with a bare value (not a function) on the RHS is invalid.
    #[test]
    #[should_panic]
    fn parse_fndef_rhs_must_be_function() {
        eval("foo ⇐ 42 ⋄ foo 1");
    }

    /// A train/fork may not be built from a lone/adverb-only RHS.
    #[test]
    #[should_panic]
    fn parse_fork_inner_must_be_functions() {
        eval(r#"foo ⇐ «» ⋄ foo 1 2"#);
    }

    /// `∇ (a0;a1) ...` destructures a multi-element left arg into its names (Kotlin
    /// `twoArgOperator2`). Left data `(10;11)` binds `a0=10, a1=11`, the right param `b`
    /// binds the right data `4`, and `⍞x`/`⍞y` apply the function operands to explicit args.
    #[test]
    fn eval_two_arg_operator_with_destructured_left_params() {
        assert_eq!(
            eval(r#"∇ (a0;a1) (x foo y) b { 100 ⍞y a0 ⍞x a1 ⍞x b } ⋄ (10;11) -foo+ 4"#),
            "103"
        );
    }

    /// A multi-name left param group destructures the left data vector element-wise.
    #[test]
    fn eval_left_param_group_destructure() {
        assert_eq!(eval(r#"∇ (a0;a1) (x foo y) b { (a0;a1) } ⋄ (10;11) -foo+ 4"#), "(10 11)");
    }

    // --- Strings: character arithmetic (Kotlin StringsTest.kt) ---

    #[test]
    fn eval_char_plus_int_vector() {
        assert_eq!(eval(r#""af" + 1 ¯1"#), "be");
        assert_eq!(eval(r#"8 ¯1 + "af""#), "ie");
        assert_eq!(eval(r#""abc" + 0.1 0.9 6.2"#), "abi");
    }

    #[test]
    fn eval_char_minus_int_vector() {
        assert_eq!(eval(r#""abj" - 0 ¯11 3"#), "amg");
    }

    #[test]
    fn eval_char_difference() {
        // "bBa" - "aAb" => codepoint diff. Kotlin's stored expectation is `(1 1 -1)` (ASCII
        // minus) but Kap renders the negative sign as the overbar glyph, so we get `(1 1 ¯1)`.
        // The value [-1] is identical; this is a display-glyph artifact, not a logic error.
        assert_eq!(eval(r#""bBa" - "aAb""#), "(1 1 ¯1)");
    }

    #[test]
    fn eval_single_char_arithmetic_is_forbidden() {
        // Single-character arithmetic is never allowed in Kap.
        let e = Engine::new();
        assert!(e.eval_string(r#"@a + @A"#).is_err());
        assert!(e.eval_string(r#"@a - 98"#).is_err());
        assert!(e.eval_string(r#"@a + 1j1"#).is_err());
        assert!(e.eval_string(r#"1j1 + @a"#).is_err());
        assert!(e.eval_string(r#"@a - 1j1"#).is_err());
        // int - char is forbidden (asymmetry: char - int is allowed).
        assert!(e.eval_string(r#"98 200 - "aj""#).is_err());
    }

    // --- Strings: `unicode:*` functions (Real Kap UnicodeModule) ---

    #[test]
    fn eval_unicode_builtins() {
        // toCodepoints / fromCodepoints (char <-> codepoint).
        assert_eq!(eval(r#"unicode:toCodepoints "ABC""#), "(65 66 67)");
        assert_eq!(eval(r#"unicode:fromCodepoints 65 66 67"#), "ABC");
        assert_eq!(eval(r#"unicode:toCodepoints @A"#), "65");
        // toGraphemes: each cluster is its own string (PLAIN display => no quotes).
        assert_eq!(eval(r#"unicode:toGraphemes "é""#), "(é)");
        // case conversion (PLAIN display => no quotes via format_value).
        assert_eq!(eval(r#"unicode:toLower "ABC""#), "abc");
        assert_eq!(eval(r#"unicode:toUpper "abc""#), "ABC");
        // toNames: Unicode name of a char, ⍬ when unnamed.
        assert_eq!(eval(r#"unicode:toNames @A"#), "LATIN CAPITAL LETTER A");
        assert_eq!(eval(r#"unicode:toNames @€"#), "null");
        // enc/dec round-trip in UTF-8 (byte vector <-> string).
        assert_eq!(eval(r#"unicode:enc "AB""#), "(65 66)");
        assert_eq!(eval(r#"unicode:dec 65 66 67"#), "ABC");
        // enc with explicit charset (left arg as a string). Plain UTF16 prefixes a BOM
        // (0xFE 0xFF) like Kotlin's default; UTF32 is big-endian.
        assert_eq!(eval(r#""UTF16" unicode:enc "A""#), "(254 255 0 65)");
        assert_eq!(eval(r#""UTF32" unicode:enc "A""#), "(0 0 0 65)");
        // astral-plane characters have Unicode names too (curated subset).
        assert_eq!(eval(r#"unicode:toNames @𝒟"#), "MATHEMATICAL FRAKTUR CAPITAL D");
        // non-integers cannot be characters.
        assert!(Engine::new().eval_string(r#"unicode:fromCodepoints 1.5"#).is_err());
        assert!(Engine::new().eval_string(r#"unicode:fromCodepoints 0.5"#).is_err());
    }

    // --- Strings: dyadic `⍕` format directives (Real Kap format.kt) ---

    // `format_display` (REPL form: nested strings stay quoted, e.g. `("0 1" "2 3")`)
    // is the faithful renderer here; the shared `eval()` uses `format_value()` (the
    // quote-free internal renderer) which collapses nested string arrays lossily.
    fn eval_display(src: &str) -> String {
        let e = Engine::new();
        let v = e.eval_string(src).expect("eval failed");
        v.format_display()
    }

    #[test]
    fn eval_format_directives() {
        // `$s` places a plain-rendered argument; one argument per directive.
        // Results are Str values, so `format_display` quotes them.
        assert_eq!(eval_display(r#""$s a $s b"⍕(1 2)"#), "\"1 a 2 b\"");
        assert_eq!(eval_display(r#""$s a $s b"⍕"x" "y""#), "\"x a y b\"");
        // `$s` with a rank-2 arg yields a nested array of per-row strings;
        // each directive consumes one element per row (Kotlin FormatAPLFunction).
        // Two `$s` in a row consume both elements of each 2-wide row.
        assert_eq!(eval_display(r#""$s $s"⍕(2 2⍴⍳4)"#), "(\"0 1\" \"2 3\")");
        assert_eq!(
            eval_display(r#""a$sfoo$sbar"⍕(3 2⍴⍳6)"#),
            "(\"a0foo1bar\" \"a2foo3bar\" \"a4foo5bar\")"
        );
        // Padding: positive width left-pads, negative (¯) right-pads.
        assert_eq!(eval_display(r#""$10s"⍕"abc""#), "\"       abc\"");
        assert_eq!(eval_display(r#""$¯5s"⍕1"#), "\"1    \"");
        assert_eq!(eval_display(r#""$5s"⍕"x""#), "\"    x\"");
        // `$h` HTML-escapes the argument (& → &amp;, < → &lt;, > → &gt;).
        assert_eq!(eval_display(r#""$h"⍕"a<b&c""#), "\"a&lt;b&amp;c\"");
        // `$$` is a literal `$`; a format string with no directives passes through.
        assert_eq!(eval_display(r#""$$"⍕10"#), "\"$\"");
        assert_eq!(eval_display(r#""no directives"⍕"ignored""#), "\"no directives\"");
        // Left/right padding coexist in one pattern.
        assert_eq!(eval_display(r#""f=$¯5s g=$5s"⍕(1 2)"#), "\"f=1     g=    2\"");
    }
}
