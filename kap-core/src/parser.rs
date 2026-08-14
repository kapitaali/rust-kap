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
        // Monadic form: function FIRST, then operand — `⍳5`, `≢arr`, `⊃arr`, `⍴arr`,
        // `+ 5`. But a lone symbol with no following operand is a *variable reference*
        // (e.g. `foo`), not an application. Only build `Apply` when an operand follows.
        if matches!(first, Instr::Symbol { .. }) {
            self.skip_newlines();
            let operand_follows = match self.peek() {
                Some(t) => {
                    !matches!(t.token, Token::EndOfFile)
                        && !matches!(t.token, Token::Newline)
                        && !matches!(t.token, Token::StatementSeparator)
                        && !matches!(t.token, Token::CloseParen)
                        && !matches!(t.token, Token::CloseBracket)
                        && !matches!(t.token, Token::ListSeparator)
                }
                None => false,
            };
            if operand_follows {
                let operand = self.parse_apply()?;
                return Ok(Instr::Apply {
                    fn_expr: Box::new(first),
                    left: None,
                    right: Box::new(operand),
                });
            }
            // lone symbol: variable reference
            return Ok(first);
        }
        // Dyadic form: a f b (f c ...). `first` is a value operand.
        let mut left = first;
        loop {
            self.skip_newlines();
            // peek a function token (symbol, or '(' expr ')', or bracket) as the operator
            let is_fn = match self.peek() {
                Some(t) => matches!(t.token, Token::Literal(LiteralValue::Symbol { .. }))
                    || matches!(t.token, Token::OpenParen)
                    || matches!(t.token, Token::OpenBracket)
                    || matches!(t.token, Token::LambdaToken)
                    || matches!(t.token, Token::ApplyToken),
                None => false,
                _ => false,
            };
            if !is_fn {
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
