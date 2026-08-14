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

/// Parse a full source string's token stream into a list of statement `Instr`s.
/// Returns `(statements, errors)`. `errors` is non-empty on parse failure.
pub fn parse(tokens: &[SpannedToken]) -> (Vec<Instr>, Vec<String>) {
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

    fn err(&self, msg: &str) -> String {
        match self.peek() {
            Some(t) => format!("parse error at {}:{}: {}", t.line, t.col, msg),
            None => format!("parse error (end of input): {}", msg),
        }
    }

    /// statement := expr (⋄ expr)*  — here we parse one expression per call.
    fn parse_expr(&mut self) -> Result<Instr, String> {
        self.parse_assign()
    }

    /// assign := symbol ← apply  (left-associative target)
    fn parse_assign(&mut self) -> Result<Instr, String> {
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
    fn parse_apply(&mut self) -> Result<Instr, String> {
        let first = self.parse_primary()?;
        // A leading symbol is parsed as a MONADIC application `f x` when:
        //   * it is a primitive AND the next token is a plain operand, or another primitive
        //     (train: `⊃ ⍳5` = `⊃(⍳5)`, `≢ ⍳5` = `≢(⍳5)`); or
        //   * it is a *user* symbol (variable/function) AND the next token is a plain operand
        //     (`f 5`). When the next token is an OPERATOR (e.g. `x + 1`), a user symbol is NOT
        //     monadic — it is the LEFT operand of the dyadic operator, so we fall through.
        // Valence is ultimately resolved at runtime; this is a syntactic heuristic.
        if let Instr::Symbol { name, .. } = &first {
            let is_prim = Self::is_primitive_op(name);
            self.skip_newlines();
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
            let do_monadic = if is_prim {
                next_is_operand_not_fn || next_is_primitive
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
            // Fall through: treat `first` as the LEFT operand of a following dyadic op.
        }
        // Stranding: consecutive operands with no operator between them form a vector.
        // e.g. `1 2 3` -> [1 2 3]. Collect into `left` so a following dyadic operator
        // (e.g. `1 2 3 + 10`) sees the strand as its left argument.
        let mut left = first;
        loop {
            self.skip_newlines();
            match self.peek() {
                Some(t) if Self::is_strand_operand(&t.token) => {
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
        // Dyadic form: a f b (f c ...). `left` may be a value or a strand. The operator must
        // be a genuine primitive (not a user variable), otherwise we stop and leave `left`
        // as a lone value / variable reference (e.g. `x` alone).
        loop {
            self.skip_newlines();
            let is_operator = match self.peek() {
                Some(t) => match &t.token {
                    Token::Literal(LiteralValue::Symbol { .. }) => true,
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
            let right = self.parse_apply()?;
            left = Instr::Apply {
                fn_expr: Box::new(fn_expr),
                left: Some(Box::new(left)),
                right: Box::new(right),
            };
        }
        Ok(left)
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
    fn is_primitive_op(name: &str) -> bool {
        matches!(
            name,
            "⍳" | "iota" | "⍴" | "rho" | "≢" | "tally" | "⊃" | "first" | "⌽" | "⊖" | "⍉"
                | "↑" | "↓" | "⊂" | "+" | "-" | "*" | "×" | "÷" | "/" | "=" | "≠" | "<" | ">"
                | "≤" | "≥" | ","
        )
    }

    /// term := primary  (stranding handled inside parse_primary's caller via runs)
    fn parse_term(&mut self) -> Result<Instr, String> {
        self.parse_primary()
    }

    /// primary := number | char | string | symbol | ( expr ) | [ elements ] | ⍬
    fn parse_primary(&mut self) -> Result<Instr, String> {
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
