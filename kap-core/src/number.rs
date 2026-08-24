//! Number model for Kap (Phase 1).
//!
//! Mirrors `array/.../number.kt`. Five distinct scalar number kinds are kept SEPARATE
//! (Long/Double/BigInt/Rational/Complex) — Kap does NOT coerce `1.0` to `1`; a Double
//! prints as `1.0` (see PROGRESS.md correction entry). Promotion rules (Long <-> Double
//! <-> BigInt <-> Rational <-> Complex) follow `number.kt`'s `asXxx` / `numericCompare`.
//!
//! Strategy §4.2 (D2): BigInt/Rational come from `num-bigint`/`num-rational` (pure Rust).

use num_bigint::BigInt;
use num_rational::BigRational;
use std::cmp::Ordering;

/// A Kap scalar number. Distinct variants are preserved on purpose.
#[derive(Debug, Clone, PartialEq)]
pub enum KapNumber {
    Long(i64),
    Double(f64),
    BigInt(BigInt),
    Rational(BigRational),
    /// Complex number: (real, imaginary), both f64 (matches Kap's `Complex`).
    Complex(f64, f64),
}

impl KapNumber {
    /// Numeric value as f64 (may lose precision / throw on huge bigint — matches Kap
    /// `asDouble`, which just calls `.toDouble()`).
    pub fn as_double(&self) -> f64 {
        match self {
            KapNumber::Long(v) => *v as f64,
            KapNumber::Double(v) => *v,
            KapNumber::BigInt(v) => v.to_string().parse::<f64>().unwrap_or(f64::INFINITY),
            KapNumber::Rational(v) => v.to_string().parse::<f64>().unwrap_or(f64::INFINITY),
            KapNumber::Complex(re, _) => *re,
        }
    }

    /// Numeric value as i64. `BigInt`/`Rational` that do not fit throw (→ return Err).
    pub fn as_long(&self) -> Result<i64, String> {
        match self {
            KapNumber::Long(v) => Ok(*v),
            KapNumber::Double(v) => Ok(*v as i64),
            KapNumber::BigInt(v) => bigint_to_i64(v),
            KapNumber::Rational(v) => {
                let n = bigint_to_i64(v.numer())?;
                let d = bigint_to_i64(v.denom())?;
                if n % d != 0 {
                    return Err(format!("not an integer: {}", v));
                }
                Ok(n / d)
            }
            KapNumber::Complex(re, im) => {
                if *im != 0.0 {
                    return Err("number is complex".to_string());
                }
                Ok(*re as i64)
            }
        }
    }

    /// Real part as f64 pair (re, im). Complex keeps both; reals have im = 0.
    pub fn as_complex(&self) -> (f64, f64) {
        match self {
            KapNumber::Complex(re, im) => (*re, *im),
            KapNumber::Double(v) => (*v, 0.0),
            KapNumber::Long(v) => (*v as f64, 0.0),
            KapNumber::BigInt(v) => (v.to_string().parse::<f64>().unwrap_or(f64::INFINITY), 0.0),
            KapNumber::Rational(v) => (v.to_string().parse::<f64>().unwrap_or(f64::INFINITY), 0.0),
        }
    }

    pub fn is_complex(&self) -> bool {
        matches!(self, KapNumber::Complex(_, im) if *im != 0.0)
    }

    pub fn is_zero(&self) -> bool {
        match self {
            KapNumber::Long(v) => *v == 0,
            KapNumber::Double(v) => *v == 0.0,
            KapNumber::BigInt(v) => *v == BigInt::from(0),
            KapNumber::Rational(v) => *v == BigRational::new(num_bigint::BigInt::from(0), num_bigint::BigInt::from(1)),
            KapNumber::Complex(re, im) => *re == 0.0 && *im == 0.0,
        }
    }

    pub fn as_boolean(&self) -> bool {
        !self.is_zero()
    }

    /// Total-order comparison across number kinds, mirroring `number.kt`'s `numericCompare`.
    /// Complex values are NOT orderable → returns `Err` (Kap throws
    /// `APLArgumentComplexOrderingException`).
    pub fn numeric_cmp(&self, other: &KapNumber) -> Result<Ordering, String> {
        use KapNumber::*;
        // Complex with non-zero imaginary is not orderable.
        if self.is_complex() || other.is_complex() {
            return Err("complex numbers are not orderable".to_string());
        }
        // Compare by converting both to BigRational when possible; fall back to f64.
        let to_rat = |n: &KapNumber| -> Option<BigRational> {
            match n {
                Long(v) => Some(BigRational::new(num_bigint::BigInt::from(*v), num_bigint::BigInt::from(1))),
                BigInt(v) => Some(BigRational::new(v.clone(), num_bigint::BigInt::from(1))),
                Rational(v) => Some(v.clone()),
                // Double/Complex: fall through to f64 comparison below.
                Double(_) | Complex(..) => None,
            }
        };
        match (to_rat(self), to_rat(other)) {
            (Some(a), Some(b)) => Ok(a.cmp(&b)),
            _ => {
                // At least one side is Double; compare as f64.
                let a = self.as_double();
                let b = other.as_double();
                a.partial_cmp(&b).ok_or_else(|| "NaN in comparison".to_string())
            }
        }
    }

    /// Format per Kap's PLAIN/PRETTY (readable=false) and READABLE (readable=true) styles.
    /// Key rule (PROGRESS correction): `1.0` prints as `1.0`, NOT `1`. Only the minus
    /// sign is swapped (`-` -> `¯`) in READABLE style.
    pub fn format(&self, readable: bool) -> String {
        let neg = |s: String| -> String {
            if readable {
                s.replace('-', "¯")
            } else {
                s
            }
        };
        match self {
            KapNumber::Long(v) => neg(v.to_string()),
            KapNumber::Double(v) => neg(format_double(*v)),
            KapNumber::BigInt(v) => neg(v.to_string()),
            KapNumber::Rational(v) => {
                let num = v.numer().to_string();
                let den = v.denom().to_string();
                // Kotlin renders rationals as `num/den` in BOTH REPL and plain
                // contexts (oracle: 1r2 -> 1/2, ¯1r2 -> -1/2). The `nrd` form
                // is a port-only invention — removed 2026-08-24.
                neg(format!("{}/{}", num, den))
            }
            KapNumber::Complex(re, im) => {
                // APL `J` notation: `re Jim`.
                let rs = format_double(*re);
                let ims = format_double(*im);
                let ims = if ims.starts_with('-') {
                    ims
                } else {
                    format!("+{}", ims)
                };
                neg(format!("{}J{}", rs, ims))
            }
        }
    }
}

/// Format an f64 the way Kap's `formatDouble` does for normal magnitudes: keep a trailing
/// `.0` for integral values, and swap nothing here (minus handling is done by caller).
/// Very large/small magnitudes use exponent notation (refined later when porting
/// `FormatNumbersTest`).
pub fn format_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "¯Infinity".to_string() } else { "Infinity".to_string() };
    }
    let s = format!("{}", v);
    if s.parse::<f64>().map(|x| x.fract() == 0.0).unwrap_or(false) && !s.contains('.') {
        // integral-looking integer (e.g. "1") -> force one decimal: "1.0"
        format!("{}.0", s)
    } else if s.contains('.') {
        s
    } else {
        format!("{}.0", s)
    }
}

/// Convert a `BigInt` to `i64`, erroring if it does not fit. Avoids the `ToPrimitive`
/// trait import dance; uses decimal string round-trip which is always correct.
fn bigint_to_i64(v: &BigInt) -> Result<i64, String> {
    v.to_string()
        .parse::<i64>()
        .map_err(|_| format!("does not fit in long: {}", v))
}

impl KapNumber {
    /// Addition with the Kap promotion rules (Long/Double/BigInt/Rational/Complex).
    pub fn add(&self, other: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, other) {
            (Long(a), Long(b)) => Long(a + b),
            (Long(a), Double(b)) | (Double(b), Long(a)) => Double(*a as f64 + *b),
            (Double(a), Double(b)) => Double(a + b),
            (BigInt(a), BigInt(b)) => BigInt(a + b),
            (Long(a), BigInt(b)) | (BigInt(b), Long(a)) => BigInt(num_bigint::BigInt::from(*a) + b),
            (Rational(a), Rational(b)) => Rational(a + b),
            (Long(a), Rational(b)) | (Rational(b), Long(a)) => {
                Rational(b + BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(1)))
            }
            (Complex(ar, ai), Complex(br, bi)) => Complex(ar + br, ai + bi),
            (Long(a), Complex(br, bi)) | (Complex(br, bi), Long(a)) => Complex(*a as f64 + br, *bi),
            (Double(a), Complex(br, bi)) | (Complex(br, bi), Double(a)) => Complex(a + br, *bi),
            // remaining mixed cases fall back to f64
            _ => Double(self.as_double() + other.as_double()),
        }
    }

    /// Multiplication with the Kap promotion rules (Long/Double/BigInt/Rational/Complex).
    pub fn mul(&self, other: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, other) {
            (Long(a), Long(b)) => Long(a * b),
            (Long(a), Double(b)) | (Double(b), Long(a)) => Double(*a as f64 * *b),
            (Double(a), Double(b)) => Double(a * b),
            (BigInt(a), BigInt(b)) => BigInt(a * b),
            (Long(a), BigInt(b)) | (BigInt(b), Long(a)) => BigInt(num_bigint::BigInt::from(*a) * b),
            (Rational(a), Rational(b)) => Rational(a * b),
            (Long(a), Rational(b)) | (Rational(b), Long(a)) => {
                Rational(b * BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(1)))
            }
            (Complex(ar, ai), Complex(br, bi)) => Complex(ar * br - ai * bi, ar * bi + ai * br),
            (Long(a), Complex(br, bi)) | (Complex(br, bi), Long(a)) => Complex(*a as f64 * br, *a as f64 * bi),
            (Double(a), Complex(br, bi)) | (Complex(br, bi), Double(a)) => Complex(a * br, a * bi),
            // remaining mixed cases fall back to f64
            _ => Double(self.as_double() * other.as_double()),
        }
    }

    /// Negation.
    pub fn neg(&self) -> KapNumber {
        use KapNumber::*;
        match self {
            Long(v) => Long(-v),
            Double(v) => Double(-v),
            BigInt(v) => BigInt(-v.clone()),
            Rational(v) => Rational(-v),
            Complex(r, i) => Complex(-r, -i),
        }
    }

    /// Subtraction with the Kap promotion rules (Long/Double/BigInt/Rational/Complex).
    pub fn sub(&self, other: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, other) {
            (Long(a), Long(b)) => Long(a - b),
            (Long(a), Double(b)) | (Double(b), Long(a)) => Double(*a as f64 - *b),
            (Double(a), Double(b)) => Double(a - b),
            (BigInt(a), BigInt(b)) => BigInt(a - b),
            (Long(a), BigInt(b)) | (BigInt(b), Long(a)) => BigInt(num_bigint::BigInt::from(*a) - b),
            (Rational(a), Rational(b)) => Rational(a - b),
            (Long(a), Rational(b)) | (Rational(b), Long(a)) => {
                Rational(b * -BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(1)))
                    .neg()
            }
            (Complex(ar, ai), Complex(br, bi)) => Complex(ar - br, ai - bi),
            (Long(a), Complex(br, bi)) | (Complex(br, bi), Long(a)) => Complex(*a as f64 - br, -*bi),
            (Double(a), Complex(br, bi)) | (Complex(br, bi), Double(a)) => Complex(a - br, -*bi),
            _ => Double(self.as_double() - other.as_double()),
        }
    }

    /// Division. Integer/integer -> Rational (Kap rule: `4 ÷ 2` is `2`, but `1 ÷ 2` is
    /// `1r2`); float inputs -> Double; complex -> complex divide.
    pub fn div(&self, other: &KapNumber) -> KapNumber {
        use KapNumber::*;
        // Integer ÷ integer where the divisor divides evenly -> integer result.
        if let (Long(a), Long(b)) = (self, other) {
            if *b != 0 && a % b == 0 {
                return Long(a / b);
            }
            if *b == 0 {
                return Rational(BigRational::new(num_bigint::BigInt::from(0), num_bigint::BigInt::from(0)));
            }
            return Rational(BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(*b)));
        }
        match (self, other) {
            (Long(a), Double(b)) | (Double(b), Long(a)) => Double(*a as f64 / *b),
            (Double(a), Double(b)) => Double(a / b),
            (BigInt(a), BigInt(b)) => {
                if *b == num_bigint::BigInt::from(0) {
                    return Rational(BigRational::new(num_bigint::BigInt::from(0), num_bigint::BigInt::from(0)));
                }
                Rational(BigRational::new(a.clone(), b.clone()))
            }
            (Long(a), BigInt(b)) | (BigInt(b), Long(a)) => {
                Rational(BigRational::new(num_bigint::BigInt::from(*a), b.clone()))
            }
            (Rational(a), Rational(b)) => Rational(a / b),
            (Long(a), Rational(b)) | (Rational(b), Long(a)) => {
                Rational(BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(1)) / b)
            }
            (Complex(ar, ai), Complex(br, bi)) => {
                // (a+bi)/(c+di) = ((ac+bd) + (bc-ad)i) / (c^2+d^2)
                let den = br * br + bi * bi;
                if den == 0.0 {
                    return Complex(0.0, 0.0);
                }
                Complex((ar * br + ai * bi) / den, (ai * br - ar * bi) / den)
            }
            (Long(a), Complex(br, bi)) | (Complex(br, bi), Long(a)) => {
                let den = br * br + bi * bi;
                if den == 0.0 {
                    return Complex(0.0, 0.0);
                }
                let x = *a as f64;
                Complex((x * br) / den, (x * -bi) / den)
            }
            (Double(a), Complex(br, bi)) | (Complex(br, bi), Double(a)) => {
                let den = br * br + bi * bi;
                if den == 0.0 {
                    return Complex(0.0, 0.0);
                }
                Complex(a / br, -a / bi)
            }
            _ => Double(self.as_double() / other.as_double()),
        }
    }

    /// Parse a Kap numeric *string* (from `⍎"…"` / `parseStringToNumber`) into a `KapNumber`.
    ///
    /// Faithful port of Kotlin `ParseNumberFunction.parseStringToNumber`
    /// (format.kt:267): integer → double → rational, in that order, each via an
    /// anchored regex. The regexes use ASCII `-` (NOT Kap's `¯` high-minus), so a
    /// `¯`-prefixed string must error. Returns `None` when no pattern matches (the
    /// caller then raises the Kotlin `Value cannot be parsed as a number` error).
    ///
    /// Reductions mirror Kotlin's `makeAPLNumberWithReduction` / `makeAPLNumber`:
    /// a rational whose denominator is 1 collapses to an integer (long if it fits,
    /// else bigint); a rational whose denominator is 0 is kept as-is (Kotlin does
    /// not reduce a zero denominator). This is distinct from the Kap *literal* lexer
    /// (`lex_helpers::lex_number`), which accepts `¯` and never produces zero-denom
    /// rationals.
    pub fn parse_kap_number_string(s: &str) -> Option<KapNumber> {
        // INTEGER: ^(-?[0-9]+)$
        if let Some(d) = s.strip_prefix('-') {
            if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) {
                let v = BigInt::parse_bytes(d.as_bytes(), 10)?;
                return Some(bigint_to_kap(&-v));
            }
        } else if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            let v = BigInt::parse_bytes(s.as_bytes(), 10)?;
            return Some(bigint_to_kap(&v));
        }
        // DOUBLE: ^(-?([0-9]*\.[0-9]+([eE]-?[0-9]+)?|[0-9]+[eE]-?[0-9]+|[0-9]+\.))$
        // (Kotlin throws on a bare "."; here "." fails all three alternations.)
        let double_ok = if let Some(d) = s.strip_prefix('-') {
            double_shape(&d)
        } else {
            double_shape(s)
        };
        if double_ok {
            if let Ok(f) = s.parse::<f64>() {
                return Some(KapNumber::Double(f));
            }
        }
        // RATIONAL: ^(-?[0-9]+)/(-?[0-9]+)$
        if let Some((num_s, den_s)) = s.split_once('/') {
            let num_digits = num_digits_only(num_s);
            let den_digits = num_digits_only(den_s);
            if let (Some(n), Some(d)) = (num_digits, den_digits) {
                let num = if num_s.starts_with('-') { -n } else { n };
                let den = if den_s.starts_with('-') { -d } else { d };
                let r = BigRational::new(num, den); // den may be 0 → kept as-is
                if *r.denom() == num_bigint::BigInt::from(1) {
                    return Some(bigint_to_kap(r.numer()));
                }
                return Some(KapNumber::Rational(r));
            }
        }
        None
    }
}

/// True iff `s` is an optional `-` followed by one or more ASCII digits. Returns the
/// magnitude as a `BigInt` when true, else `None`.
fn num_digits_only(s: &str) -> Option<BigInt> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    BigInt::parse_bytes(digits.as_bytes(), 10)
}

/// Map a `BigInt` to the smallest `KapNumber` variant that holds it exactly.
pub fn bigint_to_kap(v: &BigInt) -> KapNumber {
    if let Ok(l) = v.to_string().parse::<i64>() {
        KapNumber::Long(l)
    } else {
        KapNumber::BigInt(v.clone())
    }
}

/// Match Kotlin's DOUBLE_PATTERN on the magnitude `s` (sign already stripped).
/// Three alternatives after the optional `-`:
///   (1) `[0-9]*.[0-9]+([eE]-?[0-9]+)?`  (digits . digits, optional exponent)
///   (2) `[0-9]+[eE]-?[0-9]+`            (digits + exponent, no dot)
///   (3) `[0-9]+.`                        (digits . — no fractional part)
fn double_shape(s: &str) -> bool {
    // alternative 3: [0-9]+\.  (digits . — no fractional part, no exponent)
    if s.ends_with('.') {
        let int_part = &s[..s.len() - 1];
        return !int_part.is_empty() && int_part.bytes().all(|b| b.is_ascii_digit());
    }
    // A dot may carry an optional exponent: [0-9]*.[0-9]+([eE]-?[0-9]+)? (alt 1)
    // OR there is no dot and the exponent form [0-9]+[eE]-?[0-9]+ (alt 2).
    // Check the dot form first so `.`-and-`e` strings don't fall into alt 2.
    if let Some(dot) = s.find('.') {
        let (int_part, frac_part) = s.split_at(dot);
        if !int_part.is_empty() && !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let frac = &frac_part[1..]; // drop the dot
        if let Some(e) = frac.find('e').or_else(|| frac.find('E')) {
            let digits = &frac[..e];
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            return exp_suffix(&frac[e..]);
        }
        return !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit());
    }
    // alternative 2: [0-9]+[eE]-?[0-9]+  (no dot allowed)
    if let Some(e) = s.find('e').or_else(|| s.find('E')) {
        let (int_part, exp_part) = s.split_at(e);
        if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        return exp_suffix(exp_part);
    }
    false
}

/// Validate an exponent suffix `[eE]-?[0-9]+`. Accepts `e3`, `e-3`, `E12`.
fn exp_suffix(s: &str) -> bool {
    let body = s.strip_prefix('e').or_else(|| s.strip_prefix('E')).unwrap_or(s);
    let digits = body.strip_prefix('-').unwrap_or(body);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

impl KapNumber {
    pub fn ceil(&self) -> KapNumber {
        use KapNumber::*;
        match self {
            Long(v) => Long(*v),
            // Oracle: `⌈3.7` → `3` typed `kap:integer` — a whole-valued result
            // normalises to Long (Kotlin `ParsedDouble.floor/ceil` → `APLLong`).
            Double(v) => {
                let c = v.ceil();
                if c.fract() == 0.0 && c.abs() <= i64::MAX as f64 {
                    Long(c as i64)
                } else {
                    Double(c)
                }
            }
            BigInt(v) => BigInt(v.clone()),
            Rational(v) => {
                // ceil of a/b = -floor(-a/b)
                let c = (-v).floor();
                Rational(-c)
            }
            Complex(r, i) => Complex(r.ceil(), i.ceil()),
        }
    }

    /// Floor: largest integer ≤ self.
    pub fn floor(&self) -> KapNumber {
        use KapNumber::*;
        match self {
            Long(v) => Long(*v),
            // Oracle: `⌊3.7` → `3` typed `kap:integer` — normalise whole doubles.
            Double(v) => {
                let f = v.floor();
                if f.fract() == 0.0 && f.abs() <= i64::MAX as f64 {
                    Long(f as i64)
                } else {
                    Double(f)
                }
            }
            BigInt(v) => BigInt(v.clone()),
            Rational(v) => Rational(v.floor()),
            Complex(r, i) => Complex(r.floor(), i.floor()),
        }
    }

    /// Natural exponential (base e).
    pub fn exp(&self) -> KapNumber {
        use KapNumber::*;
        match self {
            Complex(r, i) => {
                // e^(a+bi) = e^a (cos b + i sin b)
                let ea = r.exp();
                Complex(ea * bcos(*i), ea * bsin(*i))
            }
            other => Double(other.as_double().exp()),
        }
    }

    /// Natural logarithm (monadic `⍟`).
    pub fn nat_log(&self) -> KapNumber {
        use KapNumber::*;
        match self {
            Complex(r, i) => {
                // ln(z) = ln|z| + i arg(z)
                let (mag, arg) = (babs((*r, *i)), barg((*r, *i)));
                Complex(mag.ln(), arg)
            }
            other => Double(other.as_double().ln()),
        }
    }

    /// Logarithm. Monadic `⍟ x` = natural log; dyadic `a ⍟ b` = log base b of a.
    pub fn log(&self, base: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, base) {
            (Complex(r, i), _) => {
                // ln(z) = ln|z| + i arg(z)
                let z = (*r, *i);
                let (mag, arg) = (babs(z), barg(z));
                Complex(mag.ln(), arg)
            }
            (a, b) => Double(a.as_double().ln() / b.as_double().ln()),
        }
    }

    /// Power (Kap `*`: `a * b` = a to the power of b). Integer powers of integers stay exact
    /// where possible; everything else falls back to f64 (or complex) arithmetic.
    pub fn pow(&self, exp: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, exp) {
            (Long(a), Long(b)) if *b >= 0 => {
                if let Some(e) = u32::try_from(*b).ok() {
                    return Long(a.saturating_pow(e));
                }
                Double((*a as f64).powf(*b as f64))
            }
            (Long(a), Long(b)) => Double((*a as f64).powf(*b as f64)),
            (BigInt(a), Long(b)) => {
                if let Some(e) = u32::try_from(*b).ok() {
                    return BigInt(a.pow(e));
                }
                Double(a.to_string().parse::<f64>().unwrap_or(f64::INFINITY).powf(*b as f64))
            }
            (Rational(a), Long(b)) if *b >= 0 => {
                if let Some(e) = u32::try_from(*b).ok() {
                    return Rational(a.pow(e as i32));
                }
                Double(a.to_string().parse::<f64>().unwrap_or(f64::INFINITY).powf(*b as f64))
            }
            (Complex(re, im), Long(b)) => {
                let z = (*re, *im);
                let (mag, arg) = (babs(z), barg(z));
                let p = *b as f64;
                let rm = mag.powf(p);
                Complex(rm * bcos(arg * p), rm * bsin(arg * p))
            }
            (Complex(re, im), Double(b)) => {
                let z = (*re, *im);
                let (mag, arg) = (babs(z), barg(z));
                let p = *b;
                let rm = mag.powf(p);
                Complex(rm * bcos(arg * p), rm * bsin(arg * p))
            }
            _ => Double(self.as_double().powf(exp.as_double())),
        }
    }

    /// Square root (Kap `√` monadic). Negative reals and all complex inputs yield a
    /// complex result, mirroring Kotlin `SqrtAPLFunction` (`x.pow(Complex.ONE_HALF)`).
    pub fn sqrt(&self) -> KapNumber {
        match self {
            KapNumber::Complex(r, i) => {
                let z = (*r, *i);
                let (mag, arg) = (babs(z), barg(z));
                let rm = mag.sqrt();
                KapNumber::Complex(rm * bcos(arg * 0.5), rm * bsin(arg * 0.5))
            }
            other => {
                let x = other.as_double();
                if x < 0.0 {
                    KapNumber::Complex(0.0, (-x).sqrt())
                } else {
                    KapNumber::Double(x.sqrt())
                }
            }
        }
    }

    /// Nth root (Kap `√` dyadic: `a √ b` = `b ^ (1/a)`). Delegates to the power operation
    /// via the reciprocal degree; negative radicands yield complex results.
    pub fn nth_root(&self, degree: &KapNumber) -> KapNumber {
        let a = degree.as_double();
        if a == 0.0 {
            return KapNumber::Double(f64::NAN);
        }
        let inv = 1.0 / a;
        match self {
            KapNumber::Complex(r, i) => {
                let z = (*r, *i);
                let (mag, arg) = (babs(z), barg(z));
                let rm = mag.powf(inv);
                KapNumber::Complex(rm * bcos(arg * inv), rm * bsin(arg * inv))
            }
            other => {
                let b = other.as_double();
                if b < 0.0 {
                    KapNumber::Complex(0.0, (-b).powf(inv))
                } else {
                    KapNumber::Double(b.powf(inv))
                }
            }
        }
    }

    /// Signum (Kap `×`: `1`, `0`, or `-1` for real; angle-1 unit complex for complex).
    pub fn signum(&self) -> KapNumber {
        use KapNumber::*;
        match self {
            Long(v) => Long(if *v > 0 { 1 } else if *v < 0 { -1 } else { 0 }),
            Double(v) => Double(if *v > 0.0 { 1.0 } else if *v < 0.0 { -1.0 } else { 0.0 }),
            BigInt(v) => Long(if *v > num_bigint::BigInt::from(0) { 1 } else if *v < num_bigint::BigInt::from(0) { -1 } else { 0 }),
            Rational(v) => Long(if *v.numer() > num_bigint::BigInt::from(0) { 1 } else if *v.numer() < num_bigint::BigInt::from(0) { -1 } else { 0 }),
            Complex(r, i) => {
                let m = babs((*r, *i));
                if m == 0.0 { Complex(0.0, 0.0) } else { Complex(r / m, i / m) }
            }
        }
    }

    /// Reciprocal (Kap `÷` monadic): 1 / x. Delegates to `div` (which already applies the
    /// Kap integer/integer -> Rational promotion rule).
    pub fn recip(&self) -> KapNumber {
        KapNumber::Long(1).div(self)
    }

    /// Kap residue/modulo: `|` is the modulus such that `a = (a|b) + b * floor(a/b)`.
    /// `a|b` returns the residue of `b` modulo `a` (so `a` is the modulus, `b` the value);
    /// the result lies in `[0, |a|)` and takes the sign of `a` (Kap/APL rule). Concretely
    /// `self|other` = `other.rem_euclid(self)`.
    pub fn modulo(&self, other: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, other) {
            (Long(a), Long(b)) if *a != 0 => Long(b.rem_euclid(*a)),
            (Long(a), BigInt(b)) if *a != 0 => {
                // `self` (the modulus) is a Long; compute `other rem_euclid self`.
                let b = b.to_string().parse::<f64>().unwrap_or(0.0);
                let a = *a as f64;
                if a == 0.0 {
                    Double(0.0)
                } else {
                    Double(b.rem_euclid(a))
                }
            }
            _ => {
                let a = self.as_double();
                let b = other.as_double();
                if a == 0.0 {
                    return Double(0.0);
                }
                Double(b.rem_euclid(a))
            }
        }
    }

    /// Logical not (boolean). Non-zero -> 0, zero -> 1 (Kap booleans are 1/0).
    pub fn not(&self) -> KapNumber {
        if self.is_zero() {
            KapNumber::Long(1)
        } else {
            KapNumber::Long(0)
        }
    }
}

// Small complex helpers used by exp/log above.
fn bcos(x: f64) -> f64 {
    x.cos()
}
fn bsin(x: f64) -> f64 {
    x.sin()
}
fn babs((r, i): (f64, f64)) -> f64 {
    (r * r + i * i).sqrt()
}
fn barg((r, i): (f64, f64)) -> f64 {
    i.atan2(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_format_no_decimal() {
        assert_eq!(KapNumber::Long(1).format(false), "1");
        assert_eq!(KapNumber::Long(-6).format(false), "-6");
        assert_eq!(KapNumber::Long(-6).format(true), "¯6");
    }

    #[test]
    fn double_keeps_dot_zero() {
        // User's correction: 1.0 is a Double and prints 1.0, not 1.
        assert_eq!(KapNumber::Double(1.0).format(false), "1.0");
        assert_eq!(KapNumber::Double(1.0).format(true), "1.0");
        assert_eq!(KapNumber::Double(2.1).format(false), "2.1");
        assert_eq!(KapNumber::Double(-0.5).format(true), "¯0.5");
    }

    #[test]
    fn rational_format() {
        let r = KapNumber::Rational(BigRational::new(1.into(), 2.into()));
        assert_eq!(r.format(false), "1/2");
        // Kotlin renders rationals as num/den in the REPL too (oracle: 1r2 -> 1/2).
        assert_eq!(r.format(true), "1/2");
        let neg = KapNumber::Rational(BigRational::new((-3).into(), 4.into()));
        assert_eq!(neg.format(true), "¯3/4");
    }

    #[test]
    fn complex_format() {
        let c = KapNumber::Complex(2.0, -7.0);
        assert_eq!(c.format(false), "2.0J-7.0");
        assert_eq!(c.format(true), "2.0J¯7.0");
    }

    #[test]
    fn as_long_errors_on_non_integer() {
        let r = KapNumber::Rational(BigRational::new(1.into(), 2.into()));
        assert!(r.as_long().is_err());
        assert_eq!(KapNumber::Long(42).as_long().unwrap(), 42);
    }

    #[test]
    fn comparison_across_types() {
        // 1 (Long) == 1.0 (Double)
        assert_eq!(
            KapNumber::Long(1).numeric_cmp(&KapNumber::Double(1.0)).unwrap(),
            Ordering::Equal
        );
        // 2 (Long) < 2.1 (Double)
        assert_eq!(
            KapNumber::Long(2).numeric_cmp(&KapNumber::Double(2.1)).unwrap(),
            Ordering::Less
        );
    }

    #[test]
    fn complex_not_orderable() {
        let c = KapNumber::Complex(1.0, 1.0);
        assert!(c.numeric_cmp(&KapNumber::Long(1)).is_err());
    }
}
