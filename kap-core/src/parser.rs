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

use crate::ast::Instr;
use crate::token::{LiteralValue, SpannedToken, Token};
use crate::AplError;

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
) -> (Vec<Instr>, Vec<AplError>) {
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        known_functions: known_functions.iter().map(|s| s.to_string()).collect(),
        known_ops: known_ops.iter().map(|s| s.to_string()).collect(),
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
    fn is_known_fn(&self, name: &str) -> bool {
        self.known_functions.iter().any(|n| n == name)
    }
    /// Whether an RHS `Instr` is a function value (so `name ← <rhs>` should define a function).
    fn is_function_value(&self, v: &Instr) -> bool {
        match v {
            Instr::Lambda { .. }
            | Instr::Train { .. }
            | Instr::Derived { .. }
            | Instr::Block { .. } => true,
            // a symbol that is a known/primitive function name also defines a function
            Instr::Symbol { name, .. } => self.is_known_fn(name) || Self::is_primitive_op(name),
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
        let instr = self.parse_expr()?;
        self.skip_newlines();
        if let Some(t) = self.peek() {
            if matches!(t.token, Token::StatementSeparator) {
                self.advance();
            }
        }
        Ok(Some(instr))
    }

    /// Expect the next token to be `tok`; consume it or return a parse error.
    fn expect(&mut self, tok: Token, msg: &str) -> Result<(), AplError> {
        match self.peek() {
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
        // Detect a leading keyword symbol and dispatch to the dedicated parser.
        if let Some(t) = self.peek() {
            if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                match name.as_str() {
                    "if" => return self.parse_if(),
                    "while" => return self.parse_while(),
                    "when" => return self.parse_when(),
                    _ => {}
                }
            }
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
                    let stmt = self.parse_expr()?;
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
        self.parse_apply()
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
        // The RHS is a *function expression*: a lambda, a named function, an operator
        // (possibly with adverbs, e.g. `×/`), a train, or a parenthesised function group.
        // Parse it as a function expression (not `parse_apply`, which would over-consume
        // a trailing data operand) and store the resulting function value.
        let value = self.parse_function_expr()?;
        // Validation: a primitive operator/function name cannot be reassigned to a function,
        // and the RHS must be a function, not a value.
        if Self::is_primitive_op(&name) {
            return Err(self.err(&format!("cannot redefine primitive function '{}'", name)));
        }
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
        // Register the name so a later body / mutual reference treats it as a function.
        if !self.known_functions.iter().any(|n| n == &name) {
            self.known_functions.push(name.clone());
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
        let mut components: Vec<Vec<String>> = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if matches!(t.token, Token::OpenBrace) => break,
                Some(t) if matches!(t.token, Token::Literal(LiteralValue::Symbol { .. })) => {
                    if let Token::Literal(LiteralValue::Symbol { name, .. }) = &t.token {
                        components.push(vec![name.clone()]);
                    }
                    self.advance();
                }
                Some(t) if matches!(t.token, Token::OpenParen) => {
                    self.advance();
                    let mut params = Vec::new();
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
                                if matches!(t.token, Token::Comma)
                                    || matches!(t.token, Token::ListSeparator) =>
                            {
                                self.advance();
                            }
                            _ => return Err(self.err("expected parameter name or ')'")),
                        }
                    }
                    components.push(params);
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
            1 => (components[0][0].clone(), None, None, vec![], vec![]),
            2 => {
                if components[0].len() >= 2 {
                    let nc = &components[0];
                    let op_left = Some(nc[0].clone());
                    let name = nc[1].clone();
                    let op_right = nc.get(2).cloned();
                    (name, op_left, op_right, vec![], components[1].clone())
                } else {
                    (components[0][0].clone(), None, None, vec![], components[1].clone())
                }
            }
            3 => {
                let nc = &components[1];
                if nc.len() >= 2 {
                    let op_left = Some(nc[0].clone());
                    let name = nc[1].clone();
                    let op_right = nc.get(2).cloned();
                    (name, op_left, op_right, components[0].clone(), components[2].clone())
                } else {
                    (nc[0].clone(), None, None, components[0].clone(), components[2].clone())
                }
            }
            _ => return Err(self.err("invalid function definition format")),
        };
        let is_op = op_left.is_some();
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
        // Validation (Kap semantics):
        //  * a native (primitive) operator/function name cannot be redefined.
        //  * parameter names must be distinct (no duplicated arguments).
        if Self::is_primitive_op(&name) {
            return Err(self.err(&format!("cannot redefine primitive function '{}'", name)));
        }
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
                        Some(t) if matches!(t.token, Token::Comma) || matches!(t.token, Token::ListSeparator) => {
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
            Instr::Symbol { name, .. } => {
                Self::is_primitive_op(name) || self.is_known_fn(name) || self.known_ops.iter().any(|n| n == name)
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
        // A leading symbol is parsed as a MONADIC application `f x` when:
        //   * it is a primitive AND the next token is a plain operand, or another primitive
        //     (train: `⊃ ⍳5` = `⊃(⍳5)`, `≢ ⍳5` = `≢(⍳5)`); or
        //   * it is a *user* symbol (variable/function) AND the next token is a plain operand
        //     (`f 5`). When the next token is an OPERATOR (e.g. `x + 1`), a user symbol is NOT
        //     monadic — it is the LEFT operand of the dyadic operator, so we fall through.
        // Valence is ultimately resolved at runtime; this is a syntactic heuristic.
        if let Instr::Symbol { name, .. } = &first {
            let is_prim = Self::is_primitive_op(name);
            // A leading symbol is a *function* (and thus eligible for monadic apply `f x`)
            // only if it is a primitive or a known (user/native) function. Otherwise it is a
            // value and must strand (`a c` -> (a c)) rather than apply (`a(c)`).
            let is_known = self.is_known_fn(name);
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
                        )
                }
                None => false,
            };
            let next_is_primitive = match &next {
                Some(Token::Literal(LiteralValue::Symbol { name: nn, .. })) => Self::is_primitive_op(nn),
                _ => false,
            };
            let next_is_adverb = match &next {
                Some(Token::Literal(LiteralValue::Symbol { name: nn, .. })) => {
                    Self::is_adverb(nn)
                }
                _ => false,
            };
            let do_monadic = if is_prim {
                // A leading primitive followed by an adverb (`+/`, `×¨`, `⍟\\`) is the
                // *function-then-operator* form, NOT monadic application: it is a derived
                // function `func op` applied to the following data.
                (next_is_operand_not_fn || next_is_primitive) && !next_is_adverb
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
            // and apply it to the following data operand.
            if is_prim && next_is_adverb {
                let op = self.parse_primary()?; // consume the adverb symbol
                let data = self.parse_apply()?;
                return Ok(Instr::Apply {
                    fn_expr: Box::new(Instr::Derived {
                        func: Box::new(first),
                        op: Box::new(op),
                    }),
                    left: None,
                    right: Box::new(data),
                });
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
                if self.next_is_function_token() {
                    let func = self.parse_function_expr()?;
                    let right = self.parse_apply()?;
                    return Ok(Instr::Apply {
                        fn_expr: Box::new(func),
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
        let mut strand: Option<Vec<Instr>> = None;
        let first = self.parse_index_suffix(first)?;
        let mut left = first;
        loop {
            // Stop at a statement boundary: newlines separate statements at the top
            // level, so `1 2 3` does not strand across into the next line's tokens.
            // Check BEFORE skipping newlines so we see the boundary.
            if self.at_statement_boundary() {
                break;
            }
            self.skip_newlines();
            if self.at_statement_boundary() {
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
            // Stop at a statement boundary: `a + b` on its own line is not the left
            // operand of a dyadic operator starting the next statement. Check BEFORE
            // skipping newlines so we see the boundary.
            if self.at_statement_boundary() {
                break;
            }
            self.skip_newlines();
            if self.at_statement_boundary() {
                break;
            }
            let paren_op = self.next_is_paren_operator();
            let is_operator = match self.peek() {
                Some(t) => match &t.token {
                    // A bare symbol is an operator only if it's a primitive or a
                    // known (user/native) function. Otherwise it's a value and must
                    // strand (`a c`) rather than apply (`a c` -> a(c)).
                    Token::Literal(LiteralValue::Symbol { name, .. }) => {
                        Self::is_primitive_op(name) || self.is_known_fn(name)
                    }
                    Token::OpenParen => paren_op,
                    Token::OpenBracket => true,
                    Token::LambdaToken => true,
                    Token::Comma => true,
                    _ => false,
                },
                None => false,
            };
            if !is_operator {
                break;
            }
            let fn_expr = self.parse_primary()?; // the operator (symbol or parenthesised)
            self.skip_newlines();
            // Detect `func adverb` immediately after the operator (e.g. `×¨` in
            // `2 ×¨ 3 4 5`): the operator is a function-primitive and the very next
            // token is an adverb. Bind them into a derived function and apply the
            // accumulated `left` data plus the following data to it (dyadic each).
            let op_is_func = match &fn_expr {
                Instr::Symbol { name, .. } => Self::is_primitive_op(name),
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
                    Some(Token::Literal(LiteralValue::Symbol { .. }))
                    | Some(Token::LambdaToken)
                    | Some(Token::ComposeToken)
                    | Some(Token::ReverseComposeToken)
                    | Some(Token::LeftForkToken)
                    | Some(Token::RightForkToken)
                    | Some(Token::Comma)
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
                ok = single_func || (n >= 2 && all_func) || left_bind;
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
            | Token::LambdaToken
            | Token::APLNullSym => true,
            // A bare symbol is a strand operand (so `a c` -> (a c)) UNLESS it names a
            // function — a function symbol in strand position is applied instead.
            Token::Literal(LiteralValue::Symbol { name, .. }) => {
                !Self::is_primitive_op(name) && !self.is_known_fn(name)
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
        matches!(
            name,
            "⍳" | "iota" | "⍴" | "rho" | "≢" | "tally" | "⊃" | "first" | "⌽" | "⊖" | "⍉"
                | "↑" | "↓" | "⊂" | "+" | "-" | "*" | "×" | "÷" | "/" | "=" | "≠" | "<" | ">"
                | "≤" | "≥" | "," | "⌈" | "⌊" | "|" | "⍟" | "∧" | "∨" | "~" | "∊" | "⍋" | "⊤" | "⊥"
                | "⊢" | "⊣" | "≡" | "⍓"
        )
    }

    /// Whether the *next* token begins a *function expression* (suitable for a dyadic
    /// `L f R` or a train member). Mirrors Kap's parser.kt `processFn` detection of the
    /// function position: a brace/lambda/paren/group, or a symbol naming a known function,
    /// a primitive operator, or an adverb.
    fn next_is_function_token(&self) -> bool {
        match self.peek() {
            Some(t) => match &t.token {
                Token::OpenBrace | Token::LambdaToken | Token::OpenParen | Token::ApplyToken
                | Token::ReverseComposeToken | Token::ComposeToken | Token::LeftForkToken => true,
                Token::Literal(LiteralValue::Symbol { name, .. }) => {
                    self.is_known_fn(name) || Self::is_primitive_op(name) || Self::is_adverb(name)
                }
                _ => false,
            },
            None => false,
        }
    }

    /// Higher-order operators (adverbs) that take a *function* as one operand:
    /// `/` reduce, `\` scan, `¨` each.
    fn is_adverb(name: &str) -> bool {
        matches!(name, "/" | "reduce" | "\\" | "scan" | "¨" | "each")
    }

    /// Try to parse a *train*: a parenthesised sequence of >=2 function expressions,
    /// e.g. `(f g h)`. Returns `Some(Train{funcs})` on success, `None` (without side effects
    /// other than `self.pos`, which the caller restores) otherwise.
    ///
    /// Each member is parsed with `parse_function_expr` (a *function atom* — symbols, derived
    /// operators, lambdas, or nested trains), NOT a full expression, so `(| -)` yields two
    /// separate functions `[|, -]` rather than the dyadic application `| -`.
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
                    // Parse each member as a single *function atom* (not a full expression),
                    // so `1 +` inside `(1 +)` yields two members [Literal(1), Symbol(+)] (a
                    // left-bind) rather than `parse_function_expr` stopping at `1` and
                    // orphaning the `+`.
                    let e = match self.parse_function_atom() {
                        Ok(e) => e,
                        Err(_) => return None,
                    };
                    funcs.push(e);
                    // Postfix compose/atop inside a parenthesised train: `a ∘ b` / `a ⍛ b`
                    // bind the just-pushed `a` to the next function atom `b` into a single
                    // compose Train (compose=true). Mirrors the standalone parse_function_expr
                    // compose handling, but here the left operand is already in `funcs`.
                    if let Some(tok) = self.peek() {
                        let reverse = match &tok.token {
                            Token::ComposeToken => Some(false),
                            Token::ReverseComposeToken => Some(true),
                            _ => None,
                        };
                        if let Some(rev) = reverse {
                            self.advance();
                            if let Ok(right) = self.parse_function_atom() {
                                let left = funcs.pop().unwrap();
                                funcs.push(Instr::Train {
                                    funcs: vec![left, right],
                                    reverse: rev,
                                    compose: true,
                                });
                            }
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
                            | Some(Token::Comma)
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
        // left-bind `[value, function]` (e.g. `(10 +)`). A *single* member that is itself
        // a function / derived / compose (e.g. `(-)`, `(-⍛+)`, `((×-))`) is a valid
        // 1-member train — wrap it so it applies to the surrounding left/right args.
        // Anything else (e.g. an array `(1 2)`, a group `(1+2)`) is *not* a train.
        let all_funcs = funcs.iter().all(Self::is_function_expr);
        let left_bind = funcs.len() == 2
            && matches!(funcs[0], Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
            && matches!(funcs[1], Instr::Symbol { .. } | Instr::Derived { .. } | Instr::Lambda { .. } | Instr::Train { .. });
        if funcs.len() >= 2 && (all_funcs || left_bind) {
            Some(Instr::Train { funcs, reverse: false, compose: false })
        } else if funcs.len() == 1 && Self::is_function_expr(&funcs[0]) {
            Some(Instr::Train { funcs, reverse: false, compose: false })
        } else {
            None
        }
    }

    /// Parse a *function expression*: a function atom optionally followed by a compose
    /// operator (`∘` atop, `⍛` reverse-compose), or a fork postfix (`A « B » C`).
    fn parse_function_expr(&mut self) -> Result<Instr, AplError> {
        let left = self.parse_function_atom()?;
        // Postfix compose:  f ∘ g  ->  Train([f, g], compose=true)        (atop)
        //                   f ⍛ g  ->  Train([f, g], reverse=true, compose=true)
        // Postfix fork:     a « b » c  ->  Train([a, b, c])              (fork)
        if let Some(t) = self.peek() {
            match &t.token {
                Token::ComposeToken => {
                    self.advance();
                    let right = self.parse_function_atom()?;
                    return Ok(Instr::Train { funcs: vec![left, right], reverse: false, compose: true });
                }
                Token::ReverseComposeToken => {
                    self.advance();
                    let right = self.parse_function_atom()?;
                    return Ok(Instr::Train { funcs: vec![left, right], reverse: true, compose: true });
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
        // A function atom optionally followed by a compose operator (`∘` or `⍛`)
        // combines with the *following* function into a single derived function
        // (Compose / ReverseCompose), NOT a train. e.g. `(-⍛+)` => ReverseCompose(-, +),
        // `f ∘ g` => Compose(f, g). This must run before the 2-train chainer below,
        // which otherwise would mis-parse the operator as a second train member.
        if let Some(t) = self.peek() {
            let compose_kind = match &t.token {
                Token::ComposeToken => Some(false),
                Token::ReverseComposeToken => Some(true),
                _ => None,
            };
            if let Some(reverse) = compose_kind {
                self.advance();
                let right = self.parse_function_atom()?;
                // A compose (`f ∘ g`) / reverse-compose (`f ⍛ g`) is a single derived
                // function, represented internally as a Train with the `compose`/`reverse`
                // flag set (see ast.rs Instr::Train). It is NOT a 2-train/atop.
                return Ok(Instr::Train {
                    funcs: vec![left, right],
                    reverse,
                    compose: true,
                });
            }
        }
        // 2-train chaining: a function atom immediately followed by *another* function atom
        // (and not an adverb, which is handled below as a derived function) forms a 2-train
        // (atop). Mirrors Kap's parser.kt `processFn` building `Chain2(parsedFn, holder.fn)`
        // when `parseValue` returns an `FnParseResult` for both. e.g. `×-` -> Train([×, -]),
        // `÷⌈` -> Train([÷, ⌈]). The trailing operand (if any) is applied by the caller.
        // NOTE: compose operators (`∘`/`⍛`) are deliberately excluded here — they are
        // handled in the block above as a single derived function, not a train member.
        if let Some(t) = self.peek() {
            let next_is_fn_atom = match &t.token {
                Token::Literal(LiteralValue::Symbol { name, .. }) => !Self::is_adverb(name),
                Token::OpenParen
                | Token::LambdaToken
                | Token::ApplyToken
                | Token::LeftForkToken => true,
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
                return Ok(Instr::Derived {
                    func: Box::new(left),
                    op: Box::new(Instr::Symbol { name: adv, namespace: None }),
                });
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
            Token::Comma => {
                // Catenate is a valid train member (e.g. `f , g`).
                self.advance();
                Ok(Instr::Symbol { name: ",".to_string(), namespace: None })
            }
            Token::OpenParen => {
                // Nested train or group of functions. First try a >=2-member train
                // `(f g h)`. If that doesn't pan out (e.g. a single parenthesised
                // function `(-)`, or a derived operator `(×∘-)`), fall back to parsing
                // the inner content as one *function atom* (a single function or a
                // derived operator), wrapped in a 1-element train so it applies to the
                // surrounding left/right arguments. Mirrors parse_primary's `(`) handling
                // but uses a function atom here to avoid re-entering this arm recursively.
                self.advance();
                self.skip_newlines();
                let save = self.pos;
                if let Some(train) = self.try_parse_train() {
                    return Ok(train);
                }
                self.pos = save;
                // Single function (e.g. `(-)`, `((×-))`) or derived operator `(×∘-)`.
                let inner = self.parse_function_atom()?;
                self.skip_newlines();
                match self.peek() {
                    Some(t) if matches!(t.token, Token::CloseParen) => {
                        self.advance();
                        Ok(Instr::Train {
                            funcs: vec![inner],
                            reverse: false,
                            compose: false,
                        })
                    }
                    _ => Err(self.err("expected ) to close function group")),
                }
            }
            Token::Literal(lv) => {
                // A literal value is a valid train member (enables left-bind e.g. `(10 +)`).
                let lv = lv.clone();
                self.advance();
                Ok(Instr::Literal(lv))
            }
            _ => Err(self.err("expected a function in train")),
        }
    }

    /// Whether an expression is a *function* suitable for a train operand.
    fn is_function_expr(e: &Instr) -> bool {
        matches!(
            e,
            Instr::Symbol { .. }
                | Instr::Derived { .. }
                | Instr::Lambda { .. }
                | Instr::Train { .. }
        )
    }

    /// primary := number | char | string | symbol | ( expr ) | [ elements ] | ⍬
    fn parse_primary(&mut self) -> Result<Instr, AplError> {
        let t = self.peek().ok_or_else(|| self.err("unexpected end of input"))?;
        match &t.token {
            Token::Literal(LiteralValue::Symbol { name, namespace }) => {
                let name = name.clone();
                let namespace = namespace.clone();
                self.advance();
                Ok(Instr::Symbol { name, namespace })
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
                            Ok(Instr::DynamicRef { name, namespace })
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
                // Try that first; if it doesn't pan out, fall back to a single function
                // expression (e.g. `(×∘-)`, `(+ « - »)`, which is a derived operator), then
                // finally a normal group `(expr)`.
                let save = self.pos;
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
            Token::Comma => {
                // Catenate is a dyadic operator; represent it as a symbol named ",".
                self.advance();
                Ok(Instr::Symbol {
                    name: ",".to_string(),
                    namespace: None,
                })
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
                                        params.push(name.clone());
                                    }
                                    self.advance();
                                }
                                Some(t) if matches!(t.token, Token::Comma) => {
                                    self.advance();
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
            _ => Err(self.err("unexpected token in primary")),
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
}

/// Build a Symbol Instr from a token's LiteralValue::Symbol.
fn instr_from_symbol(tok: &Token) -> Instr {
    if let Token::Literal(LiteralValue::Symbol { name, namespace }) = tok {
        Instr::Symbol { name: name.clone(), namespace: namespace.clone() }
    } else {
        Instr::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenise;
    use crate::ast::Instr;

    fn parse_one(src: &str) -> Instr {
        let toks = tokenise(src);
        let (stmts, errs) = parse(&toks, &[], &[]);
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
    fn parse_array_literal() {
        match parse_one("[1;2;3]") {
            Instr::Array { elements } => assert_eq!(elements.len(), 3),
            other => panic!("expected Array, got {:?}", other),
        }
    }
}
