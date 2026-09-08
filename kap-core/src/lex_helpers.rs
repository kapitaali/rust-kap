//! Lexer helper functions for Kap (Phase 2).
//!
//! Number/char/string/symbol lexing per docs/reference.asciidoc. Negative uses `¯`.
//! Hex `0x..`, binary `0b..`, rational `3r2`/`10.5r`, float `1.234`/`¯0.001`/`4.1e22`,
//! complex `100j200`.

use crate::KapNumber;
use num_bigint::BigInt;
use num_rational::BigRational;

/// Read exactly four hex digits from `chars` (starting at index 0 of the slice)
/// and return the resulting code point, or `None` if fewer than four hex digits.
fn parse_hex4(chars: &[char]) -> Option<u32> {
    if chars.len() < 4 {
        return None;
    }
    let mut v = 0u32;
    for k in 0..4 {
        v = v * 16 + chars[k].to_digit(16)?;
    }
    Some(v)
}

/// Lex a character after `@`. Returns (char, next_index, next_col). Column tracking
/// is approximate (counts chars). Supports `@\n`, `@\t`, `@\0`, `@\s`, `@e`, `@\\`,
/// and `@\uXXXX` (including a surrogate pair `@\uD835\uDC9F` → one astral char).
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
        match e {
            'n' => Some(('\n', i + 2, col + 2)),
            'r' => Some(('\r', i + 2, col + 2)),
            't' => Some(('\t', i + 2, col + 2)),
            'e' => Some((0x1b as char, i + 2, col + 2)),
            's' => Some((' ', i + 2, col + 2)),
            '0' => Some(('\0', i + 2, col + 2)),
            '\\' => Some(('\\', i + 2, col + 2)),
            'u' => {
                // @\uXXXX — may be a high surrogate followed by a low surrogate,
                // which combine into a single astral code point (one `char`).
                let code = parse_hex4(&chars[i + 2..])?;
                if (0xD800..=0xDBFF).contains(&code) {
                    let k = i + 6; // index just past the first `\uXXXX`
                    if k + 1 < chars.len() && chars[k] == '\\' && chars[k + 1] == 'u' {
                        let code2 = parse_hex4(&chars[k + 2..])?;
                        if (0xDC00..=0xDFFF).contains(&code2) {
                            let cp =
                                0x10000 + ((code - 0xD800) << 10) + (code2 - 0xDC00);
                            return char::from_u32(cp).map(|ch| (ch, k + 6, col + 6));
                        }
                    }
                    // Lone surrogate (no valid pairing): Kap has no valid char for it.
                    return None;
                }
                char::from_u32(code).map(|ch| (ch, i + 6, col + 6))
            }
            _ => None,
        }
    } else {
        Some((chars[i], i + 1, col + 1))
    }
}

/// Lex a string after the opening `"`. Returns (string, next_index, next_col).
/// Supports `\\`, `\"`, `\n`, `\r`, and `\uXXXX`. A surrogate pair `\\uD835\\uDC9F`
/// is combined into a single astral `char` (matching Java/Kotlin UTF-16 strings).
pub fn lex_string(chars: &[char], i: usize) -> Result<(String, usize, usize), String> {
    let mut out: Vec<char> = Vec::new();
    let mut j = i;
    while j < chars.len() {
        let c = chars[j];
        if c == '"' {
            return Ok((out.into_iter().collect(), j + 1, j + 2));
        }
        if c == '\\' {
            if j + 1 >= chars.len() {
                return Err("unterminated string escape".into());
            }
            let e = chars[j + 1];
            match e {
                '\\' => {
                    out.push('\\');
                    j += 2;
                }
                '"' => {
                    out.push('"');
                    j += 2;
                }
                'n' => {
                    out.push('\n');
                    j += 2;
                }
                'r' => {
                    out.push('\r');
                    j += 2;
                }
                'u' => {
                    let code = parse_hex4(&chars[j + 2..])
                        .ok_or_else(|| "invalid \\u escape in string".to_string())?;
                    if (0xD800..=0xDBFF).contains(&code) {
                        // high surrogate: look for a trailing `\uXXXX` low surrogate
                        let k = j + 6;
                        if k + 1 < chars.len() && chars[k] == '\\' && chars[k + 1] == 'u' {
                            if let Some(code2) = parse_hex4(&chars[k + 2..]) {
                                if (0xDC00..=0xDFFF).contains(&code2) {
                                    let cp = 0x10000
                                        + ((code - 0xD800) << 10)
                                        + (code2 - 0xDC00);
                                    if let Some(ch) = char::from_u32(cp) {
                                        out.push(ch);
                                        j = k + 6;
                                        continue;
                                    }
                                }
                            }
                        }
                        // Lone surrogate with no valid pair: replacement char.
                        out.push(char::REPLACEMENT_CHARACTER);
                        j = j + 6;
                    } else if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                        j = j + 6;
                    } else {
                        return Err("invalid \\u escape code point in string".into());
                    }
                }
                _ => return Err(format!("invalid string escape: \\{}", e)),
            }
        } else {
            out.push(c);
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
    // hex / binary, with optional leading ¯ (Kotlin `^(¯?)0x…$` / `^(¯?)0b…$`).
    let (neg, hstart) = if chars[i] == '¯'
        && i + 3 < chars.len() + 1
        && chars.get(i + 1) == Some(&'0')
        && matches!(chars.get(i + 2), Some('x') | Some('b'))
    {
        (true, i + 1)
    } else {
        (false, i)
    };
    if chars[hstart] == '0' && hstart + 1 < chars.len() && (chars[hstart + 1] == 'x' || chars[hstart + 1] == 'b') {
        let radix = if chars[hstart + 1] == 'x' { 16 } else { 2 };
        j = hstart + 2;
        let mut digits = String::new();
        while j < chars.len() && (chars[j].is_ascii_alphanumeric()) {
            digits.push(chars[j]);
            j += 1;
        }
        let v = BigInt::parse_bytes(digits.as_bytes(), radix)
            .ok_or_else(|| "invalid integer literal".to_string())?;
        let v = if neg { -v } else { v };
        // negative? handled by leading ¯ already consumed? Here `0x..` is non-negative.
        // Normalise to `Long` when it fits, exactly as the DECIMAL path below does
        // (see the tail of `parse_kap_number`). Without this, `0x20` stayed a
        // `BigInt` while `32` was a `Long`, so integer-count builtins that match
        // on `KapNumber::Long` rejected it — `⍳0x20` errored "⍳ needs an integer
        // count" while `⍳32` worked (output3.kap:38).
        if let Ok(l) = v.to_string().parse::<i64>() {
            return Ok((KapNumber::Long(l), j));
        }
        return Ok((KapNumber::BigInt(v), j));
    }

    // collect run (dots/exponents/signs tracked per complex part: reset at `j`)
    let mut buf = String::new();
    let mut seen_dot = false;
    let mut seen_r = false;
    let mut seen_j = false;
    let mut seen_e = false;
    while j < chars.len() {
        let c = chars[j];
        if c.is_ascii_digit() {
            buf.push(c);
        } else if c == '¯'
            && (j == start || buf.ends_with('e') || buf.ends_with('E') || buf.ends_with('j'))
        {
            // leading negative, exponent sign, or imaginary-part sign (`3j¯5`)
            buf.push('-');
        } else if c == '-' && (buf.ends_with('e') || buf.ends_with('E')) {
            buf.push('-');
        } else if c == '.' && !seen_dot && !seen_r {
            seen_dot = true;
            buf.push('.');
        } else if (c == 'r' || c == 'R') && !seen_r && !seen_j {
            seen_r = true;
            buf.push('r');
        } else if (c == 'j' || c == 'J') && !seen_j {
            seen_j = true;
            seen_dot = false;
            seen_e = false;
            buf.push('j');
        } else if (c == 'e' || c == 'E') && !seen_e {
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

/// Exact decimal string (e.g. `2.5`, `¯3.25`, `10`) → BigRational.
/// Kotlin `Rational.make` equivalent for the trailing-r decimal token form.
fn decimal_str_to_rational(s: &str) -> Result<BigRational, String> {
    let s = s.trim();
    let neg = s.starts_with('-');
    let digits = if neg { &s[1..] } else { s };
    let dot = digits.find('.').unwrap_or(digits.len());
    let frac_len = digits.len() - dot - if dot < digits.len() { 1 } else { 0 };
    let mant_str: String = digits.chars().filter(|c| *c != '.').collect();
    if mant_str.is_empty() {
        return Err("invalid rational numerator".into());
    }
    let mant = BigInt::parse_bytes(mant_str.as_bytes(), 10)
        .ok_or_else(|| "invalid rational numerator".to_string())?;
    let mut r = BigRational::new(mant, BigInt::from(1));
    if frac_len > 0 {
        r /= BigRational::new(BigInt::from(10).pow(frac_len as u32), BigInt::from(1));
    }
    Ok(if neg { -r } else { r })
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
    // rational: num r den. Whole rationals collapse to integers at lex time
    // (Kotlin `makeAPLNumber` reduction: oracle `typeof 2r2` → kap:integer).
    // Trailing-r decimal form (`2.5r`, tokeniser.kt `([0-9]+)(\.([0-9]*))?r$`):
    // the value is the exact decimal itself as a rational.
    if let Some(idx) = buf.find('r') {
        let num_s = &buf[..idx];
        let den_s = &buf[idx + 1..];
        if den_s.is_empty() {
            return decimal_str_to_rational(num_s).map(crate::number::rational_to_kap);
        }
        let den = parse_signed_int(den_s)?;
        if den == BigInt::from(0) {
            return Err("division by zero in rational".into());
        }
        let num_rat: BigRational = if num_s.contains('e') || num_s.contains('E') {
            // No Kotlin number regex pairs an exponent numerator with a
            // denominator; float fallback for an impossible-in-practice shape.
            let f: f64 = num_s.parse().map_err(|_| "invalid rational numerator".to_string())?;
            decimal_str_to_rational(&format!("{:?}", f))?
        } else {
            decimal_str_to_rational(num_s)?
        };
        let r = num_rat / BigRational::new(den, BigInt::from(1));
        return Ok(crate::number::rational_to_kap(r));
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
        if c.is_alphanumeric() || c == '_' || c == ':' || c == '⎕' {
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
        || c == ','
        // `⎕` (quad) begins multi-char names like `⎕A`, `⎕p`; handled in `lex_symbol`.
        || c == '⎕'
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
        // Hex/binary literals normalise to `Long` when they fit, exactly like
        // decimal literals — `0x20` must be indistinguishable from `32` so that
        // integer-count builtins (`⍳`) accept it (oracle: `⍳0x20` → ⟨0…31⟩).
        assert_eq!(number_of("0x12"), KapNumber::Long(0x12));
        assert_eq!(number_of("0b1100110"), KapNumber::Long(0b1100110));
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
