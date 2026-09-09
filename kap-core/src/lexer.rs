//! Lexer for Kap (Phase 2).

use crate::lex_helpers::*;
use crate::token::{LiteralValue, SpannedToken, Token};
/// Tokenise `src` into a flat list. Errors are emitted as `Token::Error` so the
/// parser can report them with position (strategy §4.7).
pub fn tokenise(src: &str) -> Vec<SpannedToken> {
    tokenise_with(src, &std::collections::HashSet::new())
}

/// Tokenise with a seed set of engine-registered single-char-exported names
/// (Kotlin engine.kt `exportedSingleCharFunctions`, consulted by the lexer via
/// `charIsSymbolDelimiter`). Directives met mid-input
/// (`declare(:singleCharExported "a")`) register inline as their `)` is lexed,
/// so earlier tokens (including the directive's own) use the old set while
/// everything after uses the new one — mirroring Kotlin's streaming tokenizer.
/// A registered ASCII-alphanumeric char terminates symbol runs (see
/// `lex_symbol`), so `aaaa` lexes as four `a`s.
pub fn tokenise_with(
    src: &str,
    seed: &std::collections::HashSet<char>,
) -> Vec<SpannedToken> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut single: std::collections::HashSet<char> = seed.clone();
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
        // backtick: line continuation (` isBackquote` in Kotlin tokeniser.kt:830).
        // Kotlin's lexer returns a `Whitespace` token (tokeniser.kt:596) which the
        // parser skips between operands, so the next physical line is joined to the
        // current statement. The port skips bare spaces with NO token, so to get the
        // same "skippable separator" effect we emit a Newline token (which
        // `skip_newlines` and statement parsing both treat as ignorable — statements
        // end on `⋄`/EOF, never on Newline). This mirrors exactly what the REPL does:
        // it strips the backtick and joins the lines with `\n`.
        // e.g. `typeToFormatter ← map:with ` \n 'a λx ` \n 'b λy` -> one expression.
        if c == '`' {
            i += 1;
            col += 1;
            // consume the newline(s) following the backtick
            if i < chars.len() && chars[i] == '\n' {
                i += 1;
                line += 1;
                col = 1;
            }
            // also consume any further whitespace on the continued line start
            while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                i += 1;
                col += 1;
            }
            // A backtick line-continuation joins the next line INTO the current
            // statement with NO separator token (Kotlin's lexer strips the backtick
            // and joins the source lines, so `f ` \n 'x` becomes `f 'x` — the operand
            // attaches directly). Emitting a Newline here (a prior approach) broke
            // operand attachment: `map:with ` \n "a" 1` parsed `map:with` as a bare
            // VALUE instead of an application. By emitting nothing, the continued
            // tokens are adjacent just like an inline `map:with "a" 1`.
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
            let (raw_name, ni) = lex_symbol(&chars, i + 1, &single);
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
        // reference). Kotlin ground truth (tokeniser.kt QuotePrefix +
        // parser.kt:1003): `'` emits a bare QuotePrefix token and the PARSER
        // consumes the following full symbol token (`[ns:]name`). Scanning here
        // cannot work because APL glyphs like `⍺` are NOT alphabetic
        // (`char::is_alphabetic` is false for U+237A), so a character-level scan
        // stops immediately and fuses the rest of the line into stray tokens
        // (broke io.kap:31 `isLocallyBound('⍺)`).
        if c == '\'' {
            out.push(SpannedToken {
                token: Token::QuotePrefix,
                line: start_line,
                col: start_col,
            });
            i += 1;
            col += 1;
            continue;
        }
        // single-char punctuation / symbols
        if let Some(tok) = single_char_token(c) {
            let is_close = matches!(tok, Token::CloseParen);
            out.push(SpannedToken { token: tok, line: start_line, col: start_col });
            i += 1;
            col += 1;
            // A `declare(:singleCharExported "X")` directive registers X as a
            // symbol delimiter for everything lexed AFTER its `)` (Kotlin
            // parser.kt:1169 `processSingleCharDeclaration`, engine.kt:577).
            if is_close {
                if let Some(ch) = check_single_char_directive(&out) {
                    single.insert(ch);
                }
            }
            continue;
        }
        // `⍥` (Over, U+2365) is a dedicated operator token (Kotlin `OverOp`),
        // not a plain symbol name. It is non-ASCII (3-byte UTF-8), so the glyph
        // branch below would otherwise emit it as Symbol("⍥") — catch it first.
        // Without this, `⍥⊂ 1 2 3` errors "undefined symbol: ⍥" instead of the
        // oracle's "Operator without left function: ⍥".
        if c == '⍥' {
            out.push(SpannedToken { token: Token::OverToken, line: start_line, col: start_col });
            i += 1;
            col += 1;
            continue;
        }
        // `⍫` (Obverse, U+236B) is a dedicated operator token (Kotlin `ObverseOp`,
        // op.kt:304 / engine.kt:503), not a plain symbol name — same reason as
        // `⍥` above. Without this, `bar ⇐ foo⍫{…}` parses the RHS as bare `foo`
        // and dies "No arguments specified for function".
        if c == '⍫' {
            out.push(SpannedToken { token: Token::ObverseToken, line: start_line, col: start_col });
            i += 1;
            col += 1;
            continue;
        }
        // APL glyphs that are not ASCII (e.g. ⍳ ⊃ ≢ ⍴ ⌽ etc.) are single-char
        // function/operator *names* in Kap. Emit each as its own Symbol token.
        // Exception: `⎕` (U+2395, the quad), which begins a *multi-char* name such
        // as `⎕A`, `⎕p`, `⎕pl` — those must stay one Symbol, so `⎕` falls through
        // to the symbol-run path below.
        if !c.is_ascii() && c != '⎕' {
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
            let (raw_name, ni) = lex_symbol(&chars, i, &single);
            // A lone operator char (e.g. `+`) is accepted by `is_symbol_start` but
            // rejected by `lex_symbol` (non-alphanumeric), yielding an empty name.
            // In that case the symbol *is* just the single char `c`.
            let raw_name = if raw_name.is_empty() { c.to_string() } else { raw_name };
            // Guard: always advance by >= 1 to avoid an infinite loop.
            let consumed = ni.saturating_sub(i).max(1);
            // namespace-qualified: foo:bar  (single ':' separator)
            // Invalid symbol names (Kotlin NamespaceTest.invalidSymbolNames):
            //   * multiple ':' (e.g. `foo:bar:test`)  -> parse error
            //   * trailing ':' (e.g. `bar:`)           -> parse error
            let colon_count = raw_name.matches(':').count();
            if colon_count > 1 {
                out.push(SpannedToken {
                    token: Token::Error(format!(
                        "invalid symbol name '{}': at most one ':' namespace separator allowed",
                        raw_name
                    )),
                    line: start_line,
                    col: start_col,
                });
                i += consumed;
                col += consumed;
                continue;
            }
            let trailing_colon = raw_name.ends_with(':');
            let (name, ns) = split_namespace(&raw_name);
            if trailing_colon {
                out.push(SpannedToken {
                    token: Token::Error(format!(
                        "invalid symbol name '{}': trailing ':' not allowed",
                        raw_name
                    )),
                    line: start_line,
                    col: start_col,
                });
                i += consumed;
                col += consumed;
                continue;
            }
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

/// If the token tail (ignoring newlines) completes a
/// `declare(:singleCharExported "X")` directive, return X's single char for
/// registration. Multi-char strings register nothing (Kotlin stores the whole
/// string opaquely in `exportedSingleCharFunctions`, where it can never match
/// a single char in `charIsSingleCharExported`).
fn check_single_char_directive(out: &[SpannedToken]) -> Option<char> {
    let mut it = out
        .iter()
        .rev()
        .filter(|t| !matches!(t.token, Token::Newline));
    if !matches!(it.next()?.token, Token::CloseParen) {
        return None;
    }
    let s = match &it.next()?.token {
        Token::Literal(LiteralValue::Str(s)) => s.clone(),
        _ => return None,
    };
    match &it.next()?.token {
        Token::Literal(LiteralValue::Symbol { name, namespace })
            if namespace.as_deref() == Some("keyword") && name == "singleCharExported" => {}
        _ => return None,
    }
    if !matches!(it.next()?.token, Token::OpenParen) {
        return None;
    }
    match &it.next()?.token {
        Token::Literal(LiteralValue::Symbol { name, namespace })
            if namespace.is_none() && name == "declare" => {}
        _ => return None,
    }
    let mut ch = s.chars();
    match (ch.next(), ch.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
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
        '⦻' => Token::NilToken,
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
        '⟦' => Token::FunctionCallOpenParen,
        '⟧' => Token::FunctionCallCloseParen,
        _ => return None,
    })
}

