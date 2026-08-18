//! Lexer for Kap (Phase 2).

use crate::lex_helpers::*;
use crate::token::{LiteralValue, SpannedToken, Token};
/// Tokenise `src` into a flat list. Errors are emitted as `Token::Error` so the
/// parser can report them with position (strategy §4.7).
pub fn tokenise(src: &str) -> Vec<SpannedToken> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut col = 1usize;

    while i < chars.len() {
        let c = chars[i];
        let start_line = line;
        let start_col = col;

        // whitespace (space/tab) -> skip, no token (Kap treats as insignificant)
        if c == ' ' || c == '\t' {
            i += 1;
            col += 1;
            continue;
        }
        // newline -> Newline token (keeps line continuation logic in parser)
        if c == '\n' {
            out.push(SpannedToken { token: Token::Newline, line: start_line, col: start_col });
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        // comment: ⍝ to end of line
        if c == '⍝' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }
        // character literal: @...
        if c == '@' {
            if let Some((ch, ni, ncol)) = lex_char(&chars, i + 1, line, col) {
                out.push(SpannedToken {
                    token: Token::Literal(LiteralValue::Char(ch)),
                    line: start_line,
                    col: start_col,
                });
                i = ni;
                col = ncol;
                continue;
            } else {
                out.push(SpannedToken { token: Token::Error("invalid character literal".into()), line: start_line, col: start_col });
                i += 1;
                col += 1;
                continue;
            }
        }
        // string literal: "..."
        if c == '"' {
            match lex_string(&chars, i + 1) {
                Ok((s, ni, ncol)) => {
                    out.push(SpannedToken { token: Token::Literal(LiteralValue::Str(s)), line: start_line, col: start_col });
                    i = ni;
                    col = ncol;
                }
                Err(msg) => {
                    out.push(SpannedToken { token: Token::Error(msg), line: start_line, col: start_col });
                    i = chars.len();
                }
            }
            continue;
        }
        // number (starts with digit, ¯, or 0x/0b)
        if c.is_ascii_digit() || c == '¯' || (c == '0' && i + 1 < chars.len() && (chars[i+1] == 'x' || chars[i+1] == 'b')) {
            match lex_number(&chars, i) {
                Ok((num, ni)) => {
                    out.push(SpannedToken { token: Token::Literal(LiteralValue::Number(num)), line: start_line, col: start_col });
                    // advance column by consumed length
                    let consumed = ni - i;
                    i = ni;
                    col += consumed;
                }
                Err(msg) => {
                    out.push(SpannedToken { token: Token::Error(msg), line: start_line, col: start_col });
                    i += 1;
                    col += 1;
                }
            }
            continue;
        }
        // keyword-namespace symbol: `:NAME` (e.g. `:UTF16`, `:pretty`, `:read`).
        // Kap's keyword namespace. Emitted as a symbol with namespace "keyword" so it
        // strands/passes as a value (e.g. `:UTF16 unicode:dec 0xFE 0xFF …`). A bare
        // `:` followed by a non-symbol char (space, digit, `(` …) stays `ColonSym`
        // (used by the `cond : a ⋄ b` guard expression).
        if c == ':' && i + 1 < chars.len() && is_symbol_start(chars[i + 1]) {
            let (raw_name, ni) = lex_symbol(&chars, i + 1);
            let name = if raw_name.is_empty() {
                chars[i + 1].to_string()
            } else {
                raw_name
            };
            out.push(SpannedToken {
                token: Token::Literal(LiteralValue::Symbol {
                    name,
                    namespace: Some("keyword".to_string()),
                }),
                line: start_line,
                col: start_col,
            });
            let consumed = ni.saturating_sub(i).max(2);
            i = ni.max(i + 2);
            col += consumed;
            continue;
        }
        // symbol literal: `'foo` -> a *symbol value* (distinct from a variable
        // reference). Emitted as `LiteralValue::SymbolValue` so the parser builds an
        // `Instr::SymbolValue` that evaluates to `APLValue::Symbol` (Kotlin SymbolValue).
        if c == '\'' {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() && chars[j] != '\'' && !chars[j].is_whitespace() {
                name.push(chars[j]);
                j += 1;
            }
            // Consume the closing `'` if present.
            if j < chars.len() && chars[j] == '\'' {
                j += 1;
            }
            out.push(SpannedToken {
                token: Token::Literal(LiteralValue::SymbolValue { name: name.clone() }),
                line: start_line,
                col: start_col,
            });
            let consumed = j.saturating_sub(i).max(1);
            i = j;
            col += consumed;
            continue;
        }
        // single-char punctuation / symbols
        if let Some(tok) = single_char_token(c) {
            out.push(SpannedToken { token: tok, line: start_line, col: start_col });
            i += 1;
            col += 1;
            continue;
        }
        // APL glyphs that are not ASCII (e.g. ⍳ ⊃ ≢ ⍴ ⌽ etc.) are single-char
        // function/operator *names* in Kap. Emit each as its own Symbol token.
        if !c.is_ascii() {
            out.push(SpannedToken {
                token: Token::Literal(LiteralValue::Symbol { name: c.to_string(), namespace: None }),
                line: start_line,
                col: start_col,
            });
            i += 1;
            col += 1;
            continue;
        }
        // symbol name: a run of non-space, non-special chars (letters, digits, _, etc.)
        if is_symbol_start(c) {
            let (raw_name, ni) = lex_symbol(&chars, i);
            // A lone operator char (e.g. `+`) is accepted by `is_symbol_start` but
            // rejected by `lex_symbol` (non-alphanumeric), yielding an empty name.
            // In that case the symbol *is* just the single char `c`.
            let raw_name = if raw_name.is_empty() { c.to_string() } else { raw_name };
            // namespace-qualified: foo:bar  (single ':' separator)
            let (name, ns) = split_namespace(&raw_name);
            // Guard: always advance by >= 1 to avoid an infinite loop.
            let consumed = ni.saturating_sub(i).max(1);
            out.push(SpannedToken {
                token: Token::Literal(LiteralValue::Symbol { name, namespace: ns }),
                line: start_line,
                col: start_col,
            });
            i += consumed;
            col += consumed;
            continue;
        }
        // unknown
        out.push(SpannedToken { token: Token::Error(format!("unexpected character: {:?}", c)), line: start_line, col: start_col });
        i += 1;
        col += 1;
    }
    out.push(SpannedToken { token: Token::EndOfFile, line, col });
    out
}

fn single_char_token(c: char) -> Option<Token> {
    Some(match c {
        '(' => Token::OpenParen,
        ')' => Token::CloseParen,
        '{' => Token::OpenBrace,
        '}' => Token::CloseBrace,
        '[' => Token::OpenBracket,
        ']' => Token::CloseBracket,
        '⋄' => Token::StatementSeparator,
        '◊' => Token::StatementSeparator,
        '←' => Token::LeftArrow,
        '⍬' => Token::APLNullSym,
        'λ' => Token::LambdaToken,
        '⍞' => Token::ApplyToken,
        ';' => Token::ListSeparator,
        '∇' => Token::FnDefSym,
        '⇐' => Token::DynassignToken,
        ':' => Token::ColonSym,
        '«' => Token::LeftForkToken,
        '»' => Token::RightForkToken,
        '∘' => Token::ComposeToken,
        '⍛' => Token::ReverseComposeToken,
        '.' => Token::MemberDereferenceToken,
        _ => return None,
    })
}
