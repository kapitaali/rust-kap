//! Lexer helper functions for Kap (Phase 2).
//!
//! Number/char/string/symbol lexing per docs/reference.asciidoc. Negative uses `¯`.
//! Hex `0x..`, binary `0b..`, rational `3r2`/`10.5r`, float `1.234`/`¯0.001`/`4.1e22`,
//! complex `100j200`.

use crate::KapNumber;
use num_bigint::BigInt;
use num_rational::BigRational;

/// Lex a character after `@`. Returns (char, next_index, next_col). Column tracking
/// is approximate (counts chars). Supports `@\uXXXX`, `@\n`, `@\\LATIN...`, and plain.
pub fn lex_char(chars: &[char], i: usize, _line: usize, col: usize) -> Option<(char, usize, usize)> {
    if i >= chars.len() {
        return None;
    }
    if chars[i] == '\\' {
        // escape
        if i + 1 >= chars.len() {
            return None;
        }
        let e = chars[i + 1];
        let (ch, skip) = match e {
            'n' => ('\n', 2),
            'r' => ('\r', 2),
            't' => ('\t', 2),
            'e' => (0x1b as char, 2),
            's' => (' ', 2),
            '0' => ('\0', 2),
            '\\' => ('\\', 2),
            'u' => {
                // @\uXXXX
                let hex: String = chars.get(i + 2..i + 6)?.iter().collect();
                let code = u32::from_str_radix(&hex, 16).ok()?;
                (char::from_u32(code)?, 6)
            }
            _ => return None,
        };
        Some((ch, i + skip, col + skip))
    } else {
        Some((chars[i], i + 1, col + 1))
    }
}

/// Lex a string after the opening `"`. Returns (string, next_index, next_col).
pub fn lex_string(chars: &[char], i: usize) -> Result<(String, usize, usize), String> {
    let mut s = String::new();
    let mut j = i;
    while j < chars.len() {
        let c = chars[j];
        if c == '"' {
            return Ok((s, j + 1, j + 2));
        }
        if c == '\\' {
            if j + 1 >= chars.len() {
                return Err("unterminated string escape".into());
            }
            let e = chars[j + 1];
            let ch = match e {
                '\\' => '\\',
                '"' => '"',
                'n' => '\n',
                'r' => '\r',
                _ => return Err(format!("invalid string escape: \\{}", e)),
            };
            s.push(ch);
            j += 2;
        } else {
            s.push(c);
            j += 1;
        }
    }
    Err("unterminated string".into())
}

/// Lex a number starting at `i`. Returns (KapNumber, next_index).
pub fn lex_number(chars: &[char], i: usize) -> Result<(KapNumber, usize), String> {
    // gather the numeric run: digits, ¯, ., e/E, r/R, j/J, x/b prefix, sign in exponent
    let mut j = i;
    let start = i;
    // hex / binary
    if chars[i] == '0' && i + 1 < chars.len() && (chars[i + 1] == 'x' || chars[i + 1] == 'b') {
        let radix = if chars[i + 1] == 'x' { 16 } else { 2 };
        j = i + 2;
        let mut digits = String::new();
        while j < chars.len() && (chars[j].is_ascii_alphanumeric()) {
            digits.push(chars[j]);
            j += 1;
        }
        let v = BigInt::parse_bytes(digits.as_bytes(), radix)
            .ok_or_else(|| "invalid integer literal".to_string())?;
        // negative? handled by leading ¯ already consumed? Here `0x..` is non-negative.
        return Ok((KapNumber::BigInt(v), j));
    }

    // collect run
    let mut buf = String::new();
    let mut seen_dot = false;
    let mut seen_r = false;
    let mut seen_j = false;
    let mut seen_e = false;
    while j < chars.len() {
        let c = chars[j];
        if c.is_ascii_digit() {
            buf.push(c);
        } else if c == '¯' && (j == start || buf.ends_with('e') || buf.ends_with('E')) {
            // leading negative or exponent sign
            buf.push('-');
        } else if c == '-' && (buf.ends_with('e') || buf.ends_with('E')) {
            buf.push('-');
        } else if c == '.' && !seen_dot && !seen_r && !seen_j {
            seen_dot = true;
            buf.push('.');
        } else if (c == 'r' || c == 'R') && !seen_r && !seen_j {
            seen_r = true;
            buf.push('r');
        } else if (c == 'j' || c == 'J') && !seen_j {
            seen_j = true;
            buf.push('j');
        } else if (c == 'e' || c == 'E') && !seen_j && !seen_e {
            seen_e = true;
            buf.push('e');
        } else {
            break;
        }
        j += 1;
    }
    if buf.is_empty() {
        return Err("invalid number".into());
    }
    parse_kap_number(&buf).map(|n| (n, j))
}

/// Parse a Kap numeric string into KapNumber. Handles int / bigint / float / rational /
/// complex per reference.
pub fn parse_kap_number(buf: &str) -> Result<KapNumber, String> {
    // complex: real j imag
    if let Some(idx) = buf.find('j') {
        let re_s = &buf[..idx];
        let im_s = &buf[idx + 1..];
        let re = parse_real(re_s)?;
        let im = parse_real(im_s)?;
        return Ok(KapNumber::Complex(re, im));
    }
    // rational: num r den
    if let Some(idx) = buf.find('r') {
        let num_s = &buf[..idx];
        let den_s = &buf[idx + 1..];
        let num = parse_signed_int(num_s)?;
        let den = parse_signed_int(den_s)?;
        if den == BigInt::from(0) {
            return Err("division by zero in rational".into());
        }
        return Ok(KapNumber::Rational(BigRational::new(num, den)));
    }
    // float (has '.' or 'e')
    if buf.contains('.') || buf.contains('e') || buf.contains('E') {
        let f: f64 = buf.parse().map_err(|_| "invalid float".to_string())?;
        return Ok(KapNumber::Double(f));
    }
    // integer (possibly big)
    let v = parse_signed_int(buf)?;
    if let Ok(l) = v.to_string().parse::<i64>() {
        Ok(KapNumber::Long(l))
    } else {
        Ok(KapNumber::BigInt(v))
    }
}

fn parse_signed_int(s: &str) -> Result<BigInt, String> {
    let s = s.trim();
    let neg = s.starts_with('-');
    let digits = if neg { &s[1..] } else { s };
    let mag = BigInt::parse_bytes(digits.as_bytes(), 10).ok_or_else(|| "invalid integer".to_string())?;
    Ok(if neg { -mag } else { mag })
}

fn parse_real(s: &str) -> Result<f64, String> {
    if s.is_empty() {
        return Ok(0.0);
    }
    s.parse::<f64>().map_err(|_| "invalid real part".to_string())
}

/// Lex a symbol name run (letters, digits, _, and APL-ish identifier chars that are not
/// structural). Returns (name, next_index).
pub fn lex_symbol(chars: &[char], i: usize) -> (String, usize) {
    let mut j = i;
    let mut s = String::new();
    while j < chars.len() {
        let c = chars[j];
        if c.is_alphanumeric() || c == '_' || c == ':' {
            s.push(c);
            j += 1;
        } else {
            break;
        }
    }
    (s, j)
}

/// Split `foo:bar` into (name=bar, ns=Some(foo)). `:bar` -> (bar, None) (keyword ns).
pub fn split_namespace(name: &str) -> (String, Option<String>) {
    if let Some(idx) = name.find(':') {
        let ns = &name[..idx];
        let nm = &name[idx + 1..];
        if ns.is_empty() {
            (nm.to_string(), None)
        } else {
            (nm.to_string(), Some(ns.to_string()))
        }
    } else {
        (name.to_string(), None)
    }
}

/// Whether `c` can start a symbol name.
pub fn is_symbol_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '+' || c == '-' || c == '*' || c == '/' ||
    c == '=' || c == '<' || c == '>' || c == '?' || c == '!' || c == '%' || c == '^' ||
    c == '&' || c == '|' || c == '$' || c == '#' || c == '~' || c == '@' || c == '\\'
    // note: many APL glyphs are single-char symbols; handled by parser as bare tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{LiteralValue, Token};

    fn number_of(src: &str) -> KapNumber {
        let toks = crate::lexer::tokenise(src);
        for t in &toks {
            if let Token::Literal(LiteralValue::Number(n)) = &t.token {
                return n.clone();
            }
        }
        panic!("no number literal in: {}", src);
    }

    #[test]
    fn lex_integer() {
        assert_eq!(number_of("1234"), KapNumber::Long(1234));
    }

    #[test]
    fn lex_negative_bar() {
        assert_eq!(number_of("¯456"), KapNumber::Long(-456));
    }

    #[test]
    fn lex_float_keeps_dot() {
        assert_eq!(number_of("1.234"), KapNumber::Double(1.234));
        assert_eq!(number_of("¯0.001"), KapNumber::Double(-0.001));
    }

    #[test]
    fn lex_exponential() {
        match number_of("4.1e22") {
            KapNumber::Double(v) => assert!((v - 4.1e22).abs() < 1.0),
            other => panic!("expected double, got {:?}", other),
        }
    }

    #[test]
    fn lex_hex_and_binary() {
        assert_eq!(number_of("0x12"), KapNumber::BigInt(num_bigint::BigInt::from(0x12)));
        assert_eq!(number_of("0b1100110"), KapNumber::BigInt(num_bigint::BigInt::from(0b1100110)));
    }

    #[test]
    fn lex_rational() {
        assert_eq!(
            number_of("3r2"),
            KapNumber::Rational(num_rational::BigRational::new(3.into(), 2.into()))
        );
    }

    #[test]
    fn lex_complex() {
        assert_eq!(number_of("100j200"), KapNumber::Complex(100.0, 200.0));
    }

    #[test]
    fn lex_big_integer() {
        let n = number_of("10000000000000000000000000000000");
        assert!(matches!(n, KapNumber::BigInt(_)));
    }

    #[test]
    fn lex_string_and_char() {
        let toks = crate::lexer::tokenise("\"foo\" @a");
        assert!(toks.iter().any(|t| matches!(t.token, Token::Literal(LiteralValue::Str(ref s)) if s == "foo")));
        assert!(toks.iter().any(|t| matches!(t.token, Token::Literal(LiteralValue::Char('a')))));
    }

    #[test]
    fn lex_punctuation() {
        let toks = crate::lexer::tokenise("( ) [ ] ← ⍬ λ ⍞ ;");
        let kinds: Vec<_> = toks.iter().map(|t| std::mem::discriminant(&t.token)).collect();
        assert!(kinds.contains(&std::mem::discriminant(&Token::OpenParen)));
        assert!(kinds.contains(&std::mem::discriminant(&Token::CloseBracket)));
        assert!(kinds.contains(&std::mem::discriminant(&Token::LeftArrow)));
        assert!(kinds.contains(&std::mem::discriminant(&Token::APLNullSym)));
        assert!(kinds.contains(&std::mem::discriminant(&Token::LambdaToken)));
        assert!(kinds.contains(&std::mem::discriminant(&Token::ApplyToken)));
        assert!(kinds.contains(&std::mem::discriminant(&Token::ListSeparator)));
    }
}
