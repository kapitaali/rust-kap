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
pub fn parse(tokens: &[SpannedToken]) -> (Vec<Instr>, Vec<AplError>) {
    let mut p = Parser { toks: tokens, pos: 0 };
    let mut stmts = Vec::new();
    let mut errors = Vec::new();
    loop {
        p.skip_newlines();
        if p.peek().map(|t| matches!(t.token, Token::EndOfFile)).unwrap_or(true) {
            break;
        }
        match p.parse_expr() {
            Ok(instr) => {
                stmts.push(instr);
                p.skip_newlines();
                if let Some(t) = p.peek() {
                    if matches!(t.token, Token::StatementSeparator) {
                        p.advance();
                        continue;
                    }
                }
            }
            Err(e) => {
                errors.push(e);
                // bail out to avoid an infinite loop on a broken token stream
                break;
            }
        }
    }
    (stmts, errors)
}

struct Parser<'a> {
    toks: &'a [SpannedToken],
    pos: usize,
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

    /// statement := expr (⋄ expr)*  — here we parse one expression per call.
    fn parse_expr(&mut self) -> Result<Instr, AplError> {
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
        self.parse_assign()
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
                        return Ok(Instr::Assign {
                            target: Box::new(target),
                            value: Box::new(value),
                        });
                    }
                }
                // not an assignment; rewind
                self.pos = save;
            }
        }
        self.parse_apply()
    }

    /// apply := (fn term) | (term fn term)*  — monadic `f x` or dyadic `a f b` / trains.
    fn parse_apply(&mut self) -> Result<Instr, AplError> {
        let first = self.parse_primary()?;
        // A leading *function atom* that cannot be a value — a train, a lambda, or a
        // derived (adverb) operator — is a monadic/dyadic application `fn x` / `x fn y`
        // (e.g. `(f g) y`, `λ(x)x*2 5`, `+/ 1 2 3`). Bare symbols are handled below by the
        // original heuristic, since a symbol may also be a *value* (variable) used as a
        // dyadic left operand (`x + 1`).
        if matches!(
            &first,
            Instr::Train { .. } | Instr::Lambda { .. } | Instr::Derived { .. }
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
            let next_is_operand_not_fn = match &next {
                Some(t) => {
                    !matches!(t, Token::EndOfFile)
                        && !matches!(t, Token::Newline)
                        && !matches!(t, Token::StatementSeparator)
                        && !matches!(t, Token::CloseParen)
                        && !matches!(t, Token::CloseBracket)
                        && !matches!(t, Token::ListSeparator)
                        && !matches!(t, Token::Literal(LiteralValue::Symbol { .. }))
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
                // A leading primitive followed by an adverb (`+/`, `×¨`, `⍟\`) is the
                // *function-then-operator* form, NOT monadic application: it is a derived
                // function `func op` applied to the following data.
                (next_is_operand_not_fn || next_is_primitive) && !next_is_adverb
            } else {
                next_is_operand_not_fn
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
        // Stranding: consecutive operands with no operator between them form a vector.
        // e.g. `1 2 3` -> [1 2 3]. Collect into `left` so a following dyadic operator
        // (e.g. `1 2 3 + 10`) sees the strand as its left argument. A `( OP )` parenthesised
        // operator is NOT an operand here (it is a derived function, e.g. `(+)`).
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
                Some(t) if Self::is_strand_operand(&t.token) && !paren_op => {
                    let operand = self.parse_primary()?;
                    // Build a strand incrementally: if `left` is already an Array strand,
                    // push; otherwise start one.
                    if let Instr::Array { elements } = &mut left {
                        elements.push(operand);
                    } else {
                        left = Instr::Array { elements: vec![left, operand] };
                    }
                }
                _ => break,
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
                    Token::Literal(LiteralValue::Symbol { .. }) => true,
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
            // Scan function atoms / value-then-function. Stop at ')' = valid paren operator.
            loop {
                match self.peek().map(|t| &t.token) {
                    Some(Token::CloseParen) => {
                        ok = true;
                        break;
                    }
                    Some(Token::Literal(_))
                    | Some(Token::LambdaToken)
                    | Some(Token::ComposeToken)
                    | Some(Token::ReverseComposeToken)
                    | Some(Token::LeftForkToken)
                    | Some(Token::RightForkToken)
                    | Some(Token::Comma)
                    | Some(Token::OpenParen) => {
                        self.advance();
                        self.skip_newlines();
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        self.pos = saved;
        ok
    }

    /// Whether a token can begin a strand element (an operand, not an operator/separator).
    fn is_strand_operand(tok: &Token) -> bool {
        matches!(tok, Token::Literal(LiteralValue::Number(_)))
            || matches!(tok, Token::Literal(LiteralValue::Char(_)))
            || matches!(tok, Token::Literal(LiteralValue::Str(_)))
            || matches!(tok, Token::OpenParen)
            || matches!(tok, Token::OpenBracket)
            || matches!(tok, Token::LambdaToken)
            || matches!(tok, Token::APLNullSym)
    }

    /// Kap primitive function/operator names (single-glyph). Used to decide monadic vs
    /// dyadic parse for a leading symbol. A non-primitive (user) symbol is parsed as a
    /// variable/function reference instead. This is a heuristic — Kap resolves valence
    /// at runtime; the set is the primitives wired up in the evaluator.)
    /// True when the next token ends the current statement at the top level:
    /// a newline, a `⋄` separator, or end of input. Newlines inside parentheses/brackets
    /// are NOT seen here — they are consumed by `parse_primary` internally. Breaking the
    /// apply loops on a top-level newline keeps distinct statements separate (so a file's
    /// `a ← 3 \n b ← 4` is two assignments, not `(a ← 3) b`).
    fn at_statement_boundary(&self) -> bool {
        match self.peek() {
            Some(t) => {
                matches!(t.token, Token::Newline)
                    || matches!(t.token, Token::StatementSeparator)
                    || matches!(t.token, Token::EndOfFile)
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
        )
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
                }
            }
        }
        // A train needs >= 2 members. Either all are functions, OR it is a 2-train
        // left-bind `[value, function]` (e.g. `(10 +)`).
        let all_funcs = funcs.iter().all(Self::is_function_expr);
        let left_bind = funcs.len() == 2
            && matches!(funcs[0], Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
            && matches!(funcs[1], Instr::Symbol { .. } | Instr::Derived { .. } | Instr::Lambda { .. } | Instr::Train { .. });
        if funcs.len() >= 2 && (all_funcs || left_bind) {
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
        Ok(left)
    }

    /// Parse a single *function atom* suitable as a train member: a symbol, a derived
    /// (adverb) operator, a lambda, a parenthesised train/group, a fork `A « B » C`,
    /// or a catenate `,`.
    fn parse_function_atom(&mut self) -> Result<Instr, AplError> {
        let t = self.peek().ok_or_else(|| self.err("expected a function in train"))?;
        match &t.token {
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
                // Nested train or group of functions.
                self.advance();
                self.skip_newlines();
                let save = self.pos;
                if let Some(train) = self.try_parse_train() {
                    return Ok(train);
                }
                self.pos = save;
                Err(self.err("expected a function in train"))
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
        let (stmts, errs) = parse(&toks);
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
