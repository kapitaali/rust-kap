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
        '←' => Token::LeftArrow,
        '⍬' => Token::APLNullSym,
        'λ' => Token::LambdaToken,
        '⍞' => Token::ApplyToken,
        ';' => Token::ListSeparator,
        ',' => Token::Comma,
        '∇' => Token::FnDefSym,
        '⇐' => Token::DynassignToken,
        '«' => Token::LeftForkToken,
        '»' => Token::RightForkToken,
        '.' => Token::MemberDereferenceToken,
        _ => return None,
    })
}
