//! Parser for Kap (Phase 2b).
//!
//! Recursive-descent parser over the token stream (`Vec<SpannedToken>`) producing an
//! `Instr` AST. LAZY EVALUATION NOTE: this parser builds TREES ONLY — it never evaluates.
//! Kap is lazy; laziness lives in the evaluator (Phase 3) via a `Deferred` value (a thunk
//! = unevaluated `Instr` + environment, forced on demand). Function arguments stay as
//! `Box<Instr>` trees here so they can be deferred. See PROGRESS.md "lazy evaluation".
//!
//! Grammar (core subset, faithful to docs/reference.asciidoc syntax):
//!   statement   := expr (⋄ expr)*
//!   expr        := assign
//!   assign      := leftarrow | apply
//!   leftarrow   := symbol ← apply
//!   apply       := term (fn term)*            // dyadic + trains
//!   term        := primary
//!   primary     := number | char | string | symbol | ( expr ) | [ elements ] | ⍬
//!   elements    := expr (; expr)*             // explicit list / array literal
//!   stranded vec: a run of whitespace-separated primaries at the same level becomes
//!                 an `Array` (APL stranding) — e.g. `1 2 3` -> Array[1,2,3].
//!
use crate::ast::{SyntaxMacro, SyntaxRule, SpecialToken, Instr, BooleanOpKind};
use crate::token::{LiteralValue, SpannedToken, Token};
use crate::AplError;

/// Result of the paren value/fn accumulator (`parse_paren_accum`) — mirrors Kotlin's
/// `ParseResultHolder` split between `InstrParseResult` (Value) and `FnParseResult`
/// (Fn), which drives processFn's branch selection (parser.kt:457–492).
#[derive(Debug)]
enum ParenHolder {
    /// A plain value instruction (Kotlin InstrParseResult).
    Value(Instr),
    /// A function-valued expression — a Train / Derived / symbol fn (Kotlin FnParseResult).
    Fn(Instr),
    /// Empty group `()` (Kotlin EmptyParseResult).
    Empty,
    /// Unrecoverable parse shape; caller restores position and falls back.
    Malformed,
}

/// Strand a run of instructions into an Array (single element → itself).
fn strand_instrs(mut items: Vec<Instr>) -> Instr {
    if items.len() == 1 {
        items.pop().unwrap()
    } else {
        Instr::Array { elements: items }
    }
}

/// Parse a full source string's token stream into a list of statement `Instr`s.
/// Returns `(statements, errors)`. `errors` is non-empty on parse failure.
///
/// `known_functions` is the set of names currently bound to user-defined (or
/// native) functions in the evaluation environment. It lets the parser tell a
/// *value* symbol from a *function* symbol so that `a c` strands to `(a c)`
/// (both values) while `foo 10` applies `foo` monadically and `x foo y` applies
/// `foo` dyadically. Without this, every bare symbol would be treated as an
/// operator and `a c` would wrongly parse as `a(c)`.
/// Parse a single statement starting at the parser's current position, advancing past
/// it (and any trailing `⋄` separator). Returns `Ok(Some(instr))` for a parsed statement,
/// `Ok(None)` at end-of-input, or `Err` on a parse failure. Intended for incremental
/// use by the Engine, which re-derives `known_functions` between statements so that a
/// function defined in one statement is visible to later statements in the same input.
pub fn parse(
    tokens: &[SpannedToken],
    known_functions: &[&str],
    known_ops: &[&str],
    macros: &std::collections::HashMap<String, crate::ast::SyntaxMacro>,
) -> (Vec<Instr>, Vec<AplError>) {
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        known_functions: known_functions.iter().map(|s| s.to_string()).collect(),
        known_ops: known_ops.iter().map(|s| s.to_string()).collect(),
        macros: macros.clone(),
        kotlin_close_stack: Vec::new(),
    };
    let mut stmts = Vec::new();
    let mut errors = Vec::new();
    loop {
        match p.parse_statements() {
            Ok(Some(instr)) => stmts.push(instr),
            Ok(None) => break,
            Err(e) => {
                errors.push(e);
                // bail out to avoid an infinite loop on a broken token stream
                break;
            }
        }
    }
    (stmts, errors)
}

pub struct Parser<'a> {
    pub toks: &'a [SpannedToken],
    pub pos: usize,
    /// Names currently bound to functions. Seeded with the eval environment's function
    /// names; also grown as the parser encounters `∇ name …` / `name ⇐ …` definitions,
    /// so a function body that references its own (or a mutually-earlier) name parses as
    /// an application rather than a strand. See `parse`.
    pub known_functions: Vec<String>,
    /// Names currently bound to user-defined *operators* (defined via `∇ (x foo) a`).
    /// Seeded/regrown like `known_functions` so an operator call (`X foo Y`) parses the
    /// operator name as an operator rather than a stranded value.
    pub known_ops: Vec<String>,
    /// Registered `defsyntax` macros (session-global, mirrored from `Engine::macros`).
    /// When a bare symbol matches a trigger name here, the parser expands the macro inline
    /// (Kotlin `syntax.kt`'s `processCustomSyntax`). Keyed by bare trigger name.
    pub macros: std::collections::HashMap<String, crate::ast::SyntaxMacro>,
    /// P1-M5: stack of closing tokens for nested Kotlin-path group parses. While
    /// non-empty, `finish_fn_call` parses the right argument with the SAME
    /// accumulator loop (respecting the close token), mirroring Kotlin's
    /// `parseExprToplevel(CloseParen)` context threading (:977).
    pub kotlin_close_stack: Vec<Token>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&SpannedToken> {
        self.toks.get(self.pos)
    }
    fn advance(&mut self) -> Option<&SpannedToken> {
        let t = self.toks.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    /// Whether `name` denotes a known function (so it should parse as an operator / apply).
    fn is_known_fn(&self, name: &str, namespace: &Option<String>) -> bool {
        // Reconstruct the qualified `ns:name` for namespaced builtins (`io:print`,
        // `unicode:enc`, …) so the lexer's split namespace/name still resolves correctly.
        let qual = match namespace {
            Some(ns) => format!("{}:{}", ns, name),
            None => name.to_string(),
        };
        self.known_functions.iter().any(|n| n == name || n == &qual)
            // P2: bare-namespace natives registered in the DEFAULT namespace
            // (engine.kt registers `sysparam` without a module qualifier).
            || (name == "sysparam")
            // Namespaced native builtins (`io:print`, `unicode:enc`, …) are resolved at
            // runtime in `eval_apply`; treat them as functions so the parser builds the
            // dyadic `L f R` form (preserving any left operand).
            || match qual.split_once(':') {
                Some((ns, base)) => {
                    (ns == "io" && matches!(base, "print" | "println"))
                        // P2 (ROADMAP §5 / engine.kt:436–466): the `math:` native
                        // function family — trig, hyperbolic, number theory.
                        || (ns == "math"
                            && matches!(
                                base,
                                "sin" | "cos" | "tan"
                                    | "asin" | "acos" | "atan" | "atan2" | "hypot"
                                    | "sinh" | "cosh" | "tanh"
                                    | "asinh" | "acosh" | "atanh"
                                    | "gcd" | "lcm" | "numerator" | "denominator"
                                    | "factor" | "divisors" | "primes" | "isPrime"
                                    | "round" | "formatRational"
                            ))
                        || (ns == "unicode"
                            && matches!(
                                base,
                                "toCodepoints" | "fromCodepoints" | "toGraphemes" | "toLower"
                                    | "toUpper" | "toNames" | "enc" | "dec"
                            ))
                        || (ns == "s" && matches!(base, "trimLeft" | "trimRight" | "trim"))
                        || (ns == "int" && matches!(base, "intern" | "symbolName" | "throwNative" | "unwindProtect" | "formatRational" | "libInitialised"))
                        // `int:proto v` is a native VALUE-RIGHT-ARG OPERATOR
                        // (engine.kt:505 registerNativeOperator("proto", ProtoOp(), "int")):
                        // it must parse as a function so `f int:proto v` builds the
                        // ValueOp form; the evaluator routes it to apply_proto_op.
                        || (ns == "int" && base == "proto")
                        || (ns == "default" && base == "sysparam")
                        || (ns == "kap" && base == "sysparam")
                        // P2 encoder ns (engine.kt:469–470, encoder/encoder.kt).
                        || (ns == "encoder" && matches!(base, "encode" | "decode"))
                        || (ns == "regex"
                            && matches!(
                                base,
                                "match" | "find" | "finderror" | "findall" | "replace" | "split"
                                    | "compile"
                            ))
                }
                None => false,
            }
    }
    /// Whether an RHS `Instr` is a function value (so `name ← <rhs>` should define a function).
    fn is_function_value(&self, v: &Instr) -> bool {
        match v {
            Instr::Lambda { .. }
            | Instr::Train { .. }
            | Instr::Derived { .. }
            | Instr::Block { .. } => true,
            // a symbol that is a known/primitive function name also defines a function
            Instr::Symbol { name, namespace } => {
                self.is_known_fn(name, namespace) || Self::is_primitive_op(name)
            }
            _ => false,
        }
    }
    /// Extract `(name, namespace)` from a symbol token.
    fn name_of(tok: &Token) -> (String, Option<String>) {
        match tok {
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                (name.clone(), namespace.clone())
            }
            _ => (String::new(), None),
        }
    }
    fn skip_newlines(&mut self) {
        while let Some(t) = self.peek() {
            if matches!(t.token, Token::Newline) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn err(&self, msg: &str) -> AplError {
        match self.peek() {
            Some(t) => AplError::Parse {
                line: t.line,
                col: t.col,
                msg: msg.to_string(),
            },
            None => AplError::Parse {
                line: 0,
                col: 0,
                msg: format!("unexpected end of input: {}", msg),
            },
        }
    }

    /// Parse a single statement starting at the parser's current position, advancing past
    /// it (and any trailing `⋄` separator). Returns `Ok(Some(instr))` for a parsed statement,
    /// `Ok(None)` at end-of-input, or `Err` on a parse failure. Intended for incremental
    /// use by the Engine, which re-derives `known_functions` between statements so that a
    /// function defined in one statement is visible to later statements in the same input.
    pub fn parse_statements(&mut self) -> Result<Option<Instr>, AplError> {
        self.skip_newlines();
        if self
            .peek()
            .map(|t| matches!(t.token, Token::EndOfFile))
            .unwrap_or(true)
        {
            return Ok(None);
        }
        // P1-M6 (ROADMAP §16.4): the Kotlin-accumulator parser is now the DEFAULT.
        // KAP_KOTLIN_PARSER=0 opts out to the legacy heuristic parser; block bodies
        // ({…} interiors) remain on legacy parse_block as a documented divergence.
        if std::env::var("KAP_KOTLIN_PARSER").as_deref() != Ok("0") {
            let instr = match self.parse_value_kotlin() {
                Ok(i) => i,
                // Sentinel from bind_operators_kotlin: an operator binding on a
                // VALUE (e.g. the `1/2` rational inside ⍎) — legacy handles it.
                Err(crate::AplError::Runtime(m))
                    if m.contains("__KOTLIN_FALLBACK__") =>
                {
                    let instr = self.parse_expr()?;
                    self.skip_newlines();
                    if let Some(t) = self.peek() {
                        if matches!(t.token, Token::StatementSeparator) {
                            self.advance();
                        }
                    }
                    return Ok(Some(instr));
                }
                Err(e) => return Err(e),
            };
            self.skip_newlines();
            if let Some(t) = self.peek() {
                if matches!(t.token, Token::StatementSeparator) {
                    self.advance();
                }
            }
            return Ok(Some(instr));
        }
        let instr = self.parse_expr()?;
        self.skip_newlines();
        if let Some(t) = self.peek() {
            if matches!(t.token, Token::StatementSeparator) {
                self.advance();
            }
        }
        Ok(Some(instr))
    }

    /// P1-M3 (ROADMAP §4 / `references/parser_migration.md`): direct translation of
    /// Kotlin `parseValueInner` (:875–1030) + `processFn` (:432–497). Operands accumulate
    /// left-to-right in `left_args`; a FUNCTION-SHAPED token (user fn symbol, `{…}` dfn,
    /// function-valued paren group) consumes the whole accumulated list at once via
    /// `finish_fn_call`. Constructs still outside this slice fall back to the legacy
    /// parser FROM STATEMENT START (primitives/operators/trains/forks/λ/∇).
    ///
    /// Active only under KAP_KOTLIN_PARSER=1; default path byte-identical to pre-M1.
    fn parse_value_kotlin(&mut self) -> Result<Instr, AplError> {
        let start = self.pos;
        let mut left_args: Vec<Instr> = Vec::new();
        loop {
            self.skip_newlines();
            let tok = match self.peek() {
                Some(t) => t.clone(),
                None => break,
            };
            // END_EXPR_TOKEN_LIST (parser.kt:1342), statement-level subset. M5: when
            // nested inside a group, its close token also ends the accumulation.
            if let Some(top) = self.kotlin_close_stack.last() {
                if std::mem::discriminant(&tok.token) == std::mem::discriminant(top) {
                    break;
                }
            }
            match &tok.token {
                Token::EndOfFile | Token::StatementSeparator => break,
                _ => {}
            }
            // Short-circuit `and` / `or`: the lexer emits them as plain SYMBOL names
            // (no dedicated tokens); the legacy parser string-matches at
            // parse_expr:1756. Infix over the accumulated left args.
            if let Token::Literal(LiteralValue::Symbol {
                name: nn,
                namespace: None,
            }) = &tok.token
            {
                if nn == "and" || nn == "or" {
                    let is_and = nn == "and";
                    self.advance();
                    let rhs = self.parse_value_kotlin()?;
                    let lhs = if left_args.len() == 1 {
                        left_args.pop().unwrap()
                    } else {
                        Instr::Array {
                            elements: left_args,
                        }
                    };
                    return Ok(Instr::BooleanOp {
                        op: if is_and {
                            BooleanOpKind::And
                        } else {
                            BooleanOpKind::Or
                        },
                        left: Box::new(lhs),
                        right: Box::new(rhs),
                    });
                }
            }
            match &tok.token {
                Token::Literal(LiteralValue::Number(n)) => {
                    self.advance();
                    left_args.push(Instr::Literal(LiteralValue::Number(n.clone())));
                }
                Token::Literal(LiteralValue::Str(s)) => {
                    self.advance();
                    left_args.push(Instr::Literal(LiteralValue::Str(s.clone())));
                }
                Token::Literal(LiteralValue::Char(c)) => {
                    self.advance();
                    left_args.push(Instr::Literal(LiteralValue::Char(*c)));
                }
                Token::APLNullSym => {
                    self.advance();
                    left_args.push(Instr::Empty);
                }
                // P1-M7 (parser.kt:1014–1015 IfToken/WhileToken → processIf/processWhile):
                // control-flow keyword blocks are plain INSTRUCTIONS. They are
                // statement-complete (no trailing operands), so RETURN them — the loop
                // cannot be relied on to stop, because skip_newlines() consumes the
                // Newline that Kotlin's END_EXPR_TOKEN_LIST (:1342) would terminate on.
                // The guard mirrors parse_keyword_prefix exactly (incl. the
                // defsyntax-overrides-`when` rule) so the expect() below cannot fire.
                Token::Literal(LiteralValue::Symbol { name, .. })
                    if name == "if"
                        || name == "while"
                        || (name == "when" && !self.macros.contains_key("when")) =>
                {
                    let instr = self
                        .parse_keyword_prefix()?
                        .expect("keyword prefix guaranteed by the match guard");
                    return Ok(instr);
                }
                // P1-M7 (parser.kt:999 FnDefSym → processFunctionDefinition :571): a ∇
                // definition with accumulated operands is a parse error; otherwise the
                // definition is processed and returned (APLNullValue in Kotlin — also
                // statement-complete, hence a direct return, not an append).
                Token::FnDefSym => {
                    if !left_args.is_empty() {
                        return Err(self.err("Function definition with non-null left argument"));
                    }
                    return self.parse_fn_def(); // consumes ∇ itself
                }
                // Symbol LITERALS (`'a`, `'ns:a`) are ordinary VALUE operands
                // (Kotlin parser.kt:1003 QuotePrefix → LiteralSymbol). They must
                // accumulate/strand like numbers: `'a 'b 'c` is a 3-strand of
                // symbols, NOT a function application. Without this arm the
                // statement bails wholesale to the legacy parser, whose strand
                // collector drops all but the last literal.
                Token::QuotePrefix | Token::Literal(LiteralValue::SymbolValue { .. }) => {
                    let v = self.parse_primary()?;
                    left_args.push(v);
                }
                Token::OpenBrace => {
                    // `{…}` is FUNCTION-SHAPED (Kotlin routes the lambda through
                    // processFn): `3 {⍺+⍵} 4` is dyadic; bare `{⍵×2}` is an ambivalent
                    // fn VALUE (:460).
                    let lam = match self.parse_primary() {
                        Ok(f) => f,
                        Err(_) => {
                            self.pos = start;
                            return self.parse_expr();
                        }
                    };
                    if !Self::is_function_expr(&lam) {
                        self.pos = start;
                        return self.parse_expr();
                    }
                    return self.finish_fn_call(lam, &mut left_args);
                }
                // `⍞name`: a *dynamic* function reference. Like a bare symbol or brace
                // dfn, it is FUNCTION-SHAPED (Kotlin routes the DynamicRef through
                // processFn), so it must flow through `finish_fn_call` — otherwise it
                // falls through to the legacy fallback, which cannot parse `⍞` inside a
                // parenthesised group and dies with "unexpected token in primary". This
                // is what breaks `util.kap` `((toBoolean ⍞fn)¨ arg) / arg` and any
                // `(f ⍞g)` train member. (util.kap handled fine by the oracle.)
                Token::ApplyToken => {
                    self.advance(); // consume ⍞
                    self.skip_newlines();
                    let tok = self
                        .peek()
                        .ok_or_else(|| self.err("expected a symbol after ⍞"))?;
                    let (name, namespace) = match &tok.token {
                        Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                            (name.clone(), namespace.clone())
                        }
                        _ => return Err(self.err("expected a symbol after ⍞")),
                    };
                    self.advance();
                    let dr = Instr::DynamicRef { name, namespace };
                    // A trailing adverb binds the dynamic ref as the derived
                    // function's operand: `⍞fn¨ arr` = each over ⍞fn.
                    if let Some(Token::Literal(LiteralValue::Symbol { name: adv, .. })) =
                        self.peek().map(|t| &t.token)
                    {
                        if Self::is_adverb(adv) {
                            let adv = adv.clone();
                            self.advance();
                            let dr = Instr::Derived {
                                func: Box::new(dr),
                                op: Box::new(Instr::Symbol {
                                    name: adv,
                                    namespace: None,
                                }),
                            };
                            return self.finish_fn_call(dr, &mut left_args);
                        }
                    }
                    return self.finish_fn_call(dr, &mut left_args);
                }
                Token::OpenParen => {
                    // parser.kt:977 parseExprToplevel(CloseParen): M5 parses the group
                    // CONTENT with this same accumulator loop under a close-token stack,
                    // so literal-first forks (`(1↑⍴)`) chain via left-bind exactly as
                    // Kotlin builds them, and a fn result feeds processFn (:984).
                    let save = self.pos;
                    self.advance(); // consume (
                    let group = {
                        self.kotlin_close_stack.push(Token::CloseParen);
                        let g = self.parse_value_kotlin();
                        self.kotlin_close_stack.pop();
                        match g {
                            Ok(g) => g,
                            Err(_) => {
                                self.pos = start;
                                return self.parse_expr();
                            }
                        }
                    };
                    // Consume the closing paren of the group.
                    if let Err(_) = self.expect(Token::CloseParen, "expected ) after group") {
                        self.pos = start;
                        return self.parse_expr();
                    }
                    if Self::is_function_expr(&group) {
                        // parser.kt:984 feeds a function-valued group to processFn.
                        // CRITICAL: when the group's own parse ended as an FnParseResult
                        // (no right arg inside the parens), processFn CONTINUES with the
                        // tokens after `)` — so `(1↑⍴) 3 4` chains (1↑) then ⍴ via
                        // Chain2, and `(≠⌸) v` derives the operator then applies to v.
                        return self.finish_fn_call(group, &mut left_args);
                    }
                    // Value group: restore nothing (parse_primary consumed it correctly)
                    // unless it failed to move; guard by re-checking position.
                    if self.pos == save {
                        self.pos = start;
                        return self.parse_expr();
                    }
                    left_args.push(group);
                }
                Token::LeftArrow => {
                    // `x ← v` (parser.kt:997 → processAssignment): target = last leftArg.
                    // Destructuring `(a b c) ← v`: the target is a value group of
                    // symbols → DestructAssign (matches the legacy path's behaviour).
                    let target = match left_args.pop() {
                        Some(t) => t,
                        None => return Err(self.err("assignment without a target")),
                    };
                    self.advance(); // consume ←
                    let value = self.parse_expr()?;
                    if let Instr::Array { elements } = &target {
                        if elements.iter().all(|e| matches!(e, Instr::Symbol { .. })) {
                            let names = elements
                                .iter()
                                .map(|e| match e {
                                    Instr::Symbol { name, namespace } => {
                                        (name.clone(), namespace.clone())
                                    }
                                    _ => unreachable!(),
                                })
                                .collect();
                            return Ok(Instr::DestructAssign {
                                names,
                                value: Box::new(value),
                            });
                        }
                    }
                    return Ok(Instr::Assign {
                        target: Box::new(target),
                        value: Box::new(value),
                    });
                }
                Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                    let name = name.clone();
                    let namespace = namespace.clone();
                    // defsyntax macro triggers keep working under the new path.
                    if namespace.is_none() {
                        if let Some(m) = self.macros.get(&name) {
                            let m = m.clone();
                            self.advance();
                            left_args.push(self.expand_macro(&m)?);
                            continue;
                        }
                        // `declare(…)` structural special form — same rationale as the
                        // legacy parse_primary arm: fn-valued members of an export list
                        // must NOT be classified as trains (util.kap:6).
                        if name == "declare"
                            && self
                                .toks
                                .get(self.pos + 1)
                                .map(|t| matches!(t.token, Token::OpenParen))
                                == Some(true)
                        {
                            self.advance(); // consume 'declare'
                            let arg = self.parse_declare_special()?;
                            return Ok(Instr::Apply {
                                fn_expr: Box::new(Instr::Symbol {
                                    name: "declare".to_string(),
                                    namespace: None,
                                }),
                                left: None,
                                right: Box::new(arg),
                            });
                        }
                        // defsyntax/defsyntaxsub DEFINITIONS are keyword-forms only
                        // legacy parse_expr knows; bail to it from statement start
                        // BEFORE the name can strand as an operand.
                        if matches!(name.as_str(), "defsyntax" | "defsyntaxsub") {
                            self.pos = start;
                            return self.parse_expr();
                        }
                    }
                    // parser.kt:961–964: a Name immediately followed by `⇐` is the
                    // SHORT-FORM fn definition (`name ⇐ rhs`) — handled BEFORE any
                    // function/value classification (and before the group arm can
                    // bind the name as a left arg). Lookahead 1 token.
                    if namespace.is_none()
                        && self
                            .toks
                            .get(self.pos + 1)
                            .map(|t| matches!(t.token, Token::DynassignToken))
                            == Some(true)
                    {
                        self.advance(); // consume the name
                        self.advance(); // consume ⇐
                        let value = self.parse_function_expr_impl(true)?;
                        // B2: a bare known-operator RHS is invalid.
                        if let Instr::Symbol { name: rhs, namespace: None } = &value {
                            if self.known_ops.iter().any(|n| n == rhs) {
                                return Err(self.err(&format!(
                                    "Operator without left function: {}",
                                    rhs
                                )));
                            }
                        }
                        // Kotlin processShortFormFn DEFINES the binding during parse
                        // (lookupFunction sees it immediately), so a later statement in
                        // the same block resolves `g ⍵` as an application. Mirror that.
                        if !self.known_functions.iter().any(|f| f == &name) {
                            self.known_functions.push(name.clone());
                        }
                        return Ok(Instr::FnAssign {
                            name,
                            namespace,
                            value: Box::new(value),
                        });
                    }
                    let qual = namespace
                        .clone()
                        .map(|ns| format!("{}:{}", ns, name))
                        .unwrap_or_else(|| name.clone());
                    // P1-M4: an OPERATOR name (adverb or user op) in the MAIN dispatch
                    // loop is invalid — Kotlin throws InvalidOperatorArgument here
                    // (parser.kt:970). Operators are consumed only inside parseOperator,
                    // bound to a function BEFORE any data operand (`data ⌸ fn`).
                    //
                    // DUAL-NATURE EXCEPTION: `/ ⌿ \ ⍀` are registered in Kotlin as BOTH
                    // a native function (SelectElements/Expand — value-left replicate/
                    // compress/expand) AND a native operator (reduce/scan — function-left).
                    // When a VALUE operand precedes them in the accumulator the name is the
                    // FUNCTION form and must flow through `finish_fn_call` (which strands
                    // the value-left and dispatches to eval's `replicate`/`expand`); the
                    // operator/reduce form is reached only via bind_operators_kotlin when a
                    // FUNCTION left operand binds first (e.g. `+/`). Without this exception
                    // `1 0 1 / 1 2 3` wrongly hits the "Operator without left function"
                    // guard. `known_ops` is left intact so `+/` still binds as reduce.
                    let is_dual_nature = matches!(name.as_str(), "/" | "⌿" | "\\" | "⍀");
                    if !is_dual_nature
                        && namespace.is_none()
                        && (Self::is_adverb(&name) || self.known_ops.iter().any(|n| n == &name))
                    {
                        return Err(self.err(&format!("Operator without left function: {}", name)));
                    }
                    // P1-M4: primitives now flow through processFn + parseOperator
                    // (parser.kt:969 → :437), replacing the blanket fallback.
                    if Self::is_primitive_op(&qual) {
                        self.advance();
                        let fn_instr = Instr::Symbol { name, namespace };
                        return self.finish_fn_call(fn_instr, &mut left_args);
                    }
                    // Keyword-namespace symbols (`:name`) are ALWAYS values —
                    // never function-shaped (parser.kt makeVariableRef).
                    let is_fn = (namespace.is_some() && namespace.as_deref() != Some("keyword"))
                        || self.is_known_fn(&name, &namespace)
                        || self.known_functions.iter().any(|f| f == &name);
                    if !is_fn {
                        // Keyword-namespace symbols (`:name`) are VALUES (parser.kt
                        // makeVariableRef); they must strand, never resolve as fns.
                        self.advance();
                        left_args.push(Instr::Symbol { name, namespace });
                        continue;
                    }
                    // Function-shaped USER/namespaced symbol → processFn (:969).
                    self.advance();
                    let fn_instr = Instr::Symbol { name, namespace };
                    return self.finish_fn_call(fn_instr, &mut left_args);
                }
                _ => {
                    // Unhandled token class: fall back to the legacy expression parser
                    // from the statement START (no partial consumption).
                    self.pos = start;
                    return self.parse_expr();
                }
            }
        }
        // makeResultList (parser.kt:206): a single accumulated operand IS the result,
        // unwrapped; several operands strand.
        match left_args.len() {
            0 => Err(self.err("empty expression")),
            1 => Ok(left_args.pop().unwrap()),
            _ => Ok(Instr::Array {
                elements: left_args,
            }),
        }
    }

    /// P1-M4 port of Kotlin `parseOperator` (:1273–1319): loop over trailing operator
    /// bindings on a just-parsed function. Order per Kotlin: optional `[axis]` first,
    /// then adverb (Derived) / user-operator (OpCall) binding. An operator followed by
    /// NOTHING is incomplete application (B1 semantics).
    fn bind_operators_kotlin(&mut self, mut cur: Instr) -> Result<Instr, AplError> {
        loop {
            self.skip_newlines();
            // An operator binding on a VALUE (`1 / 2`, the `1/2` rational literal
            // inside ⍎) is not operator application — Kotlin's processFn only
            // calls parseOperator after a FUNCTION. Legacy handles these shapes;
            // signal the caller to retry from statement start. EXCEPTION: a
            // *bound constant* before an adverb (`(2÷⍨) 8` → 4, `z ⇐ 2÷⍨`) IS
            // valid Kap: fall through to the adverb arm below which wraps it as
            // Derived{Literal(x), adverb} (evaluator bind case: y f x).
            if !Self::is_function_expr(&cur)
                && !matches!(cur, Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
            {
                if let Some(t) = self.peek().map(|t| t.token.clone()) {
                    if let Token::Literal(LiteralValue::Symbol { ref name, .. }) = t {
                        if Self::is_adverb(name)
                            || self.known_ops.iter().any(|n| n == name)
                        {
                            return Err(self.err("__KOTLIN_FALLBACK__"));
                        }
                    }
                }
            }
            // Fork postfix `f « g » h` (Kap 3-train, Kotlin parseOperator
            // LeftForkToken case :1295): binds after a FUNCTION exactly like the
            // adverbs — e.g. stat.kap `avg ⇐ +/«÷»≢` where the left member is the
            // DERIVED reduce `+/`. Checked at LOOP level (not inside the symbol
            // `if let` below) because « is a LeftForkToken, not a symbol.
            if matches!(self.peek().map(|t| &t.token), Some(Token::LeftForkToken))
                && Self::is_function_expr(&cur)
            {
                self.advance(); // consume «
                self.skip_newlines();
                let b = self.parse_function_atom()?;
                self.skip_newlines();
                self.expect(Token::RightForkToken, "expected » in fork")?;
                self.skip_newlines();
                let c = self.parse_function_atom()?;
                cur = Instr::Train {
                    funcs: vec![cur, b, c],
                    reverse: false,
                    compose: false,
                };
                continue;
            }
            // Power operator `f⍣n` / `f⍣g` (Kotlin parseOperator → PowerAPLOperator,
            // engine.kt:491): binds after a FUNCTION, exactly like the adverbs below,
            // but carries a combined right arg (value OR function) ⇒ ValueOp.
            if let Some(Token::Literal(LiteralValue::Symbol { ref name, ref namespace })) =
                self.peek().map(|t| &t.token)
            {
                // Dual / structural-under operator `wrapper ⍢ base` (Kotlin
            // StructuralUnderOp, engine.kt:501 registerNativeOperator("⍢")).
            // Binds after a FUNCTION exactly like ⍣: ValueOp carrying the base fn.
            if name == "⍢" && Self::is_function_expr(&cur) {
                self.advance();
                self.skip_newlines();
                let operand = self.parse_function_atom()?;
                cur = Instr::ValueOp {
                    func: Box::new(cur),
                    op_name: "⍢".to_string(),
                    operand: Box::new(operand),
                };
                continue;
            }
            if name == "⍣" && Self::is_function_expr(&cur) {
                    self.advance();
                    self.skip_newlines();
                    // Function-shaped operands (brace dfn, ⍞ref, name) parse as function
                    // atoms; everything else (numbers, parenthesised exprs) is a value
                    // expr — mirrors Kotlin's APLOperatorCombinedRightArg split between
                    // combineFunctions and combineFunctionAndExpr. The final MODE
                    // (iterate vs until) is decided at eval from the operand's VALUE.
                    let starts_fn = matches!(
                        self.peek().map(|t| &t.token),
                        Some(Token::OpenBrace)
                            | Some(Token::ApplyToken)
                            | Some(Token::Literal(LiteralValue::Symbol { .. }))
                    );
                    let operand = if starts_fn {
                        self.parse_function_atom()?
                    } else {
                        self.parse_primary()?
                    };
                    cur = Instr::ValueOp {
                        func: Box::new(cur),
                        op_name: "⍣".to_string(),
                        operand: Box::new(operand),
                    };
                    continue;
                }
                // Fork postfix `f « g » h` (Kap 3-train): binds after a FUNCTION
                // exactly like ⍣ — e.g. stat.kap `avg ⇐ +/«÷»≢` where the left
                // member is the DERIVED reduce `+/`. Mirrors the legacy-path arm in
                // parse_function_expr (parser.rs ~1756): middle fn between « »,
                // then a third function atom; nothing after » means the caller's
                // next token supplies the right member only in legacy — here we
                // REQUIRE it (Kotlin processFn always has a trailing fn for this
                // shape; `f « g »` alone falls through to the adverb arms).
                if matches!(self.peek().map(|t| &t.token), Some(Token::LeftForkToken))
                    && Self::is_function_expr(&cur)
                {
                    self.advance(); // consume «
                    self.skip_newlines();
                    let b = self.parse_function_atom()?;
                    self.skip_newlines();
                    self.expect(Token::RightForkToken, "expected » in fork")?;
                    self.skip_newlines();
                    let c = self.parse_function_atom()?;
                    cur = Instr::Train {
                        funcs: vec![cur, b, c],
                        reverse: false,
                        compose: false,
                    };
                    continue;
                }
                // Native value-op `f int:proto v` (Kotlin ProtoOp, engine.kt:505):
                // binds after a FUNCTION exactly like ⍣ — ValueOp with the proto
                // value as operand. This is the path `(↑ int:proto 5)` inside a
                // group takes (parse_value_kotlin → finish_fn_call → here).
                if namespace.as_deref() == Some("int")
                    && name == "proto"
                    && Self::is_function_expr(&cur)
                {
                    self.advance(); // consume int:proto
                    self.skip_newlines();
                    let operand = self.parse_apply()?;
                    cur = Instr::ValueOp {
                        func: Box::new(cur),
                        op_name: "int:proto".to_string(),
                        operand: Box::new(operand),
                    };
                    continue;
                }
            }
            // Compose `∘` / reverse-compose `⍛` (Kotlin parseOperator :1290 —
            // these are native two-arg operators, not symbols, so they need
            // dedicated token arms). Binds after a FUNCTION: left = cur, right =
            // parseFunctionForOperatorRightArg (SINGLE fn, op.kt:31).
            if Self::is_function_expr(&cur) {
                match self.peek().map(|t| t.token.clone()) {
                    Some(Token::ComposeToken) => {
                        self.advance();
                        self.skip_newlines();
                        let r = self.parse_function_atom()?;
                        cur = Instr::Train {
                            funcs: vec![cur, r],
                            reverse: false,
                            compose: true,
                        };
                        continue;
                    }
                    Some(Token::ReverseComposeToken) => {
                        self.advance();
                        self.skip_newlines();
                        let r = self.parse_function_atom()?;
                        cur = Instr::Train {
                            funcs: vec![cur, r],
                            reverse: true,
                            compose: true,
                        };
                        continue;
                    }
                    // Inner/outer product `f ∙ g` (Kotlin parseOperator →
                    // OuterInnerJoinOp, engine.kt:489). `∙` is U+2219, emitted as a
                    // plain Symbol token by the lexer. Binds after a FUNCTION: left
                    // fn = cur, right fn = ONE function atom (op.kt:31 shape). A
                    // leading `∘∙f` parses as ComposeToken(∘) then this arm with
                    // cur=Symbol("∘") — the NullFunction sentinel at eval.
                    Some(Token::Literal(LiteralValue::Symbol { ref name, .. }))
                        if name == "∙" && Self::is_function_expr(&cur) =>
                    {
                        self.advance();
                        self.skip_newlines();
                        let is_null_left =
                            matches!(&cur, Instr::Symbol { name, .. } if name == "∘");
                        let r = self.parse_function_atom()?;
                        cur = Instr::InnerProduct {
                            left_fn: if is_null_left { None } else { Some(Box::new(cur)) },
                            right_fn: Box::new(r),
                        };
                        continue;
                    }
                    _ => {}
                }
            }
            // parseAxis (:1321): `f[axis]` wraps into AxisApplied.
            if let Some(t) = self.peek() {
                if matches!(t.token, Token::OpenBracket) {
                    let axis_ok = matches!(
                        &cur,
                        Instr::Symbol { name, .. }
                            if matches!(name.as_str(), "+" | "-" | "×" | "÷" | "*" | "," | "⍪" | "⌽" | "⊖")
                    );
                    if axis_ok {
                        self.advance();
                        let axis = self.parse_apply()?;
                        // Consume `]` — without this the closer blocks the boundary
                        // check and the right argument never parses.
                        self.expect(Token::CloseBracket, "expected ] after axis specifier")?;
                        cur = Instr::AxisApplied {
                            func: Box::new(cur),
                            axis: Box::new(axis),
                        };
                        continue;
                    }
                }
            }
            let binding = match self.peek().map(|t| t.token.clone()) {
                Some(Token::Literal(LiteralValue::Symbol { name, namespace }))
                    if Self::is_adverb(&name) =>
                {
                    Some((name, namespace, false))
                }
                Some(Token::Literal(LiteralValue::Symbol { name, namespace }))
                    if self.known_ops.iter().any(|n| n == &name) =>
                {
                    Some((name, namespace, true))
                }
                _ => None,
            };
            let (op_name, ns, is_user_op) = match binding {
                Some(b) => b,
                None => break,
            };
            self.advance();
            self.skip_newlines();
            if is_user_op {
                let right_fn = if self.next_is_function_token() && !self.at_statement_boundary() {
                    Some(Box::new(self.parse_primary()?))
                } else {
                    None
                };
                if right_fn.is_none() && self.at_statement_boundary() {
                    return Err(self.err(&format!("Operator without left function: {}", op_name)));
                }
                cur = Instr::OpCall {
                    op: Box::new(Instr::Symbol { name: op_name, namespace: ns }),
                    left_fn: Box::new(cur),
                    right_fn,
                };
            } else {
                if self.at_statement_boundary() {
                    return Err(self.err(&format!("Operator without left function: {}", op_name)));
                }
                // Explicit axis on a reduce/scan derived fn (`+/[0] x`): Kotlin binds
                // the bracket to the FUNCTION operand of the reduction
                // (`+/[0]` ≡ `(+/)[0]`): Derived{ func: AxisApplied{fn,k}, op: / } —
                // so the evaluator's adverb arm finds adv_explicit_axis on its func.
                let wants_axis = matches!(
                    op_name.as_str(),
                    "/" | "reduce" | "\\" | "scan" | "⌿" | "⍀"
                ) && matches!(self.peek().map(|t| &t.token), Some(Token::OpenBracket));
                let func_part = if wants_axis {
                    self.advance(); // consume [
                    let ax = self.parse_value_kotlin()?;
                    self.expect(Token::CloseBracket, "expected ] after axis specifier")?;
                    Instr::AxisApplied {
                        func: Box::new(cur),
                        axis: Box::new(ax),
                    }
                } else {
                    cur
                };
                cur = Instr::Derived {
                    func: Box::new(func_part),
                    op: Box::new(Instr::Symbol { name: op_name, namespace: ns }),
                };
            }
        }
        Ok(cur)
    }

    /// P1-M3 port of Kotlin `processFn` (:432–495) valence resolution. Called with the
    /// just-parsed function-shaped instr and the CURRENT accumulated left args:
    /// - right empty & left empty  → the fn itself (ambivalent fn value, :460)
    /// - right empty & left nonempty → strand of lefts (left-bind territory, M4)
    /// - right value & left empty  → FunctionCall1Arg monadic (:468)
    /// - right value & left n      → FunctionCall2Arg, ⍺ = the SINGLE left arg or a
    ///   strand of several (makeResultList semantics, :474)
    fn finish_fn_call(&mut self, fn_instr: Instr, left_args: &mut Vec<Instr>) -> Result<Instr, AplError> {
        // M5: capture whether a NEWLINE immediately follows the function BEFORE
        // `bind_operators_kotlin` (which begins with `skip_newlines()`) can swallow it.
        // In Kap a newline terminates a statement (no operator/axis binds across it),
        // so a trailing newline means "fn value, no right arg" — even if the NEXT
        // statement's first token happens to be a known function. Without this,
        // `trim ⇐ a b\n declare(...)` merged into `trimLeft declare(...)` and eagerly
        // applied trimLeft's body with no right arg. Fork/operator binding on the SAME
        // line (no intervening newline) is unaffected because there is no newline to
        // record here.
        let newline_before_right = matches!(self.peek().map(|t| &t.token), Some(Token::Newline));
        // parseOperator FIRST (parser.kt:437): bind axis / adverbs / user operators.
        let fn_instr = self.bind_operators_kotlin(fn_instr)?;
        self.skip_newlines();
        // M5: boundary check must respect a nested close token. Inside `(f …)` the
        // CloseParen is NOT "no right arg" — Kotlin's parseValue() recurses and only
        // stops at its endToken (:441 parseExprToplevel(FunctionCallCloseParen)).
        let at_close = matches!(self.peek().map(|t| &t.token), Some(t)
            if self.kotlin_close_stack.last().map(|c| {
                std::mem::discriminant(c) == std::mem::discriminant(t)
            }) == Some(true));
        let has_right = !newline_before_right
            && !at_close
            && !self.at_statement_boundary()
            && !matches!(
                self.peek().map(|t| &t.token),
                Some(Token::StatementSeparator) | Some(Token::EndOfFile)
            );
        if !has_right {

            // parser.kt:459–466: empty right + empty left ⇒ fn ITSELF (ambivalent fn
            // value); empty right + non-empty left ⇒ makeLeftBindFunctionParseResult
            // (:462) — LeftAssignedFunction (functions.kt:628): binds strand(leftArgs)
            // as ⍺, errors on a 2nd arg (LeftAssigned2ArgException).
            if left_args.is_empty() {
                return Ok(fn_instr);
            }
            // makeResultList (:206/:515): a SINGLE left arg passes UNWRAPPED —
            // LeftBind(10, +) binds ⍺=10, NOT ⍺=(10). Several strand.
            let bound = if left_args.len() == 1 {
                left_args.pop().unwrap()
            } else {
                Instr::Array {
                    elements: std::mem::take(left_args),
                }
            };
            return Ok(Instr::Train {
                funcs: vec![bound, fn_instr],
                reverse: false,
                compose: false,
            });
        }
        // M5: nested context parses the right argument with the SAME accumulator loop
        // so it stops at this group's close token instead of running past it.
        let right = if self.kotlin_close_stack.is_empty() {
            self.parse_apply()?
        } else {
            let r = self.parse_value_kotlin()?;
            let at_close_now = matches!(self.peek().map(|t| &t.token), Some(t)
                if self.kotlin_close_stack.last().map(|c| {
                    std::mem::discriminant(c) == std::mem::discriminant(t)
                }) == Some(true));
            if at_close_now && self.nested_right_is_fn_result(&r) {
                // parser.kt:479–491 FnParseResult branch: right is a FUNCTION ⇒
                // Chain2(parsedFn, right) — atop composition, returned as a fn value.
                // When leftArgs is NON-EMPTY, Kotlin wraps parsedFn in
                // makeLeftBindFunction(leftArgs, parsedFn) FIRST (:486), then
                // Chain2s with holder.fn — i.e. `Train[Train[strand(leftArgs), fn], r]`.
                // Without this wrap, `(¯1r2 0+2÷⍨≢)` drops the value `3/2 2` and
                // builds `Train[÷⍨, ≢]` (evaluates to `1`, not `⟨3/2 2⟩`).
                let outer_fn = if left_args.is_empty() {
                    fn_instr
                } else {
                    let bound = if left_args.len() == 1 {
                        left_args.pop().unwrap()
                    } else {
                        Instr::Array {
                            elements: std::mem::take(left_args),
                        }
                    };
                    Instr::Train {
                        funcs: vec![bound, fn_instr],
                        reverse: false,
                        compose: false,
                    }
                };
                return Ok(Instr::Train {
                    funcs: vec![outer_fn, r],
                    reverse: false,
                    compose: false,
                });
            }
            r
        };
        if left_args.is_empty() {
            // parser.kt:479–484 FnParseResult branch: when the right argument is a
            // FUNCTION, Kotlin forms Chain2(parsedFn, holder.fn) — a 2-train ATOP
            // `(f g) x = f(g(x))` (instr.kt:588) — rather than applying f to g. The
            // port's apply_train already implements this (evaluator.rs:2731). This
            // is how `trim ⇐ trimRight trimLeft` becomes the derived fn
            // `(trimRight trimLeft)`, NOT `Apply{trimRight, trimLeft}` (which would
            // evaluate the right arg with no ⍵ and fail "undefined symbol: ⍺").
            // Value right-args (numbers, arrays, strings, variables) still apply
            // normally via FunctionCall1Arg below.
            if self.nested_right_is_fn_result(&right) {
                return Ok(Instr::Train {
                    funcs: vec![fn_instr, right],
                    reverse: false,
                    compose: false,
                });
            }
            // FunctionCall1Arg (parser.kt:468): monadic, ⍵ = right.
            return Ok(Instr::Apply {
                fn_expr: Box::new(fn_instr),
                left: None,
                right: Box::new(right),
            });
        }
        // FunctionCall2Arg (parser.kt:474): ⍺ = makeResultList(leftArgs) — ONE operand
        // passes through UNWRAPPED (`3 g 4` binds ⍺=3, NOT ⍺=(3)).
        if left_args.len() == 1 {
            let left = left_args.pop().unwrap();
            return Ok(Instr::Apply {
                fn_expr: Box::new(fn_instr),
                left: Some(Box::new(left)),
                right: Box::new(right),
            });
        }
        let strand = Instr::Array {
            elements: std::mem::take(left_args),
        };
        Ok(Instr::Apply {
            fn_expr: Box::new(fn_instr),
            left: Some(Box::new(strand)),
            right: Box::new(right),
        })
    }

    /// Whether the nested right-argument parse result `r` corresponds to Kotlin's
    /// FnParseResult (parser.kt:479–491). Kotlin distinguishes by the parse RESULT
    /// TYPE (Function vs Value); the port must classify structurally. A bare
    /// `Instr::Symbol` is a VARIABLE (ValueParseResult) unless it names a known
    /// function/primitive — misclassifying it chains the operator into an atop
    /// Train and later fails with "unknown function: x" (the `(2+x)` / io.kap
    /// `code` regression).
    fn nested_right_is_fn_result(&self, r: &Instr) -> bool {
        match r {
            Instr::Symbol { name, namespace } => {
                let qual = namespace
                    .as_deref()
                    .map(|ns| format!("{}:{}", ns, name))
                    .unwrap_or_else(|| name.clone());
                Self::is_primitive_op(&qual)
                    || self.is_known_fn(name, namespace)
                    || self.known_functions.iter().any(|f| f == name)
            }
            other => Self::is_function_expr(other),
        }
    }

    /// Expect the next token to be `tok`; consume it or return a parse error.
    fn expect(&mut self, tok: Token, msg: &str) -> Result<(), AplError> {        match self.peek() {
            Some(t) if std::mem::discriminant(&t.token) == std::mem::discriminant(&tok) => {
                self.advance();
                Ok(())
            }
            _ => Err(self.err(msg)),
        }
    }

    /// Parse a *guarded expression* `cond : truthy ⋄ falsy` (Kap's `:` operator).
    /// Called from the main `parse_expr` after the base expression is parsed: if a `:` follows,
    /// the truthy branch is parsed, then a mandatory `⋄`, then the falsy branch.
    fn parse_expr(&mut self) -> Result<Instr, AplError> {
        // Traditional function definition `∇ ...` takes precedence.
        if let Some(t) = self.peek() {
            if matches!(t.token, Token::FnDefSym) {
                return self.parse_fn_def();
            }
        }
        // Control-flow keywords are *syntactic* (not symbols): `if`/`while`/`when`.
        // Detect a leading keyword symbol and dispatch to the dedicated parser. This
        // must run in BOTH `parse_expr` (statement start) and `parse_apply` (value /
        // assignment positions, e.g. `x ← if (…) …` or `⊢ if (…) …`), otherwise the
        // keyword strands as a bare symbol and errors as "undefined symbol".
        if let Some(instr) = self.parse_keyword_prefix()? {
            return Ok(instr);
        }
        // `defsyntax` / `defsyntaxsub` direct the parser (Kotlin `processDefsyntax`).
        // The form is `name (the macro trigger) defsyntax (rules…) { body }`. Detect a
        // leading symbol immediately followed by the `defsyntax`/`defsyntaxsub` keyword.
        if let Some(instr) = self.parse_defsyntax_directive()? {
            return Ok(instr);
        }
        let base = self.parse_assign()?;
        // Guarded expression: `base : truthy ⋄ falsy`.
        if let Some(t) = self.peek() {
            if matches!(t.token, Token::ColonSym) {
                self.advance();
                let truthy = self.parse_expr()?;
                if let Some(s) = self.peek() {
                    if matches!(s.token, Token::StatementSeparator) {
                        self.advance();
                        let falsy = self.parse_expr()?;
                        return Ok(Instr::Guard {
                            cond: Box::new(base),
                            truthy: Box::new(truthy),
                            falsy: Box::new(falsy),
                        });
                    }
                }
                return Err(self.err("expected ⋄ after guarded expression (cond : a ⋄ b)"));
            }
        }
        Ok(base)
    }

    /// block := `{` statement* `}`  — a sequence of statements separated by ⋄/;/newline.
    fn parse_block(&mut self) -> Result<Instr, AplError> {
        // Assumes the opening `{` (OpenBrace) has already been consumed.
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if matches!(t.token, Token::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(t) if matches!(t.token, Token::StatementSeparator) => {
                    self.advance();
                    continue;
                }
                Some(t) if matches!(t.token, Token::ListSeparator) => {
                    // `;` also separates statements inside a block
                    self.advance();
                    continue;
                }
                Some(_) => {
                    // P1-M7 / M5b-2: block-body statements go through the Kotlin
                    // accumulator loop, which now carries ALL statement-level arms
                    // (keyword blocks, ∇ defs, primitives, operators). On any error,
                    // retry the statement via legacy parse_expr — a per-statement
                    // fallback, not a whole-block bailout, so mixed bodies work.
                    self.skip_newlines();
                    let save = self.pos;
                    let stmt = match self.parse_value_kotlin() {
                        Ok(s) => s,
                        Err(crate::AplError::Runtime(m))
                            if m.contains("__KOTLIN_FALLBACK__") =>
                        {
                            self.pos = save;
                            self.parse_expr()?
                        }
                        Err(_) => {
                            self.pos = save;
                            self.parse_expr()?
                        }
                    };
                    body.push(stmt);
                }
                None => return Err(self.err("expected '}' to close block")),
            }
            // after a statement, swallow a trailing separator
            self.skip_newlines();
            if let Some(t) = self.peek() {
                if matches!(t.token, Token::StatementSeparator)
                    || matches!(t.token, Token::ListSeparator)
                {
                    self.advance();
                }
            }
        }
        Ok(Instr::Block { body })
    }

    /// Shared control-flow keyword dispatch for `if`/`while`/`when`. Returns `Ok(Some(instr))`
    /// when the next token is one of those keywords (consuming it), or `Ok(None)` otherwise.
    /// Called from BOTH `parse_expr` (statement start) and `parse_apply` (value / assignment
    /// positions) so that e.g. `x ← if (…) …` and `⊢ if (…) …` parse correctly instead of
    /// stranding `if` as a bare symbol.
    fn parse_keyword_prefix(&mut self) -> Result<Option<Instr>, AplError> {
        if let Some(t) = self.peek() {
            if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                match name.as_str() {
                    "if" => return Ok(Some(self.parse_if()?)),
                    "while" => return Ok(Some(self.parse_while()?)),
                    // `when` is intercepted by the hardcoded builtin ONLY when no defsyntax
                    // macro named `when` is registered. The stdlib's `structure.kap` defines
                    // `when`/`whenInner` as defsyntax macros (same `when { … }` surface); in
                    // that case the macro path in `parse_primary` must handle expansion. (Kotlin
                    // registerCustomSyntax overrides the keyword form.)
                    "when" if !self.macros.contains_key("when") => {
                        return Ok(Some(self.parse_when()?))
                    }
                    _ => {}
                }
            }
        }
        Ok(None)
    }

    /// if (cond) { then } [ else { alt } ]
    fn parse_if(&mut self) -> Result<Instr, AplError> {
        // `if` is the current token (dispatch in parse_expr peeked it but did not consume).
        self.advance();
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
            return Err(self.err("expected '(' after if"));
        }
        self.advance();
        let cond = self.parse_expr()?;
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
            return Err(self.err("expected ')' after if condition"));
        }
        self.advance();
        self.skip_newlines();
        let then_block = self.parse_block_body()?;
        // optional `else { ... }`
        let mut else_block = None;
        self.skip_newlines();
        if let Some(t) = self.peek() {
            if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                if name == "else" {
                    self.advance();
                    self.skip_newlines();
                    else_block = Some(Box::new(self.parse_block_body()?));
                }
            }
        }
        Ok(Instr::If {
            cond: Box::new(cond),
            then_block: Box::new(then_block),
            else_block,
        })
    }

    /// Helper: expect `{ body }` and return the Block.
    fn parse_block_body(&mut self) -> Result<Instr, AplError> {
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
            return Err(self.err("expected '{' to start block"));
        }
        self.advance();
        self.parse_block()
    }

    /// while (cond) { body }
    fn parse_while(&mut self) -> Result<Instr, AplError> {
        self.advance();
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
            return Err(self.err("expected '(' after while"));
        }
        self.advance();
        let cond = self.parse_expr()?;
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
            return Err(self.err("expected ')' after while condition"));
        }
        self.advance();
        self.skip_newlines();
        let body = self.parse_block_body()?;
        Ok(Instr::While {
            cond: Box::new(cond),
            body: Box::new(body),
        })
    }

    /// when { (cond){ body } … (1){ default } }
    fn parse_when(&mut self) -> Result<Instr, AplError> {
        self.advance();
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
            return Err(self.err("expected '{' after when"));
        }
        self.advance();
        let mut clauses = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if matches!(t.token, Token::CloseBrace) => {
                    self.advance();
                    break;
                }
                _ => {}
            }
            // each clause: ( cond ) { body }
            self.skip_newlines();
            if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
                return Err(self.err("expected '(' to start a when clause"));
            }
            self.advance();
            let cond = self.parse_expr()?;
            self.skip_newlines();
            if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
                return Err(self.err("expected ')' after when clause condition"));
            }
            self.advance();
            self.skip_newlines();
            let body = self.parse_block_body()?;
            clauses.push((cond, body));
        }
        if clauses.is_empty() {
            return Err(self.err("when requires at least one clause"));
        }
        Ok(Instr::When { clauses })
    }

    /// assign := symbol ← apply  (left-associative target)
    fn parse_assign(&mut self) -> Result<Instr, AplError> {
        let save = self.pos;
        // Destructuring assignment: `(a b c) ← expr` — bind each LHS symbol to the
        // corresponding element of the (vector) RHS. Kap supports this (Kotlin
        // `AssignmentInstruction` with multiple targets). Detect `( name name … ) ←`.
        self.skip_newlines();
        if matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
            let open = self.pos;
            self.advance();
            // gather bare symbols until the matching `)`
            let mut names: Vec<(String, Option<String>)> = Vec::new();
            let mut ok = true;
            loop {
                self.skip_newlines();
                match self.peek() {
                    Some(t) => match &t.token {
                        Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                            names.push((name.clone(), namespace.clone()));
                            self.advance();
                        }
                        Token::CloseParen => {
                            self.advance();
                            break;
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    },
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                self.skip_newlines();
                if matches!(self.peek(), Some(t) if matches!(t.token, Token::LeftArrow)) {
                    self.advance();
                    let value = self.parse_apply()?;
                    return Ok(Instr::DestructAssign {
                        names,
                        value: Box::new(value),
                    });
                }
            }
            // Not a destructuring assignment; rewind and fall through to normal parsing.
            self.pos = open;
        }
        // lookahead: symbol ← ...
        if let Some(t) = self.peek() {
            if let Token::Literal(LiteralValue::Symbol { .. }) = &t.token {
                let name_tok = t.clone();
                self.advance();
                self.skip_newlines();
                if let Some(n) = self.peek() {
                    if matches!(n.token, Token::LeftArrow) {
                        self.advance();
                        let value = self.parse_apply()?;
                        let target = instr_from_symbol(&name_tok.token);
                        // A regular `←` assignment with a function-valued RHS (`{…}`, `λ`,
                        // train, derived, or a known function name) defines that name as a
                        // function — same as `⇐` — so that later uses (`g 5`) apply it rather
                        // than stranding. Pure `←` would otherwise *evaluate* the block here,
                        // which fails on `⍵`/`⍺` and never registers the name.
                        if self.is_function_value(&value) {
                            let (nm, ns) = Self::name_of(&name_tok.token);
                            if !self.known_functions.iter().any(|x| x == &nm) {
                                self.known_functions.push(nm.clone());
                            }
                            return Ok(Instr::FnAssign {
                                name: nm,
                                namespace: ns,
                                value: Box::new(value),
                            });
                        }
                        return Ok(Instr::Assign {
                            target: Box::new(target),
                            value: Box::new(value),
                        });
                    }
                    if matches!(n.token, Token::DynassignToken) {
                        return self.parse_fn_assign(&name_tok.token);
                    }
                }
                // not an assignment; rewind
                self.pos = save;
            }
        }
        // Short-circuit boolean operators `and` / `or` (Kotlin `AndToken`/`OrToken` →
        // `BooleanAndFunction`/`BooleanOrFunction`) sit at the **lowest precedence**, below
        // assignment. They are NOT the bitwise `∧`/`∨` functions. `a and b` evaluates `a`
        // first; if its truthiness decides the result, `b` is not evaluated. The result is
        // the *raw* operand (not coerced to 0/1). Left-associative, repeatable:
        // `a and b and c` → `((a and b) and c)`.
        let mut expr = self.parse_apply()?;
        loop {
            // Peek the next token: an `and`/`or` keyword (a bare *symbol* whose name is
            // exactly "and"/"or") begins a boolean-op chain. Anything else ends the level.
            let kind = match self.peek() {
                Some(t) => match &t.token {
                    Token::Literal(LiteralValue::Symbol { name, namespace })
                        if namespace.is_none() && name == "and" =>
                    {
                        Some(BooleanOpKind::And)
                    }
                    Token::Literal(LiteralValue::Symbol { name, namespace })
                        if namespace.is_none() && name == "or" =>
                    {
                        Some(BooleanOpKind::Or)
                    }
                    _ => None,
                },
                None => None,
            };
            let kind = match kind {
                Some(k) => k,
                None => break,
            };
            self.advance(); // consume `and`/`or`
            let right = self.parse_apply()?;
            expr = Instr::BooleanOp {
                op: kind,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    /// Parse `name ⇐ <fn-expr>` — dynamic function assignment. This is a distinct
    /// syntactic form from `←`: the RHS is a function-valued expression (lambda, train,
    /// builtin name, or another named function) compiled into a `UserFn`.
    fn parse_fn_assign(&mut self, name_tok: &Token) -> Result<Instr, AplError> {
        let (name, namespace) = match name_tok {
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                (name.clone(), namespace.clone())
            }
            _ => return Err(self.err("expected a symbol before ⇐")),
        };
        self.advance(); // consume ⇐
        // A primitive operator/function name cannot be reassigned to a function.
        if Self::is_primitive_op(&name) {
            return Err(self.err(&format!("cannot redefine primitive function '{}'", name)));
        }
        // Register the name as a *known function* BEFORE parsing the RHS body, so a
        // self-referential call inside the body (e.g. `foo ⇐ { … foo (⍵-1) … }`) parses
        // the inner `foo (…)` as a monadic *application* rather than being stranded as a
        // value (`[foo, …]`). Without this, the body is parsed while `foo` is still
        // unknown, so recursion/closures that call themselves silently break. (Kotlin's
        // UserFunctionDescriptor registration has the same effect — the defining name is
        // in scope throughout its own body.)
        if !self.known_functions.iter().any(|n| n == &name) {
            self.known_functions.push(name.clone());
        }
        // The RHS is a *function expression*: a lambda, a named function, an operator
        // (possibly with adverbs, e.g. `×/`), a train, or a parenthesised function group.
        // Parse it as a function expression (not `parse_apply`, which would over-consume
        // a trailing data operand) and store the resulting function value.
        let value = self.parse_function_expr_impl(true)?;
        // Validation: the RHS must be a function, not a value.
        if !matches!(
            value,
            Instr::Lambda { .. }
                | Instr::Train { .. }
                | Instr::Symbol { .. }
                | Instr::Derived { .. }
                | Instr::Block { .. }
        ) {
            return Err(self.err(&format!("'{} ⇐' requires a function on the right", name)));
        }
        // B2 (code_analysis_03): a bare known-operator symbol as the `⇐` RHS is a parse error
        // in Real Kap (`foo ⇐ ⌸` → "Operator without left function: ⌸", Kotlin
        // parser.kt:967–971 InvalidOperatorArgument). Operators are never values.
        if let Instr::Symbol { name: rhs, namespace: None } = &value {
            if self.known_ops.iter().any(|n| n == rhs) {
                return Err(self.err(&format!("Operator without left function: {}", rhs)));
            }
        }
        Ok(Instr::FnAssign {
            name,
            namespace,
            value: Box::new(value),
        })
    }

    /// `∇ name { body }` | `∇ (x;y) name (a;b) { body }` | `∇ x name y { body }`
    /// Parses whitespace-separated components: a bare `Name` or a `( … )` group. Collects
    /// them in order; then:
    ///   * 1 component  -> name (monadic `⍺`/`⍵`)
    ///   * 2 components -> name (1st), right-args (2nd)
    ///   * 3 components -> left-args (1st), name (2nd), right-args (3rd)
    fn parse_fn_def(&mut self) -> Result<Instr, AplError> {
        self.advance(); // consume ∇
        let mut components: Vec<(Vec<String>, bool)> = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if matches!(t.token, Token::OpenBrace) => break,
                Some(t) if matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) => {
                    if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                        components.push((vec![name.clone()], false));
                    }
                    self.advance();
                }
                Some(t) if matches!(t.token, Token::OpenParen) => {
                    self.advance();
                    let mut params = Vec::new();
                    let mut used_sep = false;
                    loop {
                        self.skip_newlines();
                        match self.peek() {
                            Some(t) if matches!(t.token, Token::CloseParen) => {
                                self.advance();
                                break;
                            }
                            Some(t)
                                if matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) =>
                            {
                                if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                                    params.push(name.clone());
                                }
                                self.advance();
                            }
                            Some(t)
                                if matches!(t.token, Token::ListSeparator) =>
                            {
                                used_sep = true;
                                self.advance();
                            }
                            _ => return Err(self.err("expected a parameter name")),
                        }
                    }
                    if params.is_empty() {
                        return Err(self.err("empty parameter group in function definition"));
                    }
                    components.push((params, used_sep));
                }
                _ => return Err(self.err("expected function name after ∇")),
            }
            // After the name+params, a `{` begins the body. Stop collecting components
            // once we see it (it's not part of the name spec).
            if matches!(self.peek().map(|t| &t.token), Some(Token::OpenBrace)) {
                break;
            }
        }
        if components.is_empty() {
            return Err(self.err("no function name specified"));
        }
        // Determine the *name component* and whether this is an operator definition
        // (name component has >= 2 symbols, e.g. `(x foo)` or `(x foo y)`).
        //   * 1 component            -> name only                                  (fn def)
        //   * 2 components:
        //       comp0.symbols.len()>=2 -> comp0 is the name component (operator/op args)
        //                               right args = comp1
        //       else                   -> name = comp0[0], right = comp1           (fn def)
        //   * 3 components            -> left=comp0, nameComp=comp1, right=comp2    (fn or op)
        // For an operator definition, the name component's symbols split as
        //   [op_left, name, op_right?]  (op_right only when >= 3 symbols).
        let (name, op_left, op_right, left_params, right_params): (
            String,
            Option<String>,
            Option<String>,
            Vec<String>,
            Vec<String>,
        ) = match components.len() {
            1 => (components[0].0[0].clone(), None, None, vec![], vec![]),
            2 => {
                if components[0].0.len() >= 2 {
                    let nc = &components[0].0;
                    let op_left = Some(nc[0].clone());
                    let name = nc[1].clone();
                    let op_right = nc.get(2).cloned();
                    (name, op_left, op_right, vec![], components[1].0.clone())
                } else {
                    (components[0].0[0].clone(), None, None, vec![], components[1].0.clone())
                }
            }
            3 => {
                let nc = &components[1].0;
                if nc.len() >= 2 {
                    let op_left = Some(nc[0].clone());
                    let name = nc[1].clone();
                    let op_right = nc.get(2).cloned();
                    (name, op_left, op_right, components[0].0.clone(), components[2].0.clone())
                } else {
                    (nc[0].clone(), None, None, components[0].0.clone(), components[2].0.clone())
                }
            }
            _ => return Err(self.err("invalid function definition format")),
        };
        let is_op = op_left.is_some();
        // --- Validation (Kap semantics; mirrors Kotlin ParseException cases) ---
        // 1. A `;`/`,` separator is not allowed inside the *operator-name* component.
        // 2. An operator name component may have at most 3 symbols (op_left, name, op_right).
        // 3. A parameter group with multiple names must be separated by `;`/`,`.
        // 4. Parameter names must be distinct across all groups.
        if is_op {
            let nc = if components.len() == 2 { &components[0] } else { &components[1] };
            if nc.1 {
                return Err(self.err("semicolon is not allowed in an operator name"));
            }
            if nc.0.len() > 3 {
                return Err(self.err("too many arguments for an operator"));
            }
        }
        for (i, (names, used_sep)) in components.iter().enumerate() {
            // The name component of an operator (>= 2 symbols, e.g. `(x foo y)`) uses
            // *spaces*, not `;` — it is exempt from the "multi-name must use ;" rule (that
            // rule applies to parameter groups). Skip it here; its own checks are above.
            let is_name_component = is_op && ((components.len() == 2 && i == 0) || i == 1);
            if is_name_component {
                continue;
            }
            if names.len() > 1 && !used_sep {
                return Err(self.err("a parameter group with multiple names must use ; as a separator"));
            }
        }
        {
            let mut seen = std::collections::HashSet::new();
            for (names, _) in &components {
                for p in names {
                    if !seen.insert(p.clone()) {
                        return Err(self.err(&format!("duplicated argument name '{}'", p)));
                    }
                }
            }
        }
        // Register the name as a known function/operator *now* (before the body is parsed)
        // so a recursive reference inside the body parses as an application.
        if is_op {
            if !self.known_ops.iter().any(|n| n == &name) {
                self.known_ops.push(name.clone());
            }
        } else if !self.known_functions.iter().any(|n| n == &name) {
            self.known_functions.push(name.clone());
        }
        self.skip_newlines();
        let body = self.parse_block_body()?; // expects `{ … }`
        // NOTE: Kap allows a library `∇`/`⇐` definition to *shadow* a primitive name in
        // its own namespace (e.g. `math-kap.kap` redefines `⊥`/`⊤`). We deliberately do
        // NOT reject redefinition of primitive names here; the evaluator resolves a bound
        // user/native function before falling back to the hardcoded builtin table, so the
        // stdlib version wins. (Parameter-name distinctness is still enforced below.)
        {
            let mut seen = std::collections::HashSet::new();
            for p in left_params.iter().chain(right_params.iter()) {
                if !seen.insert(p.clone()) {
                    return Err(self.err(&format!("duplicated argument name '{}'", p)));
                }
            }
        }
        if is_op {
            Ok(Instr::UserOpDef {
                name,
                op_left,
                op_right,
                left_params,
                right_params,
                body: Box::new(body),
            })
        } else {
            Ok(Instr::UserFnDef {
                name,
                namespace: None,
                left_params,
                right_params,
                body: Box::new(body),
            })
        }
    }

    /// Parse an optional parameter list. Accepts a parenthesised, `;`- or `,`-separated
    /// list. When `allow_bare` is true, a single unparenthesised symbol is also accepted
    /// (the right-parameter shorthand `∇ foo x { … }`). Left parameters are always
    /// parenthesised, so the caller passes `allow_bare=false` for them to avoid greedily
    /// consuming the function name as a parameter.
    fn parse_fn_params_opt(&mut self, allow_bare: bool) -> Result<Vec<String>, AplError> {
        let mut params = Vec::new();
        match self.peek() {
            Some(t) if matches!(t.token, Token::OpenParen) => {
                self.advance();
                loop {
                    self.skip_newlines();
                    match self.peek() {
                        Some(t) if matches!(t.token, Token::CloseParen) => {
                            self.advance();
                            break;
                        }
                        Some(t) if matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) => {
                            if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                                params.push(name.clone());
                            }
                            self.advance();
                        }
                        Some(t) if matches!(t.token, Token::ListSeparator) => {
                            self.advance();
                        }
                        _ => return Err(self.err("expected parameter name or ')'")),
                    }
                }
            }
            // Bare (unparenthesised) symbols: `∇ x foo y { … }` has the name *between*
            // the left and right param lists, so for the *left* list we must stop at the
            // first symbol that is *not* part of the params — but Kap's grammar lets the
            // name appear immediately. To disambiguate, only consume a bare left param
            // when it is explicitly parenthesised (handled above); bare symbols after the
            // name (right params) are consumed greedily up to the `{` body.
            Some(t) if allow_bare && matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) => {
                // One or more unparenthesised *right* parameters, e.g. `∇ foo x y { … }`
                // or `∇ x foo y { … }` (right params only — the left was empty/parenthesised).
                loop {
                    match self.peek() {
                        Some(t)
                            if matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) =>
                        {
                            if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                                params.push(name.clone());
                            }
                            self.advance();
                        }
                        _ => break,
                    }
                }
            }
            _ => {}
        }
        Ok(params)
    }

    /// apply := (fn term) | (term fn term)*  — monadic `f x` or dyadic `a f b` / trains.
    fn parse_apply(&mut self) -> Result<Instr, AplError> {
        // Control-flow keywords must also be recognised in *value* / assignment
        // positions (e.g. `x ← if (…) …`, `⊢ if (…) …`), not only at statement start.
        if let Some(instr) = self.parse_keyword_prefix()? {
            return Ok(instr);
        }
        // Unary minus on a non-literal operand: `-x`, `-⍵`, `(-padding)`. The lexer already
        // folds `-<digit>` into a single negative-number literal, so a bare `-` symbol here
        // (at the START of a value expression) is monadic negation. We handle it HERE —
        // at the value-expression entry point — NOT in `parse_primary`, because `parse_primary`
        // is also called to fetch a *dyadic operator* (e.g. the `-` in `3 - 4`), where the
        // same arm would wrongly hijack the operator into a unary Apply. (Must not fire for
        // dyadic subtraction, where `-` already has a left operand.)
        if let Some(Token::Literal(LiteralValue::Symbol { name, namespace })) = self.peek().map(|t| &t.token) {
            if name == "-" && namespace.is_none() {
                self.advance();
                let operand = self.parse_apply()?;
                return Ok(Instr::Apply {
                    fn_expr: Box::new(Instr::Symbol { name: "-".to_string(), namespace: None }),
                    left: None,
                    right: Box::new(operand),
                });
            }
        }
        let mut first = self.parse_primary()?;
        // A *value* followed by `primitive_op known_op` is a dyadic operator call where the
        // value is the operator's left DATA argument: `10 +foo 2` = `(+foo) applied to (10, 2)`.
        // Detect `value primitive known_op` and restructure into `Apply[OpCall{…}, left: value, right: …]`
        // so the value flows in as the operator's left data argument (not as the function operand).
        {
            let nxt = self.peek().map(|t| t.token.clone());
            let is_prim = matches!(&nxt, Some(Token::Literal(LiteralValue::Symbol { name, .. })) if Self::is_primitive_op(name));
            if is_prim {
                let save = self.pos;
                self.advance(); // peek past the primitive
                let after = self.peek().map(|t| t.token.clone());
                let is_op = matches!(
                    after,
                    Some(Token::Literal(LiteralValue::Symbol { name, .. })) if self.known_ops.iter().any(|n| n.as_str() == name)
                );
                self.pos = save;
                if is_op {
                    let data_left = first;
                    // Capture the function operand (the primitive, e.g. `+`) BEFORE consuming it.
                    let fn_operand = match self.peek().map(|t| t.token.clone()) {
                        Some(Token::Literal(LiteralValue::Symbol { name, namespace })) => {
                            Instr::Symbol { name: name.clone(), namespace: namespace.clone() }
                        }
                        _ => return Err(self.err("expected a function operand before the operator")),
                    };
                    self.advance(); // consume the function operand (primitive)
                    let opname = match self.peek().map(|t| t.token.clone()) {
                        Some(Token::Literal(LiteralValue::Symbol { name, .. })) => name.clone(),
                        _ => return Err(self.err("expected an operator name after function operand")),
                    };
                    self.advance();
                    self.skip_newlines();
                    // Optional right function operand (for a 2-arg operator, e.g. `-foo+`).
                    let right_fn = if self.next_is_function_token() && !self.at_statement_boundary() {
                        Some(Box::new(self.parse_primary()?))
                    } else {
                        None
                    };
                    let opcall = Instr::OpCall {
                        op: Box::new(Instr::Symbol { name: opname, namespace: None }),
                        left_fn: Box::new(fn_operand),
                        right_fn,
                    };
                    if self.at_statement_boundary() {
                        return Ok(Instr::Apply {
                            fn_expr: Box::new(opcall),
                            left: Some(Box::new(data_left)),
                            right: Box::new(Instr::Empty),
                        });
                    }
                    self.skip_newlines();
                    let right_operand = self.parse_apply()?;
                    return Ok(Instr::Apply {
                        fn_expr: Box::new(opcall),
                        left: Some(Box::new(data_left)),
                        right: Box::new(right_operand),
                    });
                }
            }
        }
        // Operator-call detection (`X op Y` / `X op`). If `first` is a function atom and the
        // next token is a *known user operator* (defined via `∇ (x op) a`), then `op` binds
        // `first` as a function operand. For a 2-arg operator (`∇ (x op y) a`) a second
        // function operand follows the operator name; for a 1-arg operator it does not.
        let is_first_fn = matches!(
            &first,
            Instr::Train { .. }
                | Instr::Lambda { .. }
                | Instr::Derived { .. }
                | Instr::Block { .. }
        ) || match &first {
            Instr::Symbol { name, namespace } => {
                let qual = match namespace {
                    Some(ns) => format!("{}:{}", ns, name),
                    None => name.clone(),
                };
                Self::is_primitive_op(&qual) || self.is_known_fn(name, namespace) || self.known_ops.iter().any(|n| n == name)
            }
            _ => false,
        };
        if is_first_fn {
            if let Some(Token::Literal(LiteralValue::Symbol { name: opname, .. })) = self.peek().map(|t| &t.token) {
                if self.known_ops.iter().any(|n| n == opname) {
                    let opname = opname.clone();
                    self.advance();
                    self.skip_newlines();
                    // Right function operand (for a 2-arg operator). If the next token is a
                    // function atom (not a data operand), it is the right function operand.
                    let right_fn = if self.next_is_function_token() && !self.at_statement_boundary() {
                        Some(Box::new(self.parse_primary()?))
                    } else {
                        None
                    };
                    // B1 (code_analysis_03): an operator application must carry a function
                    // operand OR a trailing data argument. `⟨known_fn⟩ ⟨known_op⟩` with
                    // neither (e.g. `typeof ⌸`) is incomplete and would otherwise reach eval
                    // and run the operator body / panic. Kotlin's `InvalidOperatorArgument`
                    // rejects this at parse time with the text below; mirror it.
                    if right_fn.is_none() && self.at_statement_boundary() {
                        return Err(self.err(&format!("Operator without left function: {}", opname)));
                    }
                    // Adopt the combined operator as `first` and fall through to the normal
                    // application logic below, which will consume any trailing data argument
                    // (e.g. the `3` in `-foo+ 3`). Returning here would leave it unconsumed.
                    first = Instr::OpCall {
                        op: Box::new(Instr::Symbol { name: opname, namespace: None }),
                        left_fn: Box::new(first),
                        right_fn,
                    };
                }
            }
        }
        // Compose `∘` / reverse-compose `⍛` after a function atom (legacy
        // parse_apply path — bind_operators_kotlin handles these in the
        // Kotlin path). `⍛` binds `first` as left, parses ONE fn as right,
        // then chains the result as the new `first` for further application.
        if Self::is_function_expr(&first) {
            match self.peek().map(|t| t.token.clone()) {
                Some(Token::ComposeToken) => {
                    self.advance();
                    self.skip_newlines();
                    let r = self.parse_function_atom()?;
                    first = Instr::Train {
                        funcs: vec![first, r],
                        reverse: false,
                        compose: true,
                    };
                }
                Some(Token::ReverseComposeToken) => {
                    self.advance();
                    self.skip_newlines();
                    let r = self.parse_function_atom()?;
                    first = Instr::Train {
                        funcs: vec![first, r],
                        reverse: true,
                        compose: true,
                    };
                    // Trailing fn after compose: chain as 2-train atop
                    // (e.g. `(…)⍛⊇ ∧` → `∧` chains on the compose result).
                    let trailing = match self.peek().map(|t| &t.token) {
                        Some(Token::Literal(LiteralValue::Symbol { name, .. })) => Self::is_primitive_op(name),
                        Some(Token::OpenParen) | Some(Token::LambdaToken) | Some(Token::ApplyToken) | Some(Token::LeftForkToken) => true,
                        _ => false,
                    };
                    if trailing {
                        let next = self.parse_function_atom()?;
                        first = Instr::Train {
                            funcs: vec![first, next],
                            reverse: false,
                            compose: false,
                        };
                    }
                }
                // Inner/outer product after a function atom (legacy parse_apply
                // path — mirrors the bind_operators_kotlin arm). `∘∙f` arrives
                // here as ComposeToken then this arm with first=Symbol("∘").
                Some(Token::Literal(LiteralValue::Symbol { ref name, .. }))
                    if name == "∙" && Self::is_function_expr(&first) =>
                {
                    let is_null_left =
                        matches!(&first, Instr::Symbol { name, .. } if name == "∘");
                    self.advance();
                    self.skip_newlines();
                    let r = self.parse_function_atom()?;
                    first = Instr::InnerProduct {
                        left_fn: if is_null_left { None } else { Some(Box::new(first)) },
                        right_fn: Box::new(r),
                    };
                }
                _ => {}
            }
        }
        // A function atom followed by an adverb (`(f g)¨ xs`, `λ…/ ys`): bind the
        // derived function `atom op` and apply it to the following data — same
        // semantics as the symbol form (`dbl¨`) handled below, which a paren-atom
        // first cannot reach (it is not a Symbol, so that arm never fires).
        // MUST run before the monadic-apply block below, which would otherwise
        // treat the adverb glyph as the data operand.
        if matches!(
            &first,
            Instr::Train { .. } | Instr::Lambda { .. } | Instr::DynamicRef { .. }
        ) && matches!(
            self.peek().map(|t| &t.token),
            Some(Token::Literal(LiteralValue::Symbol { ref name, .. }))
                if Self::is_adverb(name)
        )
        {
            let op = self.parse_primary()?; // consume the adverb symbol
            let derived = Instr::Derived {
                func: Box::new(first),
                op: Box::new(op),
            };
            let operand = self.parse_apply()?;
            return Ok(Instr::Apply {
                fn_expr: Box::new(derived),
                left: None,
                right: Box::new(operand),
            });
        }
        // A leading *function atom* that cannot be a value — a train, a lambda, or a
        // derived (adverb) operator — is a monadic/dyadic application `fn x` / `x fn y`
        // (e.g. `(f g) y`, `λ(x)x*2 5`, `+/ 1 2 3`). Bare symbols are handled below by the
        // original heuristic, since a symbol may also be a *value* (variable) used as a
        // dyadic left operand (`x + 1`).
        if matches!(
            &first,
            Instr::Train { .. }
                | Instr::Lambda { .. }
                | Instr::Derived { .. }
                | Instr::Block { .. }
                | Instr::OpCall { .. }
                | Instr::DynamicRef { .. }
        ) {
            // A statement boundary ends the expression: the function stands alone.
            if self.at_statement_boundary() {
                return Ok(first);
            }
            self.skip_newlines();
            if self.at_statement_boundary() {
                return Ok(first);
            }
            let operand = self.parse_apply()?;
            return Ok(Instr::Apply {
                fn_expr: Box::new(first),
                left: None,
                right: Box::new(operand),
            });
        }
        // A function atom followed by an adverb (`(f g)¨ xs`, `λ…/ ys`): bind the
        // derived function `atom op` and apply it to the following data — same
        // semantics as the symbol form (`dbl¨`) handled below, which a paren-atom
        // first cannot reach (it is not a Symbol, so that arm never fires).
        if matches!(
            &first,
            Instr::Train { .. } | Instr::Lambda { .. } | Instr::DynamicRef { .. }
        ) && matches!(
            self.peek().map(|t| &t.token),
            Some(Token::Literal(LiteralValue::Symbol { ref name, .. }))
                if Self::is_adverb(name)
        )
        {
            let op = self.parse_primary()?; // consume the adverb symbol
            let derived = Instr::Derived {
                func: Box::new(first),
                op: Box::new(op),
            };
            let operand = self.parse_apply()?;
            return Ok(Instr::Apply {
                fn_expr: Box::new(derived),
                left: None,
                right: Box::new(operand),
            });
        }
        // A leading symbol is parsed as a MONADIC application `f x` when:
        //   * it is a primitive AND the next token is a plain operand, or another primitive
        //     (train: `⊃ ⍳5` = `⊃(⍳5)`, `≢ ⍳5` = `≢(⍳5)`); or
        //   * it is a *user* symbol (variable/function) AND the next token is a plain operand
        //     (`f 5`). When the next token is an OPERATOR (e.g. `x + 1`), a user symbol is NOT
        //     monadic — it is the LEFT operand of the dyadic operator, so we fall through.
        // Valence is ultimately resolved at runtime; this is a syntactic heuristic.
        if let Instr::Symbol { name, namespace } = &first {
            let qual = match namespace {
                Some(ns) => format!("{}:{}", ns, name),
                None => name.clone(),
            };
            let is_prim = Self::is_primitive_op(&qual);
            // A leading symbol is a *function* (and thus eligible for monadic apply `f x`)
            // only if it is a primitive or a known (user/native) function. Otherwise it is a
            // value and must strand (`a c` -> (a c)) rather than apply (`a(c)`).
            let is_known = self.is_known_fn(name, namespace);
            // A top-level newline/separator ends the statement: `a ← 3` followed by a
            // newline is NOT a monadic apply of whatever comes next. Check BEFORE
            // skipping newlines so we see the boundary, not the next statement's token.
            if self.at_statement_boundary() {
                return Ok(first);
            }
            self.skip_newlines();
            if self.at_statement_boundary() {
                return Ok(first);
            }
            let next = self.peek().map(|t| t.token.clone());
            // `f[axis] …` where f is a primitive that supports an explicit axis
            // (`⌽[0] v` in the right-arg position of another fn, e.g. `≢ ⌽[0] 1 2 3`).
            // Without this, `[` counts as a VALUE operand below and `[0]` becomes a
            // list literal stranded into the data (port produced `≢ ⌽[0] 1 2 3` = 4).
            // Mirrors bind_operators_kotlin's axis arm / Kotlin parseAxis (:1321).
            if is_prim
                && matches!(
                    name.as_str(),
                    "+" | "-" | "×" | "÷" | "*" | "," | "⍪" | "⌽" | "⊖"
                )
                && matches!(next, Some(Token::OpenBracket))
            {
                self.advance(); // consume [
                let axis = self.parse_apply()?;
                self.expect(Token::CloseBracket, "expected ] after axis specifier")?;
                let fn_expr = Instr::AxisApplied {
                    func: Box::new(first),
                    axis: Box::new(axis),
                };
                // Optional trailing data argument (`≢ ⌽[0] 1 2 3` ⇒ right = (1 2 3)).
                let has_data = !self.at_statement_boundary()
                    && !matches!(
                        self.peek().map(|t| &t.token),
                        Some(Token::CloseParen) | Some(Token::CloseBracket)
                    );
                if has_data {
                    self.skip_newlines();
                    let data = self.parse_apply()?;
                    return Ok(Instr::Apply {
                        fn_expr: Box::new(fn_expr),
                        left: None,
                        right: Box::new(data),
                    });
                }
                return Ok(fn_expr);
            }
            // Fork postfix `a « b » c` (e.g. `⊢ « ⊣ » ,`): when a function atom is
            // immediately followed by `«`, parse the right-fork instead of treating the
            // following token as an operand.
            if matches!(next, Some(Token::LeftForkToken)) {
                self.advance();
                let b = self.parse_function_atom()?;
                self.skip_newlines();
                self.expect(Token::RightForkToken, "expected » in fork")?;
                self.skip_newlines();
                let c = self.parse_function_atom()?;
                return Ok(Instr::Train {
                    funcs: vec![first, b, c],
                    reverse: false,
                    compose: false,
                });
            }
            // A *value* operand (so a leading function applies to it monadically) is a
            // literal, or a non-primitive / non-adverb symbol (a variable or another
            // user function's name, e.g. `fact N-1` → `fact(N-1)`, `foo x` → `foo(x)`).
            // A primitive-op symbol (`+`, `≤`, …) is NOT an operand: `N≤1` is dyadic, not
            // `N` applied monadically to `≤1`.
            let next_is_operand_not_fn = match &next {
                Some(t) => {
                    !matches!(t, Token::EndOfFile)
                        && !matches!(t, Token::Newline)
                        && !matches!(t, Token::StatementSeparator)
                        && !matches!(t, Token::CloseParen)
                        && !matches!(t, Token::CloseBracket)
                        && !matches!(t, Token::ListSeparator)
                        // A primitive-op or adverb symbol is an *operator*, not a value
                        // operand: `N≤1` is dyadic (`≤` binds), `+/` is reduce — neither is
                        // a monadic application of the leading symbol to that symbol. Any
                        // other symbol (a variable or a user-function name such as `N`,
                        // `x`, `fact`) IS a value operand, so a leading function applies to
                        // it monadically: `fact N-1` → `fact(N-1)`, `foo x` → `foo(x)`.
                        && !matches!(
                            t,
                            Token::Literal(LiteralValue::Symbol { name, .. })
                                if Self::is_primitive_op(name) || Self::is_adverb(name)
                                    || name == "⍤"
                        )
                        // `int:proto` is a native VALUE-OP, never a data operand:
                        // `↑ int:proto 5` must bind proto to ↑, not feed it as ⍵.
                        && !matches!(
                            t,
                            Token::Literal(LiteralValue::Symbol { name, namespace })
                                if namespace.as_deref() == Some("int") && name == "proto"
                        )
                }
                None => false,
            };
            let next_is_primitive = match &next {
                Some(Token::Literal(LiteralValue::Symbol { name: nn, .. })) => Self::is_primitive_op(nn),
                _ => false,
            };
            let next_is_adverb = match &next {
                Some(Token::Literal(LiteralValue::Symbol { name: nn, .. })) => Self::is_adverb(nn),
                _ => false,
            };
            // `⍤` is a value-right-arg operator (rank): fn ⍤ <value expr>.
            let next_is_value_op = match &next {
                Some(Token::Literal(LiteralValue::Symbol { name: nn, .. })) => nn == "⍤",
                _ => false,
            };
            // A namespaced native value-op (`int:proto`) after a function: the
            // function operand binds it as `ValueOp{fn, "int:proto", operand}`.
            let next_is_native_value_op = match &next {
                Some(Token::Literal(LiteralValue::Symbol { name, namespace })) => {
                    namespace.as_deref() == Some("int") && name == "proto"
                }
                _ => false,
            };
            let do_monadic = if is_prim {
                // A leading primitive followed by an adverb (`+/`, `×¨`, `⍟\\`) is the
                // *function-then-operator* form, NOT monadic application: it is a derived
                // function `func op` applied to the following data.
                (next_is_operand_not_fn || next_is_primitive)
                    && !next_is_adverb
                    && !next_is_native_value_op
            } else {
                // A leading user symbol applies monadically only when it names a function.
                // A leading *value* symbol does not apply — it strands with the next operand.
                is_known && next_is_operand_not_fn
            };
            if do_monadic {
                let operand = self.parse_apply()?;
                return Ok(Instr::Apply {
                    fn_expr: Box::new(first),
                    left: None,
                    right: Box::new(operand),
                });
            }
            // Function-then-adverb: `f/` `f¨` etc. Build a derived function `func op`
            // and apply it to the following data operand. This fires for primitive ops
            // (`+/`, `×¨`) AND for known user-defined functions (`dbl¨`, `fact/`) — a
            // named function is a valid left operand of an adverb, so `dbl¨ 1 2 3`
            // must bind `Derived{dbl, ¨}` rather than applying `¨` to `dbl`.
            if (is_prim || is_known) && next_is_adverb {
                let op = self.parse_primary()?; // consume the adverb symbol
                // Optional explicit axis ON THE DERIVED FUNCTION: `+/[0] x`
                // binds [0] to the reduction (Kotlin ReduceAPLOperator axis),
                // NOT as a list literal stranded into the data.
                let func_boxed =
                    if matches!(self.peek().map(|t| &t.token), Some(Token::OpenBracket)) {
                        self.advance();
                        let ax = self.parse_apply()?;
                        self.expect(Token::CloseBracket, "expected ] after axis specifier")?;
                        Instr::AxisApplied {
                            func: Box::new(first),
                            axis: Box::new(ax),
                        }
                    } else {
                        first
                    };
                    // Fork postfix on a derived function: `+/«÷»≢` — the Derived is the
                    // fork's LEFT member. Parse middle + right and return the 3-train
                    // (mirrors bind_operators_kotlin's fork arm / Kotlin parseOperator).
                    if matches!(self.peek().map(|t| &t.token), Some(Token::LeftForkToken)) {
                        self.advance(); // consume «
                        self.skip_newlines();
                        let b = self.parse_function_atom()?;
                        self.skip_newlines();
                        self.expect(Token::RightForkToken, "expected » in fork")?;
                        self.skip_newlines();
                        let c = self.parse_function_atom()?;
                        return Ok(Instr::Train {
                            funcs: vec![
                                Instr::Derived {
                                    func: Box::new(func_boxed),
                                    op: Box::new(op),
                                },
                                b,
                                c,
                            ],
                            reverse: false,
                            compose: false,
                        });
                    }
                    let data = self.parse_apply()?;
                return Ok(Instr::Apply {
                    fn_expr: Box::new(Instr::Derived {
                        func: Box::new(func_boxed),
                        op: Box::new(op),
                    }),
                    left: None,
                    right: Box::new(data),
                });
            }
            // Value-right-arg operator: `f⍤rank` (Kotlin APLOperatorValueRightArg,
            // engine.kt:494). Unlike an adverb, the right operand is a VALUE expression
            // parsed with parse_apply (so `⊥⍤1` binds rank 1, and `f⍤0 1` binds the
            // vector `0 1`). The binding itself is a function: it consumes the trailing
            // data argument like any derived function.
            if (is_prim || is_known) && next_is_value_op {
                self.advance(); // consume ⍤
                // Rank spec: parse ONE primary, then absorb following *numeric literals*
                // into a strand (`⍤0 1` = vector spec). We must NOT use parse_apply here:
                // it strands across the data argument (`⍤1 x` would swallow `x`), whereas
                // Kotlin's APLOperatorValueRightArg.parseAndCombineFunctions uses
                // parseValue which stops before the trailing function application.
                let mut spec = self.parse_primary()?;
                loop {
                    let more = matches!(
                        self.peek().map(|t| &t.token),
                        Some(Token::Literal(LiteralValue::Number(_)))
                    );
                    if !more {
                        break;
                    }
                    let elem = self.parse_primary()?;
                    if let Instr::Array { elements } = &mut spec {
                        elements.push(elem);
                    } else {
                        spec = Instr::Array { elements: vec![spec, elem] };
                    }
                }
                // The bound operator is itself a function: consume the trailing data
                // operand if one is present (statement boundary / EOF → no data yet;
                // the binding then evaluates as a stray function, mirroring `+/`).
                let data = if !self.at_statement_boundary()
                    && self.peek().map(|t| &t.token) != Some(&Token::CloseParen)
                    && self.peek().map(|t| &t.token) != Some(&Token::CloseBracket)
                {
                    self.parse_apply()?
                } else {
                    Instr::Empty
                };
                // `f⍤k` (or `f⍤k 0 1`) is a *derived function*. It must NOT be wrapped in an
                // `Apply` with `first` (the function `f`) as a data operand — that would
                // turn `256 (⊥⍤1) M` into `(⊥⍤1)(left=⊥)` and lose the real left data `256`.
                // Instead return the bare `ValueOp` (a function value); the surrounding
                // parse loop then binds any preceding/following data args normally.
                let value_op = Instr::ValueOp {
                    func: Box::new(first),
                    op_name: "⍤".to_string(),
                    operand: Box::new(spec),
                };
                // If a trailing data argument was present on this same statement, apply it
                // now as the right operand (mirrors how `+/ 1 2 3` binds its data).
                if !matches!(data, Instr::Empty) {
                    return Ok(Instr::Apply {
                        fn_expr: Box::new(value_op),
                        left: None,
                        right: Box::new(data),
                    });
                }
                return Ok(value_op);
            }
            // Native value-right-arg operator: `f int:proto v` (Kotlin ProtoOp,
            // proto.kt — engine.kt:505). Same binding shape as `⍤`: the operator
            // takes ONE value expression on its right and yields a derived function.
            if (is_prim || is_known) && next_is_native_value_op {
                self.advance(); // consume int:proto
                let spec = self.parse_apply()?;
                let data = if !self.at_statement_boundary()
                    && self.peek().map(|t| &t.token) != Some(&Token::CloseParen)
                    && self.peek().map(|t| &t.token) != Some(&Token::CloseBracket)
                {
                    self.parse_apply()?
                } else {
                    Instr::Empty
                };
                let value_op = Instr::ValueOp {
                    func: Box::new(first),
                    op_name: "int:proto".to_string(),
                    operand: Box::new(spec),
                };
                if !matches!(data, Instr::Empty) {
                    return Ok(Instr::Apply {
                        fn_expr: Box::new(value_op),
                        left: None,
                        right: Box::new(data),
                    });
                }
                return Ok(value_op);
            }
            // Fall through: treat `first` as the LEFT operand of a following dyadic op.
        }
        // Dyadic `L f R` where `first` is a *value* and the next token is a *function atom*
        // (`{…}`, `λ`, a train, a parenthesised function, or a known/primitive/adverb
        // symbol): bind `first` as the left argument and apply the function to the
        // following right operand. This mirrors Kap's parser.kt `processFn`/`makeResultList`
        // (the left operand is accumulated *before* the function is seen).
        //   `4 foo 2`        -> ⍺=4, ⍵=2
        //   `(10;11) foo (1;2)` -> dyadic, vector args
        //   `4 {⍺+⍵} 3`      -> ⍺=4, ⍵=3
        // The function is parsed as a *function expression* (not a value expression) so it
        // does NOT greedily consume the right operand.
        if !self.at_statement_boundary() {
            self.skip_newlines();
            if !self.at_statement_boundary() {
                // A parenthesised group is only a *function* in `L f R` position when its
                // contents form a function train (e.g. `(+)`, `(×-)`, `(-⍛+)`). A value
                // group like `(x y)` / `(4 5)` is a nested-array strand element, NOT a
                // function — treating it as one mis-parses e.g. `(3 (4 5))` and errors
                // "expected a function in train". So gate the OpenParen case on its being a
                // paren operator; every other function token is unaffected.
                let paren_group_is_fn = match self.peek().map(|t| &t.token) {
                    Some(Token::OpenParen) => self.next_is_paren_operator(),
                    // Non-paren tokens: defer to the generic function-token check.
                    _ => self.next_is_function_token(),
                };
                if paren_group_is_fn {
                    let func = self.parse_function_expr()?;
                    // Axis specifier: `f[axis]` (e.g. `+[0]`, `,[0.5]`). Kotlin's
                    // `parseOperator` reads an optional `[axis]` *before* the right
                    // operand and wraps the function in `AxisValAssignedFunctionDirect`.
                    // We must parse it here, BEFORE the right operand, because
                    // `parse_apply` would otherwise greedily consume `[0] …` as a
                    // bracket-index on the right argument.
                    // Applies to the scalar-arithmetic operators (`+ - × ÷ *`, the only
                    // ones with an axis path in `num2_axis`) and the catenation operators
                    // (`,`/`⍪`, which support `,[0.5]` laminate / `,[0]` concat). These
                    // are precisely the functions with an axis path in the evaluator.
                    // Anything else must NOT swallow a following `[` — the port treats
                    // `[3;4]` as a list literal (a known port-leniency, asserted by
                    // `eval_catenate`).
                    let mut fn_expr = func;
                    let axis_ok = matches!(
                        &fn_expr,
                        Instr::Symbol { name, .. }
                            if matches!(name.as_str(), "+" | "-" | "×" | "÷" | "*" | "," | "⍪" | "⌽" | "⊖")
                    );
                    if axis_ok
                        && matches!(self.peek().map(|t| &t.token), Some(Token::OpenBracket))
                    {
                        self.advance();
                        let axis = self.parse_apply()?;
                        self.expect(Token::CloseBracket, "expected ] after axis specifier")?;
                        fn_expr = Instr::AxisApplied {
                            func: Box::new(fn_expr),
                            axis: Box::new(axis),
                        };
                    }
                    let right = self.parse_apply()?;
                    return Ok(Instr::Apply {
                        fn_expr: Box::new(fn_expr),
                        left: Some(Box::new(first)),
                        right: Box::new(right),
                    });
                }
            }
        }
        // Stranding: consecutive operands with no operator between them form a vector.
        // e.g. `1 2 3` -> [1 2 3]. Collect into `left` so a following dyadic operator
        // (e.g. `1 2 3 + 10`) sees the strand as its left argument. A `( OP )` parenthesised
        // operator is NOT an operand here (it is a derived function, e.g. `(+)`).
        //
        // NOTE: a parenthesised group like `(1 2 3)` already parses to an `Instr::Array`.
        // That array must remain a SINGLE element of the strand (`((1 2 3) 4 5)` ->
        // vector of 3 elements: the nested array `(1 2 3)`, `4`, `5`), NOT be flattened
        // into it. So we accumulate strand elements in a Vec and only build the result
        // `Instr::Array` once at the end — never pushing *into* an existing array element.
        // A traditional function definition (`∇`) may only begin a statement. If one
        // appears here (after a value/expression in the same statement, e.g. `1 ∇ foo (x) {…}`)
        // it is a syntax error. Note: `∇` at the *start* of a statement is handled earlier
        // by `parse_expr`, which dispatches to `parse_fn_def` before reaching this point.
        if matches!(self.peek().map(|t| &t.token), Some(Token::FnDefSym)) {
            return Err(self.err("function definition (∇) must start a new statement"));
        }
        let mut strand: Option<Vec<Instr>> = None;
        let first = self.parse_index_suffix(first)?;
        let mut left = first;
        loop {
            // Stop at a statement boundary: newlines separate statements at the top
            // level, so `1 2 3` does not strand across into the next line's tokens.
            // Check BEFORE skipping newlines so we see the boundary.
            // A `;` (ListSeparator) also separates — Kap uses it as a vector/argument
            // separator, so `10;11` strands into `(10 11)` rather than being one unit.
            if self.at_statement_boundary() || matches!(self.peek().map(|t| &t.token), Some(Token::ListSeparator)) {
                break;
            }
            // Short-circuit boolean `and` / `or` (Kotlin `AndToken`/`OrToken`) sit at a
            // precedence BELOW the value/strand level, so they must NOT be absorbed into a
            // strand. Break out here (without consuming the keyword) so the caller
            // (`parse_assign`'s boolean-op loop) wraps the inner expression in an
            // `Instr::BooleanOp`. (Without this, `1 and 2` would strand into `[1, and, 2]`.)
            if matches!(
                self.peek().map(|t| &t.token),
                Some(Token::Literal(LiteralValue::Symbol { name, namespace }))
                    if namespace.is_none() && (name == "and" || name == "or")
            ) {
                break;
            }
            self.skip_newlines();
            if self.at_statement_boundary() || matches!(self.peek().map(|t| &t.token), Some(Token::ListSeparator)) {
                break;
            }
            let paren_op = self.next_is_paren_operator();
            match self.peek() {
                Some(t) if self.is_strand_operand(&t.token) && !paren_op => {
                    let mut operand = self.parse_primary()?;
                    // Selection also binds to this operand.
                    operand = self.parse_index_suffix(operand)?;
                    match strand {
                        None => strand = Some(vec![left.clone(), operand]),
                        Some(ref mut elems) => elems.push(operand),
                    }
                }
                _ => break,
            }
        }
        // If we accumulated >=2 strand elements, the result is a vector; the original
        // `left` becomes element 0. A single operand (no strand) returns unchanged.
        if let Some(elems) = strand {
            if elems.len() >= 2 {
                left = Instr::Array { elements: elems };
            }
        }
        // Dyadic form: a f b (f c ...). `left` may be a value or a strand. The operator may
        // be a symbol, a parenthesised operator `(+)`, an open-bracket, a lambda, or comma.
        loop {
            if self.at_statement_boundary() || matches!(self.peek().map(|t| &t.token), Some(Token::ListSeparator)) {
                break;
            }
            self.skip_newlines();
            if self.at_statement_boundary() || matches!(self.peek().map(|t| &t.token), Some(Token::ListSeparator)) {
                break;
            }
            let paren_op = self.next_is_paren_operator();
            // Outer product `A ∘∙f B` (ComposeToken + Symbol("∙")): a dyadic
            // application of the null-left inner-product derived function.
            // Must be checked BEFORE the strand loop breaks / dyadic is_operator
            // test, neither of which knows the ComposeToken+∙ shape.
            if let Some(t) = self.peek() {
                if matches!(t.token, Token::ComposeToken) {
                    if let Some(t2) = self.peek_at(1).map(|t| &t.token) {
                        if matches!(t2, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "∙")
                        {
                            self.advance(); // consume ∘
                            self.advance(); // consume ∙
                            self.skip_newlines();
                            let rfn = self.parse_function_atom()?;
                            let right = self.parse_apply()?;
                            left = Instr::Apply {
                                fn_expr: Box::new(Instr::InnerProduct {
                                    left_fn: None,
                                    right_fn: Box::new(rfn),
                                }),
                                left: Some(Box::new(left)),
                                right: Box::new(right),
                            };
                            continue;
                        }
                    }
                }
            }
            let is_operator = match self.peek() {
                Some(t) => match &t.token {
                    // A bare symbol is an operator only if it's a primitive or a
                    // known (user/native) function. Otherwise it's a value and must
                    // strand (`a c`) rather than apply (`a c` -> a(c)).
                    Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                        Self::is_primitive_op(name) || self.is_known_fn(name, namespace)
                    }
                    Token::OpenParen => paren_op,
                    Token::OpenBracket => true,
                    Token::LambdaToken => true,
                    // A `{ … }` lambda block is also a function atom in operator
                    // position (`1 2 {≢⍵}⌸ 3 4`): without this the dyadic loop
                    // breaks and the trailing adverb is parsed as a bare symbol.
                    Token::OpenBrace => true,
                    _ => false,
                },
                None => false,
            };
            if !is_operator {
                break;
            }
            let mut fn_expr = self.parse_primary()?; // the operator (symbol or parenthesised)
            self.skip_newlines();
            // Detect `func adverb` immediately after the operator (e.g. `×¨` in
            // `2 ×¨ 3 4 5`): the operator is a function-primitive and the very next
            // token is an adverb. Bind them into a derived function and apply the
            // accumulated `left` data plus the following data to it (dyadic each).
            let op_is_func = match &fn_expr {
                Instr::Symbol { name, namespace } => {
                    // A primitive operator, OR a known user-defined function, can be the
                    // left operand of an adverb: `×¨` and `dbl¨` both bind into a Derived.
                    // (Previously only primitives were handled, so `dbl¨ 1 2 3` fell through
                    // to a plain apply with `¨` as the function → "unknown function: ¨".)
                    Self::is_primitive_op(name) || self.is_known_fn(name, namespace)
                }
                // A lambda can also be the adverb's function operand:
                // `{≢⍵}⌸ vals` (Key) binds the lambda into a Derived.
                Instr::Lambda { .. } | Instr::Block { .. } => true,
                _ => false,
            };
            if op_is_func {
                let next_is_adv = match self.peek() {
                    Some(t) => match &t.token {
                        Token::Literal(LiteralValue::Symbol { name: nn, .. }) => Self::is_adverb(nn),
                        _ => false,
                    },
                    None => false,
                };
                if next_is_adv {
                    let adv = self.parse_primary()?; // consume the adverb
                    self.skip_newlines();
                    let right = self.parse_apply()?;
                    left = Instr::Apply {
                        fn_expr: Box::new(Instr::Derived {
                            func: Box::new(fn_expr),
                            op: Box::new(adv),
                        }),
                        left: Some(Box::new(left)),
                        right: Box::new(right),
                    };
                    continue;
                }
            }
            // Axis specifier `f[axis]` (e.g. `1 2 3 +[0] 4 5 6`). The single-value
            // `L f R` path above handles this, but a STRANDED left operand (`1 2 3`)
            // reaches the dyadic loop here and would otherwise greedily consume `[0] …`
            // as a bracket-index on the right argument (producing "different length").
            // Read the optional `[axis]` BEFORE the right operand and wrap the function
            // in an `AxisApplied` node, mirroring Kotlin's `AxisValAssignedFunctionDirect`.
            // GATED to the scalar-arithmetic operators `num2_axis` supports: only those
            // five have an axis path in the evaluator. Anything else (e.g. `,` catenate)
            // must NOT swallow a following `[` — the port treats `[3;4]` as a list
            // literal (a known port-leniency vs Kotlin, asserted by `eval_catenate`).
            let op_is_scalar_arith = matches!(
                &fn_expr,
                Instr::Symbol { name, .. }
                    if matches!(name.as_str(), "+" | "-" | "×" | "÷" | "*" | "," | "⍪")
            );
            if op_is_scalar_arith
                && matches!(self.peek().map(|t| &t.token), Some(Token::OpenBracket))
            {
                self.advance();
                let axis = self.parse_apply()?;
                self.expect(Token::CloseBracket, "expected ] after axis specifier")?;
                fn_expr = Instr::AxisApplied {
                    func: Box::new(fn_expr),
                    axis: Box::new(axis),
                };
            }
            let right = self.parse_apply()?;
            left = Instr::Apply {
                fn_expr: Box::new(fn_expr),
                left: Some(Box::new(left)),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// Whether the upcoming tokens form a *parenthesised operator*: `( OP )` where OP is
    /// a single function symbol. This is Kap's "derived function" syntax, e.g. `(+)`,
    /// `(×)`. Such a group is NOT a plain operand — in dyadic position it is the operator
    /// (`2 (+) 3` = `5`), and it must not be swallowed into a strand (`(1+2)(3+4)` is a
    /// strand of two *groups*, not `(+)`).
    /// Whether the next token is a parenthesised *function* — a `( ... )` group whose
    /// content is a function atom or a train of function atoms (e.g. `(+)`, `(-,)`,
    /// Whether the next token is a parenthesised *function* — a `( ... )` group whose
    /// content is a function expression, a train, or a left-bind `[value, fn]`
    /// (e.g. `(+)`, `(-,)`, `(×∘-)`, `(f g)`, `«a b»`, `(10+)`). Used so `a (fn) b` parses
    /// as a dyadic application. A bare value group like `(10)` is also accepted here
    /// (it is not valid Kap, and would error at evaluation).
    fn next_is_paren_operator(&mut self) -> bool {
        let saved = self.pos;
        let mut ok = false;
        if let Some(t) = self.peek() {
            if !matches!(t.token, Token::OpenParen) {
                return false;
            }
            self.pos += 1;
            self.skip_newlines();
            // Classify each member as a *function* atom or a *value* atom. A paren group is a
            // dyadic operator only if its content is a function train — a single function, a
            // fork/train of >=2 functions, or a 2-member left-bind [value fn]. A naked value
            // strand like `(3 4 5)` or a single value `(10)` is NOT a function, so it must be
            // treated as a plain data group (a strand element), never as an operator.
            #[derive(PartialEq, Clone, Copy)]
            enum Kind {
                Func,
                Value,
            }
            let mut kinds: Vec<Kind> = Vec::new();
            let mut valid = true;
            let mut saw_rank = false;
            loop {
                match self.peek().map(|t| &t.token) {
                    Some(Token::CloseParen) => break,
                    Some(Token::Literal(LiteralValue::Number(_)))
                    | Some(Token::Literal(LiteralValue::Char(_)))
                    | Some(Token::Literal(LiteralValue::Str(_))) => {
                        kinds.push(Kind::Value);
                        self.advance();
                        self.skip_newlines();
                    }
                    Some(Token::Literal(LiteralValue::Symbol { name, namespace })) => {
                        let is_fn = Self::is_primitive_op(name)
                            || self.is_known_fn(name, namespace)
                            || Self::is_adverb(name)
                            || matches!(name.as_str(), "⊢" | "⊣" | "⍤");
                        self.advance();
                        self.skip_newlines();
                        // A function followed by the rank glyph (`(⊥⍤1)`) absorbs the
                        // value-op operand — the numbers are the rank SPEC of this one
                        // derived function, not separate Value members.
                        if is_fn && self.peek().map(|t| &t.token)
                            == Some(&Token::Literal(LiteralValue::Symbol {
                                name: "⍤".to_string(),
                                namespace: None,
                            }))
                        {
                            saw_rank = true;
                            self.advance(); // consume ⍤
                            while matches!(
                                self.peek().map(|t| &t.token),
                                Some(Token::Literal(LiteralValue::Number(_)))
                            ) {
                                self.advance();
                                self.skip_newlines();
                            }
                        }
                        if is_fn {
                            kinds.push(Kind::Func);
                        } else {
                            kinds.push(Kind::Value);
                        }
                    }
                    Some(Token::Literal(LiteralValue::SymbolValue { .. })) => {
                        // A symbol *value* (`'abc`) is always a value member of a
                        // paren-train — it is data, never a function.
                        kinds.push(Kind::Value);
                        self.advance();
                        self.skip_newlines();
                    }
                    Some(Token::LambdaToken)
                    | Some(Token::ComposeToken)
                    | Some(Token::ReverseComposeToken)
                    | Some(Token::LeftForkToken)
                    | Some(Token::RightForkToken)
                    | Some(Token::OpenParen) => {
                        kinds.push(Kind::Func);
                        self.advance();
                        self.skip_newlines();
                    }
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if valid {
                let n = kinds.len();
                let all_func = kinds.iter().all(|k| *k == Kind::Func);
                let single_func = n == 1 && kinds[0] == Kind::Func;
                let left_bind = n == 2 && kinds[0] == Kind::Value && kinds[1] == Kind::Func;
                ok = single_func || (n >= 2 && all_func) || left_bind || saw_rank;
            }
        }
        self.pos = saved;
        ok
    }

    /// Whether a token can begin a strand element (an operand, not an operator/separator).
    fn is_strand_operand(&self, tok: &Token) -> bool {
        match tok {
            Token::Literal(LiteralValue::Number(_))
            | Token::Literal(LiteralValue::Char(_))
            | Token::Literal(LiteralValue::Str(_))
            | Token::OpenParen
            | Token::OpenBracket
            | Token::APLNullSym => true,
            // A symbol LITERAL (`'a`, `'ns:a`) is always data (Kotlin parser.kt:1003
            // LiteralSymbol) — strand it like any operand so `'a 'b 'c` is a
            // 3-strand, and the QuotePrefix token itself opens a literal.
            Token::QuotePrefix => true,
            Token::Literal(LiteralValue::SymbolValue { .. }) => true,
            // NOTE: LambdaToken deliberately NOT a strand operand — a lambda is a
            // FUNCTION and applies to the preceding strand (`1 2 2 {≢⍵}⌸ v`), it
            // never joins it as data.
            // A bare symbol is a strand operand (so `a c` -> (a c)) UNLESS it names a
            // function — a function symbol in strand position is applied instead.
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                let qual = match namespace {
                    Some(ns) => format!("{}:{}", ns, name),
                    None => name.clone(),
                };
                !Self::is_primitive_op(&qual) && !self.is_known_fn(name, namespace)
            }
            _ => false,
        }
    }

    /// Kap primitive function/operator names (single-glyph). Used to decide monadic vs
    /// dyadic parse for a leading symbol. A non-primitive (user) symbol is parsed as a
    /// variable/function reference instead. This is a heuristic — Kap resolves valence
    /// at runtime; the set is the primitives wired up in the evaluator.)
    /// True when the next token ends the current expression at this nesting level:
    /// a newline, a `⋄` separator, end of input, or a *close* delimiter (`}`, `)`, `]`)
    /// that belongs to an enclosing construct (a block, a parenthesised group, a bracket
    /// index). Close delimiters always terminate an apply/strand expression — we must
    /// never try to parse them as an operand or operator.
    fn at_statement_boundary(&self) -> bool {
        match self.peek() {
            Some(t) => {
                matches!(t.token, Token::Newline)
                    || matches!(t.token, Token::StatementSeparator)
                    || matches!(t.token, Token::EndOfFile)
                    || matches!(t.token, Token::CloseBrace)
                    || matches!(t.token, Token::CloseParen)
                    || matches!(t.token, Token::CloseBracket)
            }
            None => true,
        }
    }

    fn is_primitive_op(name: &str) -> bool {
        // Only BARE primitive operators. Namespaced builtins (`io:print`, `unicode:enc`, …)
        // are resolved at runtime and must take the dyadic `L f R` path (via `is_known_fn`),
        // NOT be treated as monadic primitives — otherwise `"UTF16" unicode:enc "A"` would
        // drop its left argument.
        if name.contains(':') {
            return false;
        }
        matches!(
            name,
            "⍳" | "iota" | "⍴" | "rho" | "≢" | "tally" | "⊃" | "first" | "⌽" | "⊖" | "⍉"
                | "↑" | "↓" | "⊂" | "⌷" | "reveal" | "disclose"
                | "+" | "-" | "*" | "×" | "÷" | "/" | "⌿" | "\\" | "⍀" | "=" | "≠" | "<" | ">"
                | "≤" | "≥" | "," | "⍪" | "⌈" | "⌊" | "|" | "⍟" | "∧" | "∨" | "~" | "∊" | "⍋" | "⊤" | "⊥" | "⍸" | "⍒" | "⍲" | "⍱" | "∼"
                | "⊢" | "⊣" | "≡" | "⍓" | "∪" | "∩" | "!" | "…" | "⍷" | "cmp" | "⋆" | "√" | "⍮" | "pair"
                | "⊆" | "⊇" | "→" | "≬" | "toList" | "fromList" | "⫇" | "group"
                | "⍕" | "format" | "⍎" | "execute" | "typeof"
                | "namespace" | "import" | "declare" | "use" | "isLocallyBound"
                | "throw"
        )
    }

    /// Whether the *next* token begins a *function expression* (suitable for a dyadic
    /// `L f R` or a train member). Mirrors Kap's parser.kt `processFn` detection of the
    /// function position: a brace/lambda/paren/group, or a symbol naming a known function,
    /// a primitive operator, or an adverb.
    fn next_is_function_token(&mut self) -> bool {
        match self.peek() {
            Some(t) => match &t.token {
                // A bare `(` group is a function operand *only* when its contents are a
                // function expression (a train / derived fn / function name). A parenthesised
                // *data* value like `(2;1)` or `(1 2 3)` must NOT be swallowed as a function
                // operand — otherwise `x -foo (2;1)` would consume `(2;1)` as a right function
                // operand and leave the operator's data-right arg empty. Use a guarded
                // lookahead: try a train parse on the contents, restoring position on failure.
                Token::OpenParen => {
                    let save = self.pos;
                    let is_fn = self.try_parse_train().is_some();
                    self.pos = save;
                    is_fn
                }
                Token::OpenBrace | Token::LambdaToken | Token::ApplyToken
                | Token::ReverseComposeToken | Token::ComposeToken | Token::LeftForkToken => true,
                Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                    self.is_known_fn(name, namespace) || Self::is_primitive_op(name) || Self::is_adverb(name)
                }
                _ => false,
            },
            None => false,
        }
    }

    /// Higher-order operators (adverbs) that take a *function* as one operand:
    /// `/` reduce, `\` scan, `¨` each.
    fn is_adverb(name: &str) -> bool {
        matches!(name, "/" | "reduce" | "\\" | "scan" | "⌿" | "⍀" | "¨" | "each" | "⍨" | "commute" | "∵" | "bitwise" | "⌸" | "key" | "⌻" | "˝" | "inverse")
    }

    /// Try to parse a *train*: a parenthesised sequence of >=2 function expressions,
    /// e.g. `(f g h)`. Returns `Some(Train{funcs})` on success, `None` (without side effects
    /// other than `self.pos`, which the caller restores) otherwise.
    ///
    /// Each member is parsed with `parse_function_expr` (a *function atom* — symbols, derived
    /// operators, lambdas, or nested trains), NOT a full expression, so `(| -)` yields two
    /// separate functions `[|, -]` rather than the dyadic application `| -`.
    /// Parse a parenthesised group as Kotlin's inner `parseExpr` (parser.kt:939): a
    /// single left-to-right accumulator that strands values, applies functions
    /// dyadically when a function follows a value, and — crucially — when a function
    /// follows a *value* (or value-expression) it binds the value as the function's LEFT
    /// arg (LeftAssignedFunction) and continues chaining further functions as a 2-train.
    ///
    /// Examples (oracle-verified):
    /// - `(¯1r2 0+2÷⍨≢)` → `Train[ Train[Array(3r2,2), Derived{÷,⍨}], ≢ ]`   (value left-bind to fn-chain)
    /// - `(÷⍨≢)`       → `Train[ Derived{÷,⍨}, ≢ ]`                          (pure fn 2-train)
    /// - `(¯1r2 0+2)`   → `Array(3r2,2)`                                      (pure value)
    /// - `(3/2 2 ÷⍨≢)` → (3r2 2 ÷⍨≢) — same as the unparen'd form
    ///
    /// Returns `None` when the group is not a recognizable train/value (caller falls
    /// back to `parse_expr`). The opening `(` must already be consumed.
    fn parse_paren_vfn_chain(&mut self) -> Option<Instr> {
        // Peek the first real token to decide value-leading vs fn-leading.
        self.skip_newlines();
        let first_is_value = match self.peek().map(|t| &t.token) {
            Some(Token::Literal(LiteralValue::Number(_)))
            | Some(Token::Literal(LiteralValue::Char(_)))
            | Some(Token::Literal(LiteralValue::Str(_)))
            | Some(Token::Literal(LiteralValue::SymbolValue { .. })) => true,
            Some(Token::OpenBracket)
            | Some(Token::QuotePrefix) => true,
            _ => false,
        };
        if first_is_value {
            self.parse_paren_value_leading()
        } else {
            self.try_parse_train()
        }
    }

    /// Value-leading paren accumulator — a faithful port of Kotlin's `parseValueInner`
    /// (parser.kt:875–1029) for groups whose FIRST token is a value literal.
    ///
    /// Mutual recursion (oracle-verified matrix):
    /// - `(1 2 3)`     → Value(Array(1,2,3))
    /// - `(¯1r2 0+2)`  → Value((¯1r2 0)+2)          — dyadic fn + VALUE right ⇒ Apply, keep looping
    /// - `(2÷⍨)`       → Fn(Train[2, ÷⍨])           — fn + Empty tail ⇒ left-bind (parser.kt:462)
    /// - `(2÷⍨≢)`      → Fn(Train[Train[2,÷⍨],≢])   — fn + Fn tail ⇒ Chain2 (parser.kt:486)
    /// - `(¯1r2 0+2÷⍨≢)` → Fn(Train[Train[Array,+], Train[Train[2,÷⍨],≢]])  (stat.kap median)
    ///
    /// A `None` return means malformed (caller restores position and falls back).
    fn parse_paren_value_leading(&mut self) -> Option<Instr> {
        match self.parse_paren_accum() {
            crate::parser::ParenHolder::Value(i) | crate::parser::ParenHolder::Fn(i) => Some(i),
            crate::parser::ParenHolder::Empty => None,
            crate::parser::ParenHolder::Malformed => None,
        }
    }

    /// The recursive accumulator. Returns a Holder distinguishing value vs fn results
    /// (Kotlin `ParseResultHolder.InstrParseResult` vs `FnParseResult`), because the
    /// CALLER's behaviour depends on it (parser.kt:457–492).
    fn parse_paren_accum(&mut self) -> ParenHolder {
        let mut left_args: Vec<Instr> = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().map(|t| &t.token) {
                Some(Token::CloseParen) => {
                    self.advance();
                    // END_EXPR: makeResultList (parser.kt:942–948)
                    return if left_args.is_empty() {
                        ParenHolder::Empty
                    } else if left_args.len() == 1 {
                        ParenHolder::Value(left_args.pop().unwrap())
                    } else {
                        ParenHolder::Value(Instr::Array { elements: left_args })
                    };
                }
                None => return ParenHolder::Malformed,
                // A function token: Symbol(primitive or known user fn), paren group,
                // lambda block, ⍞ dynamic ref, or fork glyph.
                Some(Token::Literal(LiteralValue::Symbol { name, namespace })) => {
                    let is_fn = Self::is_primitive_op(name)
                        || self.is_known_fn(name, namespace);
                    if !is_fn {
                        // A value variable: strand it.
                        let t = self.peek().map(|t| t.token.clone()).unwrap();
                        self.advance();
                        if let Token::Literal(lv) = t {
                            left_args.push(Instr::Literal(lv));
                        }
                        continue;
                    }
                    // Parse the function WITH a trailing adverb folded (`÷⍨`).
                    let f0 = match self.parse_function_atom() {
                        Ok(a) => a,
                        Err(_) => return ParenHolder::Malformed,
                    };
                    let f = self.fold_trailing_adverb(f0);
                    // Recurse for everything after the function (Kotlin parseValue at :457).
                    match self.parse_paren_accum() {
                        ParenHolder::Empty => {
                            // parser.kt:458–463: left-bind when left args present.
                            return if left_args.is_empty() {
                                ParenHolder::Fn(f)
                            } else {
                                let v = strand_instrs(left_args);
                                ParenHolder::Fn(Instr::Train {
                                    funcs: vec![v, f],
                                    reverse: false,
                                    compose: false,
                                })
                            };
                        }
                        ParenHolder::Fn(g) => {
                            // parser.kt:479–491: Chain2 (atop) — with left-bind when
                            // leading values exist.
                            return if left_args.is_empty() {
                                ParenHolder::Fn(Instr::Train {
                                    funcs: vec![f, g],
                                    reverse: false,
                                    compose: false,
                                })
                            } else {
                                let v = strand_instrs(left_args);
                                ParenHolder::Fn(Instr::Train {
                                    funcs: vec![
                                        Instr::Train { funcs: vec![v, f], reverse: false, compose: false },
                                        g,
                                    ],
                                    reverse: false,
                                    compose: false,
                                })
                            };
                        }
                        ParenHolder::Value(v) => {
                            // parser.kt:465–477: dyadic/monadic APPLICATION — the result is
                            // a VALUE instruction; push and KEEP LOOPING (more tokens may
                            // follow inside the group).
                            let app = if left_args.is_empty() {
                                Instr::Apply {
                                    fn_expr: Box::new(f),
                                    left: None,
                                    right: Box::new(v),
                                }
                            } else {
                                let l = strand_instrs(std::mem::take(&mut left_args));
                                Instr::Apply {
                                    fn_expr: Box::new(f),
                                    left: Some(Box::new(l)),
                                    right: Box::new(v),
                                }
                            };
                            left_args.push(app);
                            continue;
                        }
                        ParenHolder::Malformed => return ParenHolder::Malformed,
                    }
                }
                // Plain value atom: strand it (parser.kt:991–1003 addLeftArg).
                Some(_) => {
                    match self.parse_function_atom() {
                        Ok(atom) => left_args.push(atom),
                        Err(_) => return ParenHolder::Malformed,
                    }
                }
            }
        }
    }

    /// Fold a single trailing adverb symbol into a bare primitive symbol, forming a
    /// Derived function (`÷` + `⍨` ⇒ `Derived{÷,⍨}`). Non-symbol or non-adverb → unchanged.
    fn fold_trailing_adverb(&mut self, f: Instr) -> Instr {
        if let Some(Token::Literal(LiteralValue::Symbol { name: adv, namespace: None })) =
            self.peek().map(|t| &t.token)
        {
            if Self::is_adverb(adv) && matches!(f, Instr::Symbol { namespace: None, .. }) {
                let adv = adv.clone();
                self.advance();
                return Instr::Derived {
                    func: Box::new(f),
                    op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                };
            }
        }
        f
    }

    fn try_parse_train(&mut self) -> Option<Instr> {
        // We are positioned just after the opening '('.
        let mut funcs = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if matches!(t.token, Token::CloseParen) => {
                    self.advance();
                    break;
                }
                None => return None,
                _ => {
                    // Left-bind pre-fold (Kotlin parser.kt makeLeftBindFunction:486, hit
                    // during parseOperator's member accumulation): a Literal followed by
                    // `<primitive fn><adverb>` (e.g. `2÷⍨`) binds INTO ONE derived member
                    // Train[Literal(2), Derived{÷,⍨}] BEFORE train-member classification.
                    // stat.kap median `(¯1r2 0+2÷⍨≢)`: without this, `Lit(2)` sits between
                    // the leading value run and the fn tail and fold_train_members bails.
                    if matches!(funcs.last(), Some(Instr::Literal(_)))
                        && !matches!(self.peek().map(|t| &t.token), Some(Token::CloseParen))
                    {
                        let save = self.pos;
                        let is_bind_shape = matches!(
                            self.peek_at(0).map(|t| &t.token),
                            Some(Token::Literal(LiteralValue::Symbol { name, namespace }))
                                if namespace.is_none()
                                    && Self::is_primitive_op(name)
                                    && !Self::is_adverb(name)
                        ) && matches!(
                            self.peek_at(1).map(|t| &t.token),
                            Some(Token::Literal(LiteralValue::Symbol { name, namespace }))
                                if namespace.is_none() && Self::is_adverb(name)
                        );
                        if is_bind_shape {
                            // Re-parse the two symbol tokens as instrs (peek borrows released).
                            let f = match self.parse_function_atom() {
                                Ok(f) => f,
                                Err(_) => {
                                    self.pos = save;
                                    return None;
                                }
                            };
                            // Capture the adverb BEFORE advancing past it.
                            let adv = match self.peek().map(|t| &t.token) {
                                Some(Token::Literal(LiteralValue::Symbol { name, namespace })) => {
                                    Instr::Symbol { name: name.clone(), namespace: namespace.clone() }
                                }
                                _ => {
                                    self.pos = save;
                                    return None;
                                }
                            };
                            self.advance();
                            let lit = funcs.pop().unwrap();
                            funcs.push(Instr::Train {
                                funcs: vec![lit, Instr::Derived { func: Box::new(f), op: Box::new(adv) }],
                                reverse: false,
                                compose: false,
                            });
                            continue;
                        }
                    }
                    // Parse each member as a single *function atom* (not a full expression),
                    // so `1 +` inside `(1 +)` yields two members [Literal(1), Symbol(+)] (a
                    // left-bind) rather than `parse_function_expr` stopping at `1` and
                    // orphaning the `+`.
                    let e = match self.parse_function_atom() {
                        Ok(e) => e,
                        Err(_) => return None,
                    };
                    // Rank-operator form `f⍤spec` inside a parenthesised group:
                    // the member is a function followed by `⍤` and a (numeric) spec.
                    // Parse it as a ValueOp derived function and use THAT as the member.
                    if let Some(Token::Literal(LiteralValue::Symbol { name, namespace })) = self.peek().map(|t| &t.token) {
                        if name == "⍤" {
                            self.advance(); // consume ⍤
                            let mut spec = match self.parse_primary() {
                                Ok(s) => s,
                                Err(_) => return None,
                            };
                            // Absorb additional numeric spec elements into a strand.
                            loop {
                                let more = matches!(
                                    self.peek().map(|t| &t.token),
                                    Some(Token::Literal(LiteralValue::Number(_)))
                                );
                                if !more {
                                    break;
                                }
                                let elem = match self.parse_primary() {
                                    Ok(s) => s,
                                    Err(_) => return None,
                                };
                                if let Instr::Array { elements } = &mut spec {
                                    elements.push(elem);
                                } else {
                                    spec = Instr::Array { elements: vec![spec, elem] };
                                }
                            }
                            funcs.push(Instr::ValueOp {
                                func: Box::new(e),
                                op_name: "⍤".to_string(),
                                operand: Box::new(spec),
                            });
                            continue;
                        }
                        // Native value-op inside a parenthesised group:
                        // `(↑ int:proto 5)` binds as ValueOp{↑, "int:proto", 5}.
                        if namespace.as_deref() == Some("int") && name == "proto" {
                            self.advance(); // consume int:proto
                            let spec = match self.parse_apply() {
                                Ok(s) => s,
                                Err(_) => return None,
                            };
                            funcs.push(Instr::ValueOp {
                                func: Box::new(e),
                                op_name: "int:proto".to_string(),
                                operand: Box::new(spec),
                            });
                            continue;
                        }
                    }
                    // Operator-derivation: a function member immediately followed by an operator
                    // glyph (a known *user* operator OR a builtin adverb) binds as an OpCall /
                    // Derived function rather than a train member. e.g. `(≠⌸)` ->
                    // OpCall{op:⌸, left_fn:≠}, `(×/)` -> Derived{func:×, op:/},
                    // `(f¨)` -> Derived{func:f, op:¨}. This mirrors the flat `parse_apply`
                    // operator-call detection, but here the operands are parenthesised.
                    // NOTE: copy the name into an owned `String` *before* any `&mut self`
                    // call (e.g. `parse_primary`/`next_is_function_token`) — holding the
                    // immutable `peek()` borrow open across those would violate the borrow
                    // checker.
                    let op_name: Option<String> = match self.peek().map(|t| &t.token) {
                        Some(Token::Literal(LiteralValue::Symbol { name, .. })) => {
                            let is_user_op = self.known_ops.iter().any(|n| n == name);
                            if is_user_op || Self::is_adverb(name) {
                                Some(name.clone())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(name) = op_name {
                        // A keyword-namespaced symbol (`namespace == "keyword"`, e.g.
                        // `:export`, `:const`) is NEVER an operator operand — it is a
                        // declaration keyword. If the preceding member `e` is a keyword
                        // symbol, do NOT bind it as the left operand of the operator;
                        // instead let `e` stand as a plain member (so `(:export ⌸)` →
                        // `[keyword:export, ⌸]`, a symbol-array list literal for
                        // `declare`, not a `Derived{:export, ⌸}` that would try to
                        // *apply* the keyword).
                        let e_is_keyword = matches!(
                            e,
                            Instr::Symbol {
                                namespace: Some(ref ns),
                                ..
                            } if ns == "keyword"
                        );
                        if e_is_keyword {
                            funcs.push(e);
                            continue;
                        }
                        let is_user_op = self.known_ops.iter().any(|n| n == &name);
                        self.advance();
                        self.skip_newlines();
                        // A 2-arg user operator (`∇ (x op y) a`) consumes a second
                        // function operand (`-foo+`). Builtin adverbs (`/`, `¨`, `⌸`)
                        // take only the single left function operand.
                        let right_fn = if is_user_op
                            && self.next_is_function_token()
                            && !self.at_statement_boundary()
                        {
                            match self.parse_primary() {
                                Ok(p) => Some(Box::new(p)),
                                Err(_) => return None,
                            }
                        } else {
                            None
                        };
                        let derived = if is_user_op {
                            Instr::OpCall {
                                op: Box::new(Instr::Symbol {
                                    name: name.clone(),
                                    namespace: None,
                                }),
                                left_fn: Box::new(e),
                                right_fn,
                            }
                        } else {
                            Instr::Derived {
                                func: Box::new(e),
                                op: Box::new(Instr::Symbol {
                                    name: name.clone(),
                                    namespace: None,
                                }),
                            }
                        };
                        funcs.push(derived);
                            continue;
                        }
                    funcs.push(e);
                    // Postfix compose/atop inside a parenthesised train: `a ∘ b` / `a ⍛ b`
                    // bind the just-pushed `a` to the next function atom `b` into a single
                    // compose Train (compose=true). e.g. `(-⍛+)` -> ReverseCompose(-, +).
                    // This mirrors the standalone `parse_function_expr` compose handling,
                    // but here the left operand is already in `funcs`.
                    if let Some(tok) = self.peek() {
                        let reverse = match &tok.token {
                            Token::ComposeToken => Some(false),
                            Token::ReverseComposeToken => Some(true),
                            _ => None,
                        };
                        if let Some(rev) = reverse {
                            self.advance();
                            // Kotlin parseFunctionForOperatorRightArg (op.kt:31):
                            // SINGLE function right operand. `∧` chains separately.
                            let right = match self.parse_function_atom() {
                                Ok(r) => r,
                                Err(_) => return None,
                            };
                            let left = funcs.pop().unwrap();
                            funcs.push(Instr::Train {
                                funcs: vec![left, right],
                                reverse: rev,
                                compose: true,
                            });
                        }
                    }
                    // Fork postfix: `a « b » c` (Kap's right-fork / `⊢«⊣»,` style).
                    // The trailing `c` is optional: `a « b »` with nothing after `»`
                    // is a 2-train (atop) `[a, b]`; `a « b » c` is a 3-fork.
                    if let Some(Token::LeftForkToken) = self.peek().map(|t| &t.token) {
                        self.advance();
                        let b = match self.parse_function_atom() {
                            Ok(b) => b,
                            Err(_) => return None,
                        };
                        self.expect(Token::RightForkToken, "expected » in fork").ok()?;
                        self.skip_newlines();
                        // Optional third function. A `)` (end of group) or any
                        // non-function-atom token means this is a 2-train `[a, b]`.
                        let is_func_start = match self.peek().map(|t| &t.token) {
                            Some(Token::Literal(LiteralValue::Symbol { .. }))
                            | Some(Token::OpenParen)
                            | Some(Token::LambdaToken)
                            | Some(Token::LeftForkToken) => true,
                            _ => false,
                        };
                        if is_func_start {
                            let c = match self.parse_function_atom() {
                                Ok(c) => c,
                                Err(_) => return None,
                            };
                            funcs.push(b);
                            funcs.push(c);
                        } else {
                            funcs.push(b);
                        }
                    }
                }
            }
        }
        // A train needs >= 2 members. Either all are functions, OR it is a 2-train
        // left-bind `[value, function]` (e.g. `(10 +)`).
        //
        // A 2-member group that is *not* a left-bind must be a genuine function composition
        // (atop) `f g`. That requires BOTH members to be *definite* functions — a primitive,
        // a known function/operator, a lambda, a derived function, or a nested train. A bare
        // variable symbol (e.g. `x` in `(⌷x)`) is NOT definite: at runtime it may hold a
        // *value*, so `(⌷x)` must parse as the monadic application `⌷ x`, not the 2-train
        // `⌷∘x` (which would treat `x` as a function and fail). Genuine function-variable
        // trains like `(f g)` still work because `f`/`g` are registered as known functions.
        // A train's members must be *definite* functions (primitives, known fns,
        // lambdas, derived trains) — NOT bare unknown symbols. A group of bare symbols
        // like `(a b c d e)` is a *list literal* of symbols (see `declare`), not a
        // 5-function train. Using `is_function_expr` here (which admits ANY symbol)
        // would wrongly turn `(a b c d e)` into a 5-train.
        let all_funcs = funcs.iter().all(|f| self.is_definite_function(f));
        let left_bind = funcs.len() == 2
            && matches!(funcs[0], Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
            && matches!(funcs[1], Instr::Symbol { .. } | Instr::Derived { .. } | Instr::Lambda { .. } | Instr::Train { .. });
        // Oracle-verified (kap-jvm-text, 2026-08-25): implicit juxtaposition of
        // functions inside parens is a RIGHT-ASSOCIATED ATOP chain — NOT an APL
        // fork. Evidence: `(≢,≢) 7 8 9` → 1 (fork would be ⟨3 3⟩);
        // `(+×÷) 6` → 1 = +∘(×∘(÷6))·signum; `(A + D) y` → A+(D y) elementwise.
        // Explicit forks use the «» glyphs. So fold members right-associated:
        // [f0, f1, f2] ⇒ Train[f0, Train[f1, f2]].
        //
        // Mixed value+fn groups (stat.kap median `(¯1r2 0+2÷⍨≢)`): leading VALUE
        // members strand into one constant Array, then the rassoc-fold applies:
        // [Lit(-1/2), Lit(0), +, D] ⇒ Train[Array(A), Train[+, D]]; called with y,
        // the dyadic-atop arm gives A + (D y) — oracle-exact ⟨3/2 2⟩. Oracle also
        // confirms this shape is accepted ONLY as an operator operand / fn chain
        // member (`m⇐(…)` errors, `(3)⍛+` errors) — so we require ≥1 trailing fn.
        if let Some(folded) = self.fold_train_members(&mut funcs) {
            let mut it = folded.into_iter().rev();
            let mut cur = it.next().unwrap();
            for f in it {
                cur = Instr::Train {
                    funcs: vec![f, cur],
                    reverse: false,
                    compose: false,
                };
            }
            return Some(cur);
        }
        if funcs.len() >= 2 && (all_funcs || left_bind) {
            if funcs.len() == 2 {
                Some(Instr::Train { funcs, reverse: false, compose: false })
            } else {
                // Right-fold: f0 ∘ (f1 ∘ (... fn)).
                let mut it = funcs.into_iter().rev();
                let mut cur = it.next().unwrap();
                for f in it {
                    cur = Instr::Train {
                        funcs: vec![f, cur],
                        reverse: false,
                        compose: false,
                    };
                }
                Some(cur)
            }
        } else if funcs.len() == 2
            && matches!(funcs[1], Instr::DynamicRef { .. })
            && matches!(funcs[0], Instr::Symbol { .. } | Instr::Derived { .. } | Instr::Train { .. } | Instr::ValueOp { .. })
        {
            // `[fn, ⍞ref]`: the DynamicRef ALWAYS denotes a function (it is only
            // resolvable inside a macro expansion), so a symbol/derived member in
            // front of it can only be an atop — even when the symbol's function-ness
            // cannot be verified at parse time (e.g. `toBoolean` in an unexpanded
            // macro body). Kotlin resolves these names at evaluation time.
            Some(Instr::Train { funcs, reverse: false, compose: false })
        } else if funcs.len() == 1 && Self::is_function_expr(&funcs[0]) {
            Some(Instr::Train { funcs, reverse: false, compose: false })
        } else if !funcs.is_empty()
            && funcs.iter().all(|f| matches!(f, Instr::Symbol { .. }))
            && !matches!(&funcs[0], Instr::Symbol { name, namespace: None } if Self::is_primitive_op(name))
        {
            Some(Instr::Array { elements: funcs })
        } else {
            None
        }
    }

    /// Classify train members into a foldable chain: optional LEADING VALUE
    /// members (literals strand into one constant Array) followed by ≥1
    /// definite-function members. Returns the folded member list, or None —
    /// restoring `funcs` unchanged — when the members don't fit that shape
    /// (pure values, bare-symbol lists, non-definite tails).
    fn fold_train_members(&self, funcs: &mut Vec<Instr>) -> Option<Vec<Instr>> {
        // A single member can never chain; bail out immediately without
        // touching `funcs` (the `Empty` sentinel below would corrupt it).
        if funcs.len() < 2 {
            return None;
        }
        let mut out: Vec<Instr> = Vec::with_capacity(funcs.len());
        let mut pending_values: Vec<Instr> = Vec::new();
        let mut tail_started = false;
        let mut idx = 0;
        while idx < funcs.len() {
            let f = std::mem::replace(&mut funcs[idx], Instr::Empty);
            if !tail_started && matches!(f, Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
            {
                pending_values.push(f);
            } else {
                if !tail_started {
                    tail_started = true;
                    match pending_values.len() {
                        0 => {}
                        1 => out.push(pending_values.pop().unwrap()),
                        _ => out.push(Instr::Array {
                            elements: std::mem::take(&mut pending_values),
                        }),
                    }
                }
                if self.is_definite_function(&f) {
                    out.push(f);
                } else {
                    // Not a foldable shape: restore everything.
                    funcs.insert(idx, f);
                    for v in pending_values.into_iter().rev() {
                        funcs.insert(idx, v);
                    }
                    return None;
                }
            }
            idx += 1;
        }
        // Restore on failure shapes: no trailing function found (pure values),
        // or fewer than two folded members (nothing to chain).
        if !tail_started || out.len() < 2 {
            for v in pending_values.into_iter().rev() {
                funcs.insert(0, v);
            }
            // Put back any already-moved tail members.
            for f in out.into_iter().rev() {
                funcs.insert(0, f);
            }
            return None;
        }
        Some(out)
    }

    /// A *definite* function expression suitable for a 2-train (`f g` atop): a primitive
    /// symbol, a known function/operator symbol, a lambda, a derived adverb, or a nested
    /// train. Excludes bare variable symbols (which may hold a value at runtime) and literals.
    fn is_definite_function(&self, e: &Instr) -> bool {
        match e {
            Instr::Symbol { name, namespace } => {
                // A primitive/identity symbol, OR a namespace-qualified name
                // (`io:print` => name="print", namespace=Some("io")) which can only
                // reference a module function (never a local value), so it is safe to
                // treat as a definite function in a 2-train. This lets `(⊣ io:print)`
                // parse as an atop; a bare local variable (`x` in `(⌷x)`) stays
                // non-definite so it parses as the monadic application `⌷ x` instead.
                // A KNOWN user-defined function (`(toBoolean ⍞fn)`) is also definite.
                Self::is_primitive_op(name)
                    || name == "⊢"
                    || name == "⊣"
                    || (namespace.is_some() && namespace.as_deref() != Some("keyword"))
                    || self.is_known_fn(name, namespace)
            }
            Instr::Derived { .. } | Instr::OpCall { .. } | Instr::Lambda { .. } | Instr::Train { .. } | Instr::ValueOp { .. } => true,
            // `⍞name` (DynamicRef) ALWAYS means "fetch the value bound to name as a
            // function" — never a data variable — so it is a definite function for
            // train members (util.kap filter body `(toBoolean ⍞fn)¨ arg`).
            Instr::DynamicRef { .. } => true,
            _ => false,
        }
    }

    /// Parse a *function expression*: a function atom optionally followed by a compose
    /// operator (`∘` atop, `⍛` reverse-compose), or a fork postfix (`A « B » C`).
    ///
    /// `allow_train` gates 2-train chaining (`×-` -> Train([×, -])). It is **only** true
    /// when parsing the RHS of `⇐` (function definition position), because there a bare
    /// `f g` is genuinely a derived 2-train. In value-apply position (`3 ↑ ⍳10`, `3 < 5`)
    /// the same `f g` pattern is a dyadic call `L f (g …)` and must NOT be chained — so
    /// the public entry point passes `false`.
    fn parse_function_expr(&mut self) -> Result<Instr, AplError> {
        self.parse_function_expr_impl(false)
    }

    fn parse_function_expr_impl(&mut self, allow_train: bool) -> Result<Instr, AplError> {
        let mut left = self.parse_function_atom()?;
        self.parse_function_expr_continuation_impl(&mut left, allow_train)
    }

    /// Continuation of `parse_function_expr_impl` starting from an already-parsed
    /// `left` — handles power/dual/rank/compose/fork/adverb/2-train arms. Used by the
    /// value-strand left-bind arm to delegate compose/fork handling after building
    /// the inner `Train[Array(v*), f0]` pair. `allow_train` is true because this is
    /// only called from the `⇐` RHS context (where 2-trains are valid).
    fn parse_function_expr_continuation(&mut self, left: Instr) -> Result<Instr, AplError> {
        let mut l = left;
        self.parse_function_expr_continuation_impl(&mut l, true)
    }

    fn parse_function_expr_continuation_impl(
        &mut self,
        left: &mut Instr,
        allow_train: bool,
    ) -> Result<Instr, AplError> {
        let mut left = std::mem::replace(left, Instr::Empty);
        // When called from the continuation (allow_train=false), skip the
        // value-strand arm — it already ran in the original entry.
        // Power operator `f⍣n` / `f⍣g` (Kotlin PowerAPLOperator, operator.kt:7,
        // engine.kt:491 registerNativeOperator("⍣")). Like `f⍤spec`, it is a
        // derived function stored as a ValueOp; the MODE is decided at eval time
        // from the operand shape (Kotlin combines fn+expr ⇒ iterate-count,
        // fn+fn ⇒ until-loop).
        if let Some(t) = self.peek() {
            let is_power = matches!(&t.token, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "⍣");
            if is_power {
                self.advance(); // consume ⍣
                let operand = self.parse_function_atom()?;
                return Ok(Instr::ValueOp {
                    func: Box::new(left),
                    op_name: "⍣".to_string(),
                    operand: Box::new(operand),
                });
            }
            // Dual / structural-under `base ⍢ wrapper`: ValueOp like ⍣ (Kotlin
            // StructuralUnderOp, engine.kt:501). Evaluator applies
            // wrapper⁻¹ ∘ base ∘ wrapper.
            let is_dual = matches!(&t.token, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "⍢");
            if is_dual {
                self.advance(); // consume ⍢
                let operand = self.parse_function_atom()?;
                return Ok(Instr::ValueOp {
                    func: Box::new(left),
                    op_name: "⍢".to_string(),
                    operand: Box::new(operand),
                });
            }
        }
        // Rank-operator form `f⍤spec` is a *derived function* (APLOperatorValueRightArg).
        if let Some(t) = self.peek() {
            let is_rank = matches!(&t.token, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "⍤");
            if is_rank {
                self.advance(); // consume ⍤
                let mut spec = self.parse_primary()?;
                loop {
                    let more = matches!(
                        self.peek().map(|t| &t.token),
                        Some(Token::Literal(LiteralValue::Number(_)))
                    );
                    if !more {
                        break;
                    }
                    let elem = self.parse_primary()?;
                    if let Instr::Array { elements } = &mut spec {
                        elements.push(elem);
                    } else {
                        spec = Instr::Array { elements: vec![spec, elem] };
                    }
                }
                return Ok(Instr::ValueOp {
                    func: Box::new(left),
                    op_name: "⍤".to_string(),
                    operand: Box::new(spec),
                });
            }
        }
        // Postfix compose:  f ∘ g  ->  Train([f, g], compose=true)        (atop)
        //                   f ⍛ g  ->  Train([f, g], reverse=true, compose=true)
        // Postfix fork:     a « b » c  ->  Train([a, b, c])              (fork)
        if let Some(t) = self.peek() {
            match &t.token {
                Token::ComposeToken => {
                    self.advance();
                    // `∘∙f` (outer product): ∘ then the InnerProduct operator.
                    if let Some(t) = self.peek() {
                        if matches!(&t.token, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "∙") {
                            self.advance();
                            let right = self.parse_function_atom()?;
                            return Ok(Instr::InnerProduct { left_fn: None, right_fn: Box::new(right) });
                        }
                    }
                    let right = self.parse_function_atom()?;
                    return Ok(Instr::Train { funcs: vec![left, right], reverse: false, compose: true });
                }
                Token::ReverseComposeToken => {
                    self.advance();
                    // Kotlin parseFunctionForOperatorRightArg (op.kt:31): SINGLE
                    // function right operand. After binding, the outer
                    // parseOperator loop (parser.kt:1276) CONTINUES — a trailing
                    // fn like `∧` chains as a 2-train atop. So do NOT return
                    // immediately; fall through to the 2-train/adverb arms by
                    // updating `left` and continuing.
                    let right = self.parse_function_atom()?;
                    left = Instr::Train { funcs: vec![left, right], reverse: true, compose: true };
                    // Re-check for adverb/fork on the new compose train, then
                    // fall through to the 2-train loop below.
                    if let Some(t) = self.peek() {
                        match &t.token {
                            Token::Literal(LiteralValue::Symbol { name, .. })
                                if Self::is_adverb(name) =>
                            {
                                let adv = name.clone();
                                self.advance();
                                left = Instr::Derived {
                                    func: Box::new(left),
                                    op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                                };
                            }
                            Token::LeftForkToken => {
                                self.advance();
                                self.skip_newlines();
                                let b = self.parse_function_atom()?;
                                self.skip_newlines();
                                self.expect(Token::RightForkToken, "expected » in fork")?;
                                self.skip_newlines();
                                let c = self.parse_function_atom()?;
                                return Ok(Instr::Train {
                                    funcs: vec![left, b, c],
                                    reverse: false,
                                    compose: false,
                                });
                            }
                            _ => {}
                        }
                    }
                    // Fall through: check for a trailing fn-atom 2-train (e.g. `∧`).
                    if let Some(t) = self.peek() {
                        let next_is_fn_atom = match &t.token {
                            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                                self.is_fn_atom_name(name, namespace)
                            }
                            Token::OpenParen | Token::LambdaToken | Token::ApplyToken | Token::LeftForkToken => true,
                            _ => false,
                        };
                        if next_is_fn_atom {
                            let right = self.parse_function_atom()?;
                            return Ok(Instr::Train {
                                funcs: vec![left, right],
                                reverse: false,
                                compose: false,
                            });
                        }
                    }
                    return Ok(left);
                }
                Token::LeftForkToken => {
                    // a « b » c  ->  fork. `a` is the already-parsed `left`.
                    self.advance();
                    self.skip_newlines();
                    let b = self.parse_function_atom()?;
                    self.skip_newlines();
                    self.expect(Token::RightForkToken, "expected » in fork")?;
                    self.skip_newlines();
                    let c = self.parse_function_atom()?;
                    return Ok(Instr::Train { funcs: vec![left, b, c], reverse: false, compose: false });
                }
                _ => {}
            }
        }
        // A function atom optionally followed by an adverb (`/`, `¨`, `⍟`, `\\`) forms a
        // *derived* function, e.g. `×/`, `+¨`, `×\\`. This mirrors the `f op` derived-function
        // production in `parse_apply`, but here (function-expression position) no trailing
        // data operand is consumed — the derived function stands alone (e.g. `foo ⇐ ×/`).
        if let Some(t) = self.peek() {
            let adv_name = match &t.token {
                Token::Literal(LiteralValue::Symbol { name, .. }) if Self::is_adverb(name) => {
                    Some(name.clone())
                }
                _ => None,
            };
            if let Some(adv) = adv_name {
                self.advance();
                let derived = Instr::Derived {
                    func: Box::new(left),
                    op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                };
                // Fork postfix on the derived function: `+/«÷»≢` (stat.kap avg).
                // The Derived is the fork's LEFT member; parse middle + right and
                // return the 3-train. Without this the early return leaves « unconsumed.
                if matches!(self.peek().map(|t| &t.token), Some(Token::LeftForkToken)) {
                    self.advance(); // consume «
                    self.skip_newlines();
                    let b = self.parse_function_atom()?;
                    self.skip_newlines();
                    self.expect(Token::RightForkToken, "expected » in fork")?;
                    self.skip_newlines();
                    let c = self.parse_function_atom()?;
                    return Ok(Instr::Train {
                        funcs: vec![derived, b, c],
                        reverse: false,
                        compose: false,
                    });
                }
                return Ok(derived);
            }
        }
        // 2-train chaining (atop): a function atom immediately followed by *another* function
        // atom (not an adverb/compose handled above) forms a 2-train, e.g. `×-` ->
        // Train([×, -]). This is gated behind `allow_train`: it is ONLY valid when parsing
        // the RHS of `⇐` (function-definition position), where a bare `f g` is genuinely a
        // derived 2-train. In value-apply position (`3 ↑ ⍳10`, `3 < 5`) the same `f g`
        // pattern is a dyadic call `L f (g …)` and must NOT be chained — there `allow_train`
        // is `false`, so we return the single `left` and let `parse_apply` handle the dyadic
        // application. The guard also requires the next token to be a function atom (a
        // primitive op, or a bracketed/lambda/dynamic/fork group) so we never chain on a
        // value variable or operand.
        if allow_train {
            // Leading VALUE strand in a `⇐` RHS: `1 2+≢` (Kotlin parseValue accumulates
            // the strand, then processFn applies + dyadically). The port's fn-atom
            // parser stops at the literal, so fold the value run ourselves:
            // [Lit(1), Lit(2), +, ≢] ⇒ Train[Array[1,2], +, ≢] — called with y,
            // the left-bind rule gives (1 2)+(≢y). Oracle: `p9⇐1 2+≢ ⋄ p9 5` → ⟨2 3⟩.
            if matches!(left, Instr::Literal(_)) {
                let mut vals = vec![left.clone()];
                while matches!(self.peek().map(|t| &t.token), Some(Token::Literal(LiteralValue::Number(_)))) {
                    vals.push(self.parse_function_atom()?);
                }
                let is_chain_start = matches!(
                    self.peek().map(|t| &t.token),
                    Some(Token::Literal(LiteralValue::Symbol { name, .. })) if Self::is_primitive_op(name)
                );
                if vals.len() > 1 && is_chain_start {
                    // Kotlin `parseExpr` returns at the FIRST FnParseResult, so a
                    // `⇐` RHS / train value+fn chain is built by processFn's
                    // FnParseResult branch: value-strand ⇒ leftArgs, first fn ⇒
                    // Chain2[ makeLeftBindFunction(value, f0), f1 ]. Hence:
                    //   [v*, f0, f1, f2] => Train[ Train[Array(v*), f0],
                    //                                   (right-fold of f1,f2) ]
                    // Chain2(fn0,fn1).eval1Arg(a) = fn0(fn1(a)) (instr.kt:588) ⇒
                    // the value-left-bind is the INNER pair (fn0 = f0, applied
                    // dyadically to (v*; arg)), and the remaining functions RIGHT-
                    // FOLD as the outer fn0 (so f1 wraps f2's result). Oracle:
                    // `f⇐1 2+≢ ⋄ f 5` → ⟨2 3⟩ ; `f⇐1 2+×≢ ⋄ f 5` → ⟨2 3⟩.
                    let arr = Instr::Array { elements: vals };
                    // First function member (the left-bind target).
                    let mut f0 = self.parse_function_atom()?;
                    if let Some(Token::Literal(LiteralValue::Symbol { name: adv, namespace: None })) =
                        self.peek().map(|t| &t.token)
                    {
                        if Self::is_adverb(adv)
                            && matches!(f0, Instr::Symbol { namespace: None, .. })
                        {
                            let adv = adv.clone();
                            self.advance();
                            f0 = Instr::Derived {
                                func: Box::new(f0),
                                op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                            };
                        }
                    }
                    // Build the inner value-left-bind pair and delegate the rest
                    // to a RECURSIVE call to parse_function_expr_impl so the
                    // compose/fork/2-train arms handle `⍛`/`∘`/`«»`/adverbs
                    // properly on the left-bind pair (e.g. stat.kap median:
                    // `2÷⍨ +/ (…)⍛⊇ ∧` — the `⍛` binds the paren group, not the
                    // outer train). Without this, the greedy loop eats `+/` and
                    // `(…)` as plain 2-train members, leaving `⍛` dangling.
                    let inner = Instr::Train {
                        funcs: vec![arr, f0],
                        reverse: false,
                        compose: false,
                    };
                    // The recursive call treats `inner` as the already-parsed left
                    // and handles compose/fork/adverb/2-train on it. allow_train=false
                    // because the value-strand is already consumed; any further fn
                    // chaining is a 2-train handled by the recursive call's own arms.
                    return self.parse_function_expr_continuation(inner);
                }
            }
            if let Some(t) = self.peek() {
                let next_is_fn_atom = match &t.token {
                    Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                        self.is_fn_atom_name(name, namespace)
                    }
                    Token::OpenParen | Token::LambdaToken | Token::ApplyToken
                    | Token::LeftForkToken => true,
                    _ => false,
                };
                if next_is_fn_atom {
                    let mut right = self.parse_function_atom()?;
                    // Adverb folds into the just-parsed atom (`÷` + `⍨` → `÷⍨`).
                    if let Some(Token::Literal(LiteralValue::Symbol { name: adv, namespace: None })) =
                        self.peek().map(|t| &t.token)
                    {
                        if Self::is_adverb(adv)
                            && matches!(right, Instr::Symbol { namespace: None, .. })
                        {
                            let adv = adv.clone();
                            self.advance();
                            right = Instr::Derived {
                                func: Box::new(right),
                                op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                            };
                        }
                    }
                    // Compose/reverse-compose binds the JUST-PARSED atom (NOT
                    // the accumulated train) as its left — Kotlin parseOperator
                    // runs on each function individually (parser.kt:1273).
                    // `F (…)⍛⊇ ∧` ⇒ `⍛` binds `(…)`, not `F (…)`.
                    if let Some(t) = self.peek() {
                        match &t.token {
                            Token::ComposeToken => {
                                self.advance();
                                let r = self.parse_function_atom()?;
                                right = Instr::Train {
                                    funcs: vec![right, r],
                                    reverse: false,
                                    compose: true,
                                };
                                // Trailing adverb on the compose.
                                if let Some(Token::Literal(LiteralValue::Symbol { name: adv, namespace: None })) =
                                    self.peek().map(|t| &t.token)
                                {
                                    if Self::is_adverb(adv) {
                                        let adv = adv.clone();
                                        self.advance();
                                        right = Instr::Derived {
                                            func: Box::new(right),
                                            op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                                        };
                                    }
                                }
                            }
                            Token::ReverseComposeToken => {
                                self.advance();
                                let r = self.parse_function_atom()?;
                                right = Instr::Train {
                                    funcs: vec![right, r],
                                    reverse: true,
                                    compose: true,
                                };
                                // Trailing adverb/fork on the compose.
                                if let Some(Token::Literal(LiteralValue::Symbol { name: adv, namespace: None })) =
                                    self.peek().map(|t| &t.token)
                                {
                                    if Self::is_adverb(adv) {
                                        let adv = adv.clone();
                                        self.advance();
                                        right = Instr::Derived {
                                            func: Box::new(right),
                                            op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                                        };
                                    }
                                }
                            }
                            Token::LeftForkToken => {
                                self.advance();
                                self.skip_newlines();
                                let b = self.parse_function_atom()?;
                                self.skip_newlines();
                                self.expect(Token::RightForkToken, "expected » in fork")?;
                                self.skip_newlines();
                                let c = self.parse_function_atom()?;
                                right = Instr::Train {
                                    funcs: vec![right, b, c],
                                    reverse: false,
                                    compose: false,
                                };
                            }
                            _ => {}
                        }
                    }
                    // Now chain `right` (possibly compose-wrapped) as a
                    // 2-train atop with `left`. Then loop for more fns.
                    let mut cur = Instr::Train {
                        funcs: vec![left, right],
                        reverse: false,
                        compose: false,
                    };
                    loop {
                        let more = match self.peek().map(|t| &t.token) {
                            Some(Token::Literal(LiteralValue::Symbol { name, .. })) => {
                                Self::is_primitive_op(name)
                            }
                            Some(Token::OpenParen) | Some(Token::LambdaToken)
                            | Some(Token::ApplyToken) | Some(Token::LeftForkToken) => true,
                            _ => false,
                        };
                        if !more {
                            break;
                        }
                        let mut member = self.parse_function_atom()?;
                        // Adverb folds into the atom (`+` + `/` → `+/`).
                        if let Some(Token::Literal(LiteralValue::Symbol { name: adv, namespace: None })) =
                            self.peek().map(|t| &t.token)
                        {
                            if Self::is_adverb(adv)
                                && matches!(member, Instr::Symbol { namespace: None, .. })
                            {
                                let adv = adv.clone();
                                self.advance();
                                member = Instr::Derived {
                                    func: Box::new(member),
                                    op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                                };
                            }
                        }
                        // Compose/reverse-compose binds the JUST-PARSED atom
                        // (NOT the accumulated `cur`) as its left — Kotlin
                        // parseOperator runs per-function (parser.kt:1273).
                        if let Some(t) = self.peek() {
                            match &t.token {
                                Token::ComposeToken => {
                                    self.advance();
                                    // `∘∙f` (outer product) inside a train member.
                                    if let Some(t2) = self.peek() {
                                        if matches!(&t2.token, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "∙") {
                                            self.advance();
                                            let r = self.parse_function_atom()?;
                                            member = Instr::InnerProduct { left_fn: None, right_fn: Box::new(r) };
                                        } else {
                                            let r = self.parse_function_atom()?;
                                            member = Instr::Train {
                                                funcs: vec![member, r],
                                                reverse: false,
                                                compose: true,
                                            };
                                        }
                                    } else {
                                        let r = self.parse_function_atom()?;
                                        member = Instr::Train {
                                            funcs: vec![member, r],
                                            reverse: false,
                                            compose: true,
                                        };
                                    }
                                }
                                Token::ReverseComposeToken => {
                                    self.advance();
                                    let r = self.parse_function_atom()?;
                                    member = Instr::Train {
                                        funcs: vec![member, r],
                                        reverse: true,
                                        compose: true,
                                    };
                                    // Trailing fn after the compose: chain as
                                    // 2-train atop (e.g. `(…)⍛⊇ ∧` → the `∧`).
                                    let trailing = match self.peek().map(|t| &t.token) {
                                        Some(Token::Literal(LiteralValue::Symbol { name, .. })) => Self::is_primitive_op(name),
                                        Some(Token::OpenParen) | Some(Token::LambdaToken) | Some(Token::ApplyToken) | Some(Token::LeftForkToken) => true,
                                        _ => false,
                                    };
                                    if trailing {
                                        let next = self.parse_function_atom()?;
                                        member = Instr::Train {
                                            funcs: vec![member, next],
                                            reverse: false,
                                            compose: false,
                                        };
                                    }
                                }
                                Token::LeftForkToken => {
                                    self.advance();
                                    self.skip_newlines();
                                    let b = self.parse_function_atom()?;
                                    self.skip_newlines();
                                    self.expect(Token::RightForkToken, "expected » in fork")?;
                                    self.skip_newlines();
                                    let c = self.parse_function_atom()?;
                                    member = Instr::Train {
                                        funcs: vec![member, b, c],
                                        reverse: false,
                                        compose: false,
                                    };
                                }
                                _ => {}
                            }
                        }
                        cur = Instr::Train {
                            funcs: vec![cur, member],
                            reverse: false,
                            compose: false,
                        };
                    }
                    return Ok(cur);
                }
            }
        }
        Ok(left)
    }

    /// Parse a single *function atom* suitable as a train member: a symbol, a derived
    /// (adverb) operator, a lambda, a parenthesised train/group, a fork `A « B » C`,
    /// or a catenate `,`.
    fn parse_function_atom(&mut self) -> Result<Instr, AplError> {
        let t = self.peek().ok_or_else(|| self.err("expected a function in train"))?;
        match &t.token {
            Token::ApplyToken => {
                // `⍞name`: a dynamic function reference — the value bound to `name`
                // is used as a function. Parse the following symbol into a `DynamicRef`.
                self.advance();
                let tok = self.peek().ok_or_else(|| self.err("expected a symbol after ⍞"))?;
                match &tok.token {
                    Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                        let name = name.clone();
                        let namespace = namespace.clone();
                        self.advance();
                        Ok(Instr::DynamicRef { name, namespace })
                    }
                    _ => Err(self.err("expected a symbol after ⍞")),
                }
            }
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                let name = name.clone();
                let namespace = namespace.clone();
                self.advance();
                Ok(Instr::Symbol { name, namespace })
            }
            Token::Literal(LiteralValue::SymbolValue { name }) => {
                let name = name.clone();
                self.advance();
                Ok(Instr::SymbolValue { name })
            }
            Token::OpenParen => {
                // Nested train or group of functions.
                let open_pos = self.pos;
                self.advance();
                self.skip_newlines();
                let save = self.pos;
                if let Some(train) = self.parse_paren_vfn_chain() {
                    return Ok(train);
                }
                self.pos = save;
                if let Some(train) = self.try_parse_train() {
                    return Ok(train);
                }
                // Not a train — fall back to a single function expression, e.g.
                // `((f ⍞g)¨ arg)` inside a `⇐` RHS: the outer group holds one
                // derived function plus its DATA argument, which is an
                // application, not a train. parse_function_expr parses the
                // derived fn and leaves `arg` for the caller's apply loop.
                self.pos = save;
                self.parse_function_expr()
            }
            Token::Literal(lv) => {
                // A literal value is a valid train member (enables left-bind e.g. `(10 +)`).
                let lv = lv.clone();
                self.advance();
                Ok(Instr::Literal(lv))
            }
            Token::OpenBrace => {
                // A `{ … }` block used as a function expression (e.g. `foo ⇐ { … }`).
                // `parse_block` consumes the opening `{` and returns `Instr::Block { body }`.
                self.advance();
                self.parse_block()
            }
            Token::ComposeToken => {
                // `∘∙f` — the OUTER product surface form (Kotlin NullFunction left
                // operand of OuterInnerJoinOp, engine.kt:489). `∘` alone is also a
                // valid compose; only treat it as the null-fn sentinel when `∙`
                // follows immediately.
                self.advance(); // consume ∘
                if let Some(t) = self.peek() {
                    if matches!(&t.token, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "∙") {
                        self.advance(); // consume ∙
                        let r = self.parse_function_atom()?;
                        return Ok(Instr::InnerProduct { left_fn: None, right_fn: Box::new(r) });
                    }
                }
                let r = self.parse_function_atom()?;
                Ok(Instr::Train { funcs: vec![Instr::symbol("∘"), r], reverse: false, compose: true })
            }
            _ => Err(self.err("expected a function in train")),
        }
    }

    /// Whether an expression is a *function* suitable for a train operand.
    fn is_function_expr(e: &Instr) -> bool {
        // Keyword-namespaced symbols (`:export`, `:const`, `:local`) are *values*
        // (symbol-list elements for `declare`), never functions — a lone `(:export)`
        // or `:export` member must not be wrapped as a 1-train.
        if let Instr::Symbol { namespace, .. } = e {
            if namespace.as_deref() == Some("keyword") {
                return false;
            }
        }
        matches!(
            e,
            Instr::Symbol { .. }
                | Instr::Derived { .. }
                | Instr::OpCall { .. }
                | Instr::InnerProduct { .. }
                | Instr::Lambda { .. }
                | Instr::Train { .. }
                | Instr::ValueOp { .. }
                | // An axis-applied function (`⌽[0]`, `,[0.5]`) is a DERIVED
                  // FUNCTION value (Kotlin AxisValAssignedFunctionDirect) — it must
                  // count as a function so `⌽[0]˝` binds the inverse adverb and the
                  // __KOTLIN_FALLBACK__ guard does not fire on it.
                  Instr::AxisApplied { .. }
                // P1-M7: a `{…}` block IS a function value (Kotlin OpenFnDef →
                // processFn); required for `{2×⍵}¨ 1 2 3` to bind the each-adverb.
                | Instr::Block { .. }
        )
    }

    /// Whether a symbol NAME is a function atom for train/atop formation. A bare
    /// symbol is a function atom only if it is a primitive operator OR a
    /// user-defined / namespaced function known at parse time (e.g. `f ⇐ +`
    /// followed by `g ⇐ f h` — both `f` and `h` are functions, so `f h` is a
    /// 2-train ATOP, not `Apply{f, h}`). A plain *value* variable (e.g. `x` in
    /// `foo x`) is NOT a function atom and must stay a normal right-argument
    /// application (Kotlin's parseValue() returns InstrParseResult for variables,
    /// FnParseResult only for functions — parser.kt:479). `:keyword` symbols
    /// (`:export`, `:const`) are values, never functions.
    fn is_fn_atom_name(&self, name: &str, namespace: &Option<String>) -> bool {
        if namespace.as_deref() == Some("keyword") {
            return false;
        }
        Self::is_primitive_op(name) || self.is_known_fn(name, namespace)
    }

    /// Structural `declare(…)` parser (Kotlin DeclareToken). Consumes
    /// `declare ( <keyword> <target>? )` — already positioned AFTER the `declare`
    /// symbol — and returns `Array[keyword, target]` exactly as `eval_declare`
    /// expects. The target is a Symbol, a parenthesised list of Symbols (raw scan:
    /// NO train/operator classification, so fn-valued names stay plain symbols),
    /// or any other single token's primary (no-op per oracle tolerance).
    fn parse_declare_special(&mut self) -> Result<Instr, AplError> {
        self.skip_newlines();
        self.expect(Token::OpenParen, "expected '(' after declare")?;
        self.skip_newlines();
        // Directive keyword: a :keyword-namespaced symbol (`:export`, `:const`, …).
        let keyword = match self.peek() {
            Some(t) => match &t.token {
                Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                    let kw = name.clone();
                    debug_assert!(namespace.as_deref() == Some("keyword") || !kw.is_empty());
                    self.advance();
                    kw
                }
                _ => {
                    // `declare()` with no directive: consume through ')' as a no-op.
                    self.skip_to_close_paren()?;
                    return Ok(Instr::Array { elements: vec![] });
                }
            },
            None => return Err(self.err("unexpected end of input in declare")),
        };
        self.skip_newlines();
        // Target: either a bare Symbol or a `( name name … )` list scanned RAW.
        let target = match self.peek().map(|t| &t.token) {
            Some(Token::Literal(LiteralValue::Symbol { .. })) => {
                let t = self.peek().unwrap().token.clone();
                if let Token::Literal(LiteralValue::Symbol { name, namespace }) = t {
                    self.advance();
                    Instr::Symbol { name, namespace }
                } else {
                    unreachable!()
                }
            }
            Some(Token::OpenParen) => {
                self.advance(); // consume (
                let mut names = Vec::new();
                loop {
                    self.skip_newlines();
                    match self.peek() {
                        Some(t) if matches!(t.token, Token::CloseParen) => {
                            self.advance();
                            break;
                        }
                        Some(t) => match &t.token {
                            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                                names.push(Instr::Symbol {
                                    name: name.clone(),
                                    namespace: namespace.clone(),
                                });
                                self.advance();
                            }
                            _ => {
                                // Non-symbol member inside the list: skip the whole
                                // group raw (oracle tolerance for odd shapes).
                                self.skip_to_close_paren()?;
                                break;
                            }
                        },
                        None => return Err(self.err("unexpected end of input in declare list")),
                    }
                }
                Instr::Array { elements: names }
            }
            _ => Instr::Empty,
        };
        self.skip_newlines();
        self.expect(Token::CloseParen, "expected ')' after declare arguments")?;
        Ok(Instr::Array {
            elements: vec![
                Instr::Symbol {
                    name: keyword,
                    namespace: Some("keyword".to_string()),
                },
                target,
            ],
        })
    }

    /// Skip tokens to the matching close paren of the CURRENT nesting level
    /// (assumes the caller has consumed the opening paren of this level).
    fn skip_to_close_paren(&mut self) -> Result<(), AplError> {
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek() {
                Some(t) => match &t.token {
                    Token::OpenParen => {
                        depth += 1;
                        self.advance();
                    }
                    Token::CloseParen => {
                        depth -= 1;
                        self.advance();
                    }
                    _ => {
                        self.advance();
                    }
                },
                None => return Err(self.err("unexpected end of input: missing ')'")),
            }
        }
        Ok(())
    }

    /// Peek ahead `n` tokens without consuming.
    fn peek_at(&self, n: usize) -> Option<&SpannedToken> {
        self.toks.get(self.pos + n)
    }

    /// primary := number | char | string | symbol | ( expr ) | [ elements ] | ⍬
    fn parse_primary(&mut self) -> Result<Instr, AplError> {
        let t = self.peek().ok_or_else(|| self.err("unexpected end of input"))?;
        match &t.token {
            // `∘∙f` at PRIMARY position (outer product, e.g. `1 2 3 ∘∙× 3 4`):
            // Kotlin parses `∘` as NullFunction then binds OuterInnerJoinOp. Only
            // when `∙` immediately follows; otherwise fall through to the normal
            // symbol path (plain compose).
            Token::ComposeToken => {
                if let Some(t2) = self.peek_at(1).map(|t| &t.token) {
                    if matches!(t2, Token::Literal(LiteralValue::Symbol { name, .. }) if name == "∙") {
                        self.advance(); // consume ∘
                        self.advance(); // consume ∙
                        let r = self.parse_function_atom()?;
                        return Ok(Instr::InnerProduct { left_fn: None, right_fn: Box::new(r) });
                    }
                }
                // Not followed by ∙ — plain compose symbol; handled by the general path.
                self.advance();
                return Ok(Instr::symbol("∘"));
            }
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                let name = name.clone();
                let namespace = namespace.clone();
                // A registered `defsyntax` macro: expand inline (Kotlin `processCustomSyntax`).
                // Must be checked BEFORE the plain-symbol path, because a macro trigger is a
                // bare symbol that should consume its argument tokens per the rule list.
                if namespace.is_none() {
                    if let Some(m) = self.macros.get(&name) {
                        let m = m.clone();
                        self.advance(); // consume the trigger symbol
                        return self.expand_macro(&m);
                    }
                    // `declare(…)` is a STRUCTURAL special form (Kotlin DeclareToken →
                    // processExport): its argument is never evaluated and must NOT go
                    // through the train/operator classifiers. Parsing it via the normal
                    // apply path breaks on fn-valued members — `declare(:export (cols col))`
                    // with `cols ⇐ …` turns the inner list into a 2-train (both names are
                    // known functions), and the outer group then dies with "unexpected
                    // token in primary". Scan the paren group RAW instead and build the
                    // exact AST eval_declare expects: Array[keyword-symbol, target] where
                    // target is Symbol / Array-of-Symbols. (Oracle: declare(:export (f))
                    // → null.) Only fires when directly followed by '('.
                    if name == "declare"
                        && matches!(
                            self.peek_at(1).map(|t| &t.token),
                            Some(Token::OpenParen)
                        )
                    {
                        self.advance(); // consume 'declare'
                        return self.parse_declare_special();
                    }
                }
                self.advance();
                Ok(Instr::Symbol { name, namespace })
            }
            Token::Literal(LiteralValue::SymbolValue { name }) => {
                let name = name.clone();
                self.advance();
                Ok(Instr::SymbolValue { name })
            }
            Token::QuotePrefix => {
                // `'name` — a *symbol literal* (Kotlin QuotePrefix +
                // parser.kt:1003: `LiteralSymbol(nameToSymbol(tokeniser.nextTokenWithType()))`).
                // Consume the following symbol token (which may be namespace-qualified)
                // and yield a SymbolValue that evaluates to `APLValue::Symbol`.
                self.advance();
                match self.peek() {
                    Some(t) if matches!(
                        t.token,
                        Token::Literal(LiteralValue::Symbol { .. })
                    ) => {
                        let tok = t.token.clone();
                        self.advance();
                        if let Token::Literal(LiteralValue::Symbol { name, namespace }) = tok {
                            Ok(Instr::Literal(LiteralValue::SymbolValue {
                                name: match namespace {
                                    Some(ns) => format!("{}:{}", ns, name),
                                    None => name,
                                },
                            }))
                        } else {
                            unreachable!()
                        }
                    }
                    _ => Err(self.err("expected a symbol after '")),
                }
            }
            Token::ApplyToken => {
                // Kap's `⍞name`: a *dynamic* function reference. Unlike a plain symbol
                // reference (which strands into a vector when not applied), `⍞name` always
                // means "fetch the value bound to `name` *as a function* and apply it".
                self.advance();
                self.skip_newlines();
                match self.peek() {
                    Some(t) if matches!(
                        t.token,
                        Token::Literal(LiteralValue::Symbol { .. })
                    ) => {
                        let tok = t.token.clone();
                        self.advance();
                        if let Token::Literal(LiteralValue::Symbol { name, namespace }) = tok {
                            let dr = Instr::DynamicRef { name, namespace };
                            // A trailing adverb binds the dynamic ref as the derived
                            // function's operand: `⍞fn¨ arr` = each over ⍞fn.
                            if let Some(Token::Literal(LiteralValue::Symbol { name: adv, .. })) =
                                self.peek().map(|t| &t.token)
                            {
                                if Self::is_adverb(adv) {
                                    let adv = adv.clone();
                                    self.advance();
                                    return Ok(Instr::Derived {
                                        func: Box::new(dr),
                                        op: Box::new(Instr::Symbol {
                                            name: adv,
                                            namespace: None,
                                        }),
                                    });
                                }
                            }
                            Ok(dr)
                        } else {
                            unreachable!()
                        }
                    }
                    _ => Err(self.err("expected a function name after ⍞")),
                }
            }
            Token::Literal(lv) => {
                let lv = lv.clone();
                self.advance();
                Ok(Instr::Literal(lv))
            }
            Token::OpenParen => {
                self.advance();
                self.skip_newlines();
                // A parenthesised *sequence of >=2 functions* is a train: `(f g h)`.
                // Try the value/fn accumulator first (Kotlin inner parseExpr — handles the
                // value-left-bind-to-fn-chain case, e.g. `(¯1r2 0+2÷⍨≢)`); if it doesn't
                // pan out, fall back to a single function expression (e.g. `(×∘-)`,
                // `(+ « - »)`, which is a derived operator), then finally a normal group `(expr)`.
                let save = self.pos;
                if let Some(train) = self.parse_paren_vfn_chain() {
                    return Ok(train);
                }
                self.pos = save;
                if let Some(train) = self.try_parse_train() {
                    return Ok(train);
                }
                self.pos = save;
                if let Ok(func) = self.parse_function_expr() {
                    self.skip_newlines();
                    if let Some(t) = self.peek() {
                        if matches!(t.token, Token::CloseParen) {
                            self.advance();
                            return Ok(func);
                        }
                    }
                }
                self.pos = save;
                let e = self.parse_expr()?;
                self.skip_newlines();
                // A `;` between operands inside a group is a vector literal: `(10;20;30)`.
                // (Kap uses `;` as the list separator in vectors and function argument lists.)
                if matches!(self.peek().map(|t| &t.token), Some(Token::ListSeparator)) {
                    let mut elems = vec![e];
                    while matches!(self.peek().map(|t| &t.token), Some(Token::ListSeparator)) {
                        self.advance();
                        self.skip_newlines();
                        elems.push(self.parse_expr()?);
                        self.skip_newlines();
                    }
                    match self.peek() {
                        Some(t) if matches!(t.token, Token::CloseParen) => {
                            self.advance();
                            return Ok(Instr::Array { elements: elems });
                        }
                        _ => return Err(self.err("expected ')' after ';'-separated vector")),
                    }
                }
                // A `⋄` (statement separator) inside the group makes it a statement sequence
                // `(a ⋄ b ⋄ c)` — like a block, returns the value of the last statement.
                if matches!(self.peek().map(|t| &t.token), Some(Token::StatementSeparator)) {
                    let mut body = vec![e];
                    while matches!(
                        self.peek().map(|t| &t.token),
                        Some(Token::StatementSeparator)
                    ) {
                        self.advance();
                        self.skip_newlines();
                        body.push(self.parse_expr()?);
                        self.skip_newlines();
                    }
                    match self.peek() {
                        Some(t) if matches!(t.token, Token::CloseParen) => {
                            self.advance();
                            return Ok(Instr::Block { body });
                        }
                        _ => return Err(self.err("expected ')' after '⋄'-separated group")),
                    }
                }
                match self.peek() {
                    Some(t) if matches!(t.token, Token::CloseParen) => {
                        self.advance();
                        Ok(e)
                    }
                    _ => Err(self.err("expected ')'")),
                }
            }
            Token::OpenBrace => {
                self.advance();
                self.parse_block()
            }
            Token::OpenBracket => {
                self.advance();
                let mut elements = Vec::new();
                loop {
                    self.skip_newlines();
                    if let Some(t) = self.peek() {
                        if matches!(t.token, Token::CloseBracket) {
                            self.advance();
                            break;
                        }
                        if matches!(t.token, Token::ListSeparator) {
                            self.advance();
                            continue;
                        }
                    } else {
                        return Err(self.err("expected ']'"));
                    }
                    let e = self.parse_expr()?;
                    elements.push(e);
                }
                Ok(Instr::Array { elements })
            }
            Token::APLNullSym => {
                self.advance();
                Ok(Instr::Empty)
            }
            Token::LambdaToken => {
                // λ(params) body  — params are bare symbols (or a parenthesised list),
                // body is the rest of the expression.
                self.advance();
                self.skip_newlines();
                let mut params = Vec::new();
                // optional parenthesised parameter list: (a b c) or (a,b,c)
                if let Some(t) = self.peek() {
                    if matches!(t.token, Token::OpenParen) {
                        self.advance();
                        loop {
                            self.skip_newlines();
                            match self.peek() {
                                Some(t) if matches!(t.token, Token::CloseParen) => {
                                    self.advance();
                                    break;
                                }
                                Some(t) if matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) => {
                                    if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                                        if name == "," {
                                            // `,` is the catenate separator between params
                                            self.advance();
                                        } else {
                                            params.push(name.clone());
                                            self.advance();
                                        }
                                    } else {
                                        unreachable!()
                                    }
                                }
                                _ => return Err(self.err("expected parameter name or ')'")),
                            }
                        }
                    } else if let Some(t) = self.peek() {
                        if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                            // single unparenthesised param: λx x*2
                            params.push(name.clone());
                            self.advance();
                        }
                    }
                }
                self.skip_newlines();
                let body = self.parse_expr()?;
                Ok(Instr::Lambda {
                    params,
                    body: Box::new(body),
                })
            }
            _ => {
                Err(self.err("unexpected token in primary"))
            }
        }
    }

    /// Consume a trailing `[...]` index selector (Kap array *pick* / selection) and wrap
    /// `base` into `Instr::Index`. Repeats for chained `a[i][j]`.
    ///
    /// The bracket content is parsed as `;`-separated axis *sections*. Each section is
    /// stored as one element of an `Instr::Array` selector:
    ///   - an empty section (`[2;]` second part, or `⍬`) -> `Instr::Empty` (means "all" along that axis)
    ///   - otherwise the parsed expression: a scalar index, or a strand/vector of indices
    ///     (e.g. `[0 2]` -> one section that is the vector `(0 2)`, NOT `((0 2))`).
    /// A single section with no `;` is the 1-D pick form (`a[i]`, `a[i j]`); multiple
    /// sections (`a[r;c]`) are multi-axis selection. Binds more tightly than stranding
    /// and any dyadic operator, so `a b (c d)[0] e` indexes `(c d)`, not the whole strand.
    fn parse_index_suffix(&mut self, mut base: Instr) -> Result<Instr, AplError> {
        loop {
            let is_open_bracket =
                matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBracket));
            if !is_open_bracket {
                break;
            }
            self.advance();
            let mut sections: Vec<Instr> = Vec::new();
            let mut current: Option<Instr> = None;
            loop {
                self.skip_newlines();
                match self.peek() {
                    Some(t) if matches!(t.token, Token::CloseBracket) => {
                        self.advance();
                        sections.push(current.take().unwrap_or(Instr::Empty));
                        break;
                    }
                    Some(t) if matches!(t.token, Token::ListSeparator) => {
                        self.advance();
                        sections.push(current.take().unwrap_or(Instr::Empty));
                        continue;
                    }
                    _ => {
                        // `parse_expr` consumes a whole strand (`0 2` -> one Array instr),
                        // so a section is exactly one instr.
                        let e = self.parse_expr()?;
                        current = Some(e);
                    }
                }
            }
            let selector_instr = Instr::Array { elements: sections };
            base = Instr::Index {
                array: Box::new(base),
                selector: Box::new(selector_instr),
            };
        }
        Ok(base)
    }

    /// Detect `defsyntax name (rules…) { body }` / `defsyntaxsub name (rules…) { body }`
    /// (Kotlin `processDefsyntax`/`processDefsyntaxSub`) and parse accordingly. The Kotlin
    /// form puts the keyword FIRST: `defsyntax triggerName (rules) { body }`. Returns `None`
    /// if the current position is not a defsyntax directive.
    fn parse_defsyntax_directive(&mut self) -> Result<Option<Instr>, AplError> {
        self.skip_newlines();
        let kw = match self.peek() {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        let is_directive = match &kw.token {
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                (name == "defsyntax" || name == "defsyntaxsub") && namespace.is_none()
            }
            _ => false,
        };
        if !is_directive {
            return Ok(None);
        }
        let is_sub = match &kw.token {
            Token::Literal(LiteralValue::Symbol { name, .. }) => name == "defsyntaxsub",
            _ => false,
        };
        self.advance(); // consume the keyword
        // Next symbol is the macro trigger name.
        self.skip_newlines();
        let trigger_tok = match self.peek() {
            Some(t) => t.clone(),
            None => return Err(self.err("expected a macro trigger name after defsyntax")),
        };
        let (trigger, ns) = match &trigger_tok.token {
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                (name.clone(), namespace.clone())
            }
            _ => return Err(self.err("expected a symbol as the macro trigger name")),
        };
        self.advance(); // consume the trigger name
        let rules = self.parse_syntax_rules()?;
        self.skip_newlines();
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
            return Err(self.err("expected '{' before defsyntax body"));
        }
        self.advance(); // consume the opening `{` (parse_block assumes it is gone)
        let body = self.parse_block()?;
        if is_sub {
            Ok(Some(Instr::DefSyntaxSub {
                name: trigger,
                namespace: ns,
                rules,
                body: std::rc::Rc::new(body),
            }))
        } else {
            Ok(Some(Instr::DefSyntax {
                name: trigger,
                namespace: ns,
                rules,
                body: std::rc::Rc::new(body),
            }))
        }
    }

    /// Parse a `(rule rule …)` syntax-rule list (Kotlin `processPairs`). Each rule is a
    /// `:keyword name` pair (or `:special :token`). The parser is currently positioned just
    /// *after* the opening `(` of the rule list.
    fn parse_syntax_rules(&mut self) -> Result<Vec<SyntaxRule>, AplError> {
        if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
            return Err(self.err("expected '(' for syntax rule list"));
        }
        self.advance();
        let mut rules = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if matches!(t.token, Token::CloseParen) => {
                    self.advance();
                    break;
                }
                Some(t) => {
                    if let Token::Literal(LiteralValue::Symbol { name, namespace }) = &t.token {
                        if namespace.as_deref() != Some("keyword") {
                            return Err(self.err("syntax rule tag must be a :keyword"));
                        }
                        let tag = name.clone();
                        self.advance();
                        match tag.as_str() {
                            "function" | "nfunction" | "exprfunction" | "nexprfunction" => {
                                let var = self.expect_keyword_var()?;
                                let rule = match tag.as_str() {
                                    "function" => SyntaxRule::Function { var },
                                    "nfunction" => SyntaxRule::NFunction { var },
                                    "exprfunction" => SyntaxRule::ExprFunction { var },
                                    _ => SyntaxRule::NExprFunction { var },
                                };
                                rules.push(rule);
                            }
                            "value" | "string" => {
                                let var = self.expect_keyword_var()?;
                                rules.push(if tag == "value" {
                                    SyntaxRule::Value { var }
                                } else {
                                    SyntaxRule::String { var }
                                });
                            }
                            "special" => {
                                let sp = self.expect_keyword_var()?;
                                let token = match sp.as_str() {
                                    "openBrace" => SpecialToken::OpenBrace,
                                    "closeBrace" => SpecialToken::CloseBrace,
                                    "newline" => SpecialToken::Newline,
                                    other => return Err(self.err(&format!("unknown special token: {}", other))),
                                };
                                rules.push(SyntaxRule::Special { token });
                            }
                            "optional" => {
                                let inner = self.parse_syntax_rules()?;
                                rules.push(SyntaxRule::Optional { inner });
                            }
                            "repeat" => {
                                if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
                                    return Err(self.err("expected '(' after :repeat"));
                                }
                                self.advance();
                                self.skip_newlines();
                                let var = self.expect_keyword_var()?;
                                self.skip_newlines();
                                let sub = match self.peek() {
                                    Some(t) => match &t.token {
                                        Token::Literal(LiteralValue::Symbol { name, .. }) => {
                                            let s = name.clone();
                                            self.advance();
                                            s
                                        }
                                        _ => return Err(self.err("expected sub-macro name in :repeat")),
                                    },
                                    None => return Err(self.err("expected sub-macro name in :repeat")),
                                };
                                self.skip_newlines();
                                if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
                                    return Err(self.err("expected ')' after :repeat"));
                                }
                                self.advance();
                                rules.push(SyntaxRule::Repeat { var, sub });
                            }
                            other => return Err(self.err(&format!("unknown syntax rule tag: {}", other))),
                        }
                    } else {
                        return Err(self.err("expected a :keyword syntax rule tag"));
                    }
                }
                None => return Err(self.err("unexpected end of input in syntax rule list")),
            }
        }
        Ok(rules)
    }

    /// Expect a bare symbol and return its name (the bound var of a rule tag).
    fn expect_keyword_var(&mut self) -> Result<String, AplError> {
        self.skip_newlines();
        match self.peek() {
            Some(t) => match &t.token {
                Token::Literal(LiteralValue::Symbol { name, .. }) => {
                    let n = name.clone();
                    self.advance();
                    Ok(n)
                }
                _ => Err(self.err("expected a variable name in syntax rule")),
            },
            None => Err(self.err("expected a variable name in syntax rule")),
        }
    }

    /// Expand a registered macro at the current position (Kotlin `processCustomSyntax`).
    /// The trigger symbol has *already* been consumed; we now consume the rule list's
    /// tokens, binding each rule's var to a parsed `Instr`, and return
    /// `MacroExpand { body, bindings }`.
    fn expand_macro(&mut self, m: &SyntaxMacro) -> Result<Instr, AplError> {
        let mut bindings: Vec<(String, Box<Instr>)> = Vec::new();
        for rule in &m.rules {
            match rule {
                SyntaxRule::Function { var } | SyntaxRule::NFunction { var } => {
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
                        return Err(self.err(&format!("expected '{{' for :function rule '{}'", var)));
                    }
                    self.advance(); // consume the opening `{` (parse_block assumes it is gone)
                    let body = self.parse_block()?;
                    bindings.push((
                        var.clone(),
                        Box::new(Instr::Lambda { params: vec![], body: Box::new(body) }),
                    ));
                }
                SyntaxRule::ExprFunction { var } | SyntaxRule::NExprFunction { var } => {
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
                        return Err(self.err(&format!("expected '(' for :exprfunction rule '{}'", var)));
                    }
                    self.advance();
                    let inner = self.parse_expr()?;
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
                        return Err(self.err(&format!("expected ')' after :exprfunction rule '{}'", var)));
                    }
                    self.advance();
                    bindings.push((var.clone(), Box::new(inner)));
                }
                SyntaxRule::Value { var } => {
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
                        return Err(self.err(&format!("expected '(' for :value rule '{}'", var)));
                    }
                    self.advance();
                    let inner = self.parse_expr()?;
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
                        return Err(self.err(&format!("expected ')' after :value rule '{}'", var)));
                    }
                    self.advance();
                    bindings.push((var.clone(), Box::new(inner)));
                }
                SyntaxRule::String { var } => {
                    self.skip_newlines();
                    match self.peek() {
                        Some(t) => match &t.token {
                            Token::Literal(LiteralValue::Str(s)) => {
                                let s = s.clone();
                                self.advance();
                                bindings.push((
                                    var.clone(),
                                    Box::new(Instr::Literal(LiteralValue::Str(s))),
                                ));
                            }
                            _ => return Err(self.err(&format!("expected a string for :string rule '{}'", var))),
                        },
                        None => return Err(self.err(&format!("expected a string for :string rule '{}'", var))),
                    }
                }
                SyntaxRule::Special { token } => {
                    self.skip_newlines();
                    let expected = match token {
                        SpecialToken::OpenBrace => Token::OpenBrace,
                        SpecialToken::CloseBrace => Token::CloseBrace,
                        SpecialToken::Newline => Token::Newline,
                    };
                    match self.peek() {
                        Some(t) if t.token == expected => {
                            self.advance();
                        }
                        _ => return Err(self.err("syntax rule :special token mismatch")),
                    }
                }
                SyntaxRule::Optional { inner } => {
                    if self.optional_matches(inner) {
                        for r in inner {
                            self.apply_optional_rule(r)?;
                        }
                    }
                }
                SyntaxRule::Repeat { var, sub } => {
                    // Sub-macros are registered globally (see DefSyntaxSub arm), so look the
                    // bare name up in the parser's macro table.
                    let sub_macro = match self.macros.get(sub) {
                        Some(s) => s.clone(),
                        None => return Err(self.err(&format!("unknown sub-macro '{}' in :repeat", sub))),
                    };
                    let mut results: Vec<Instr> = Vec::new();
                    while self.sub_macro_matches(&sub_macro) {
                        results.push(self.expand_sub_macro(&sub_macro)?);
                    }
                    bindings.push((
                        var.clone(),
                        Box::new(Instr::Array { elements: results }),
                    ));
                }
            }
        }
        Ok(Instr::MacroExpand {
            body: m.body.clone(),
            bindings,
        })
    }

    /// Whether the next token(s) begin a match for the head rule of `inner`.
    fn optional_matches(&self, inner: &[SyntaxRule]) -> bool {
        match inner.first() {
            Some(SyntaxRule::Function { .. }) | Some(SyntaxRule::NFunction { .. }) => {
                matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace))
            }
            Some(SyntaxRule::Value { .. })
            | Some(SyntaxRule::ExprFunction { .. })
            | Some(SyntaxRule::NExprFunction { .. }) => {
                matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen))
            }
            Some(SyntaxRule::String { .. }) => {
                matches!(self.peek(), Some(t) if matches!(t.token, Token::Literal(LiteralValue::Str(_))))
            }
            Some(SyntaxRule::Special { token }) => {
                let expected = match token {
                    SpecialToken::OpenBrace => Token::OpenBrace,
                    SpecialToken::CloseBrace => Token::CloseBrace,
                    SpecialToken::Newline => Token::Newline,
                };
                matches!(self.peek(), Some(t) if t.token == expected)
            }
            _ => false,
        }
    }

    /// Apply a single optional inner rule, consuming its tokens (bindings are discarded).
    fn apply_optional_rule(&mut self, rule: &SyntaxRule) -> Result<(), AplError> {
        match rule {
            SyntaxRule::Function { var } | SyntaxRule::NFunction { var } => {
                self.skip_newlines();
                if matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
                    self.advance(); // consume the opening `{` (parse_block assumes it is gone)
                }
                let _ = self.parse_block()?;
                let _ = var;
                Ok(())
            }
            SyntaxRule::Value { var } | SyntaxRule::ExprFunction { var } | SyntaxRule::NExprFunction { var } => {
                self.skip_newlines();
                if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
                    return Err(self.err(&format!("expected '(' in optional :value rule '{}'", var)));
                }
                self.advance();
                let _ = self.parse_expr()?;
                self.skip_newlines();
                if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
                    return Err(self.err(&format!("expected ')' in optional :value rule '{}'", var)));
                }
                self.advance();
                Ok(())
            }
            SyntaxRule::String { var } => {
                self.skip_newlines();
                match self.peek() {
                    Some(t) => match &t.token {
                        Token::Literal(LiteralValue::Str(_)) => {
                            self.advance();
                            Ok(())
                        }
                        _ => Err(self.err(&format!("expected string in optional :string '{}'", var))),
                    },
                    None => Err(self.err(&format!("expected string in optional :string '{}'", var))),
                }
            }
            SyntaxRule::Special { token } => {
                let expected = match token {
                    SpecialToken::OpenBrace => Token::OpenBrace,
                    SpecialToken::CloseBrace => Token::CloseBrace,
                    SpecialToken::Newline => Token::Newline,
                };
                if !matches!(self.peek(), Some(t) if t.token == expected) {
                    return Err(self.err("optional :special token mismatch"));
                }
                self.advance();
                Ok(())
            }
            SyntaxRule::Optional { .. } | SyntaxRule::Repeat { .. } => Ok(()),
        }
    }

    /// Whether the next token begins a match for the sub-macro (its head rule).
    fn sub_macro_matches(&self, sub: &SyntaxMacro) -> bool {
        match sub.rules.first() {
            Some(r) => self.optional_matches(std::slice::from_ref(r)),
            None => false,
        }
    }

    /// Expand a sub-macro once (used by `:repeat`). The sub-macro's rules form one entry;
    /// the result is a `MacroExpand` pair that yields a 2-vector like `(cond fn)`.
    fn expand_sub_macro(&mut self, sub: &SyntaxMacro) -> Result<Instr, AplError> {
        let mut bindings: Vec<(String, Box<Instr>)> = Vec::new();
        for rule in &sub.rules {
            match rule {
                SyntaxRule::Function { var } | SyntaxRule::NFunction { var } => {
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
                        return Err(self.err(&format!(
                            "expected '{{' for :function rule '{}'",
                            var
                        )));
                    }
                    self.advance(); // consume the opening `{` (parse_block assumes it is gone)
                    let body = self.parse_block()?;
                    bindings.push((
                        var.clone(),
                        Box::new(Instr::Lambda {
                            params: vec![],
                            body: Box::new(body),
                        }),
                    ));
                }
                SyntaxRule::Value { var } | SyntaxRule::ExprFunction { var } | SyntaxRule::NExprFunction { var } => {
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenParen)) {
                        return Err(self.err(&format!("expected '(' in sub :value rule '{}'", var)));
                    }
                    self.advance();
                    let inner = self.parse_expr()?;
                    self.skip_newlines();
                    if !matches!(self.peek(), Some(t) if matches!(t.token, Token::CloseParen)) {
                        return Err(self.err(&format!("expected ')' in sub :value rule '{}'", var)));
                    }
                    self.advance();
                    bindings.push((var.clone(), Box::new(inner)));
                }
                SyntaxRule::String { var } => {
                    self.skip_newlines();
                    match self.peek() {
                        Some(t) => match &t.token {
                            Token::Literal(LiteralValue::Str(s)) => {
                                let s = s.clone();
                                self.advance();
                                bindings.push((
                                    var.clone(),
                                    Box::new(Instr::Literal(LiteralValue::Str(s))),
                                ));
                            }
                            _ => return Err(self.err(&format!("expected string in sub :string '{}'", var))),
                        },
                        None => return Err(self.err(&format!("expected string in sub :string '{}'", var))),
                    }
                }
                SyntaxRule::Special { token } => {
                    let expected = match token {
                        SpecialToken::OpenBrace => Token::OpenBrace,
                        SpecialToken::CloseBrace => Token::CloseBrace,
                        SpecialToken::Newline => Token::Newline,
                    };
                    if !matches!(self.peek(), Some(t) if t.token == expected) {
                        return Err(self.err("sub :special token mismatch"));
                    }
                    self.advance();
                }
                SyntaxRule::Optional { inner } => {
                    if self.optional_matches(inner) {
                        for r in inner {
                            self.apply_optional_rule(r)?;
                        }
                    }
                }
                SyntaxRule::Repeat { .. } => {}
            }
        }
        Ok(Instr::MacroExpand {
            body: sub.body.clone(),
            bindings,
        })
    }
}

/// Build a Symbol Instr from a token's LiteralValue::Symbol.
fn instr_from_symbol(tok: &Token) -> Instr {
    if let Token::Literal(LiteralValue::Symbol { name, namespace }) = tok {
        Instr::Symbol {
            name: name.clone(),
            namespace: namespace.clone(),
        }
    } else {
        Instr::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenise;
    use crate::ast::{Instr, BooleanOpKind};

    fn parse_one(src: &str) -> Instr {
        let toks = tokenise(src);
        let (stmts, errs) = parse(&toks, &[], &[], &std::collections::HashMap::new());
        assert!(errs.is_empty(), "parse errors for {:?}: {:?}", src, errs);
        assert_eq!(stmts.len(), 1, "expected one statement for {:?}", src);
        stmts.into_iter().next().unwrap()
    }

    #[test]
    fn parse_number_literal() {
        assert!(matches!(parse_one("42"), Instr::Literal(LiteralValue::Number(_))));
    }

    #[test]
    fn parse_symbol() {
        assert!(matches!(parse_one("foo"), Instr::Symbol { name, namespace: None } if name == "foo"));
    }

    #[test]
    fn parse_dyadic_apply() {
        match parse_one("1 + 2") {
            Instr::Apply { fn_expr, left, right } => {
                assert!(matches!(*fn_expr, Instr::Symbol { name, .. } if name == "+"));
                assert!(matches!(*left.unwrap(), Instr::Literal(_)));
                assert!(matches!(*right, Instr::Literal(_)));
            }
            other => panic!("expected Apply, got {:?}", other),
        }
    }

    #[test]
    fn parse_parenthesised() {
        match parse_one("(1+2)*3") {
            Instr::Apply { fn_expr, left, right } => {
                assert!(matches!(*fn_expr, Instr::Symbol { name, .. } if name == "*"));
                assert!(matches!(*left.unwrap(), Instr::Apply { .. }));
                assert!(matches!(*right, Instr::Literal(_)));
            }
            other => panic!("expected Apply, got {:?}", other),
        }
    }

    #[test]
    fn parse_assignment() {
        match parse_one("a ← 5") {
            Instr::Assign { target, value } => {
                assert!(matches!(*target, Instr::Symbol { name, .. } if name == "a"));
                assert!(matches!(*value, Instr::Literal(_)));
            }
            other => panic!("expected Assign, got {:?}", other),
        }
    }

    #[test]
    fn parse_empty_param_group_errors_not_panics() {
        // `∇ (a;b) () (c;d) { … }` has an empty `()` parameter group, which is invalid
        // (Kotlin errors with `Unexpected token: CloseParen`). The parser must return a
        // parse error, not panic on an out-of-bounds index (regression: parser.rs:559).
        let toks = tokenise("∇ (a;b) () (c;d) { a+b+c+d }");
        let (_stmts, errs) = parse(&toks, &[], &[], &std::collections::HashMap::new());
        assert!(
            !errs.is_empty(),
            "empty parameter group should produce a parse error, not panic"
        );
    }

    #[test]
    fn parse_two_disclose_groups_strand() {
        // `(⌷x) (⌷y)` is two adjacent value groups (⌷x = disclose(x), a value), NOT a
        // function train — they must strand into a 2-element vector, not be applied.
        match parse_one("∇ foo { (⌷x) (⌷y) }") {
            Instr::UserFnDef { .. } => {}
            other => panic!("expected UserFnDef, got {:?}", other),
        }
        // A genuine 2-train of definite functions still parses as a train.
        match parse_one("foo ⇐ (×-)") {
            Instr::FnAssign { .. } => {}
            other => panic!("expected FnAssign, got {:?}", other),
        }
    }

    #[test]
    fn parse_array_literal() {
        match parse_one("[1;2;3]") {
            Instr::Array { elements } => assert_eq!(elements.len(), 3),
            other => panic!("expected Array, got {:?}", other),
        }
    }
}
