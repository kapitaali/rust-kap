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

    /// True when the value is an integer type (Long / BigInt) or a Rational whose
    /// denominator divides its numerator exactly. Used by `*`-overflow detection.
    pub fn is_integer(&self) -> bool {
        match self {
            KapNumber::Long(_) | KapNumber::BigInt(_) => true,
            KapNumber::Rational(v) => v.is_integer(),
            _ => false,
        }
    }

    /// True for the floating-point (Double) representation.
    pub fn is_double(&self) -> bool {
        matches!(self, KapNumber::Double(_))
    }

    /// True for the rational (BigRational) representation.
    pub fn is_rational(&self) -> bool {
        matches!(self, KapNumber::Rational(_))
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

    /// Numeric value as an exact `BigRational` (Long/BigInt -> denominator 1;
    /// Double -> `double_to_rational`; Complex -> real part only).
    pub fn as_rational(&self) -> BigRational {
        match self {
            KapNumber::Long(v) => BigRational::new(num_bigint::BigInt::from(*v), num_bigint::BigInt::from(1)),
            KapNumber::BigInt(v) => BigRational::new(v.clone(), num_bigint::BigInt::from(1)),
            KapNumber::Rational(v) => v.clone(),
            KapNumber::Double(v) => Self::double_to_rational(*v),
            KapNumber::Complex(re, _) => BigRational::new(num_bigint::BigInt::from(*re as i64), num_bigint::BigInt::from(1)),
        }
    }

    /// Numeric value as `num_bigint::BigInt` (Long -> BigInt; BigInt -> self).
    pub fn as_bigint(&self) -> num_bigint::BigInt {
        match self {
            KapNumber::Long(v) => num_bigint::BigInt::from(*v),
            KapNumber::BigInt(v) => v.clone(),
            _ => unreachable!("as_bigint called on non-integral number"),
        }
    }

    /// Convert a finite Double to a `BigRational` exactly, mirroring Kotlin
    /// `Double.rationalise()`. Uses the IEEE-754 bit pattern (sign, biased exponent,
    /// mantissa) so a value like `1.6` becomes exactly `8/5`, not a lossy f64 ratio.
    fn double_to_rational(v: f64) -> BigRational {
        if v.is_infinite() || v.is_nan() {
            return BigRational::new(num_bigint::BigInt::from(0), num_bigint::BigInt::from(1));
        }
        let bits = v.to_bits();
        let sign = if bits >> 63 == 1 { -1i64 } else { 1i64 };
        let exp_biased = ((bits >> 52) & 0x7ff) as i32;
        let mant = bits & 0xfffffffffffff; // 52-bit fraction
        let (numer, denom): (num_bigint::BigInt, num_bigint::BigInt) =
            if exp_biased == 0 {
                // subnormal: value = ±(mant) * 2^(-1074)
                (num_bigint::BigInt::from(sign) * num_bigint::BigInt::from(mant), num_bigint::BigInt::from(1) << 1074)
            } else {
                let exp = exp_biased - 1075; // unbiased exponent for the (1+mant/2^52) form
                let top = num_bigint::BigInt::from(1) << 52 | num_bigint::BigInt::from(mant);
                let (n, d) = if exp >= 0 {
                    (num_bigint::BigInt::from(sign) * top * (num_bigint::BigInt::from(1) << exp), num_bigint::BigInt::from(1))
                } else {
                    (num_bigint::BigInt::from(sign) * top, num_bigint::BigInt::from(1) << (-exp))
                };
                (n, d)
            };
        BigRational::new(numer, denom)
    }

    /// Total-order comparison across number kinds, mirroring `number.kt`'s
    /// `numericCompare(reference, typeDiscrimination)`.
    ///
    /// When `type_discrimination` is true (used by `cmp`/`≡`/`≢`/`⍒⍋`), a numeric tie is
    /// broken by the *type*: `-0.0` sorts before `0`/`0.0`, and an equal-but-different-kind
    /// comparison returns `-1`. When false (used by `=`/`≠`/`<`/`>`), value equality wins
    /// so `10 = 10.0` and `0.5 = 1r2` are both true.
    ///
    /// Complex values with non-zero imaginary are NOT orderable → returns `Err` (Kap
    /// throws `APLArgumentComplexOrderingException`).
    pub fn numeric_cmp(
        &self,
        other: &KapNumber,
        type_discrimination: bool,
    ) -> Result<Ordering, String> {
        use KapNumber::*;
        // Complex with non-zero imaginary is not orderable.
        if (self.is_complex() && self.as_complex().1 != 0.0) || (other.is_complex() && other.as_complex().1 != 0.0) {
            return Err("complex numbers are not orderable".to_string());
        }
        // Normalize any complex with zero imaginary to its real part for ordering.
        let a = match self {
            Complex(re, 0.0) => Double(*re),
            other => other.clone(),
        };
        let b = match other {
            Complex(re, 0.0) => Double(*re),
            other => other.clone(),
        };
        // Delegate Long/BigInt/Rational/Double per the Kotlin `numericCompare` switch.
        let res = match (&a, &b) {
            (Long(x), Long(y)) => x.cmp(y),
            (Long(x), BigInt(y)) => num_bigint::BigInt::from(*x).cmp(y),
            (Long(x), Rational(y)) => BigRational::new(num_bigint::BigInt::from(*x), num_bigint::BigInt::from(1)).cmp(y),
            (Long(x), Double(y)) => Self::cmp_double_long(*y, *x, type_discrimination).reverse(),
            (BigInt(x), Long(y)) => x.cmp(&num_bigint::BigInt::from(*y)),
            (BigInt(x), BigInt(y)) => x.cmp(y),
            (BigInt(x), Rational(y)) => BigRational::new(x.clone(), num_bigint::BigInt::from(1)).cmp(y),
            (BigInt(x), Double(y)) => Self::cmp_double_bigint(*y, x, type_discrimination).reverse(),
            (Rational(x), Long(y)) => x.cmp(&BigRational::new(num_bigint::BigInt::from(*y), num_bigint::BigInt::from(1))),
            (Rational(x), BigInt(y)) => x.cmp(&BigRational::new(y.clone(), num_bigint::BigInt::from(1))),
            (Rational(x), Rational(y)) => x.cmp(y),
            (Rational(x), Double(y)) => Self::cmp_double_rational(*y, x, type_discrimination).reverse(),
            (Double(x), Long(y)) => Self::cmp_double_long(*x, *y, type_discrimination),
            (Double(x), BigInt(y)) => Self::cmp_double_bigint(*x, y, type_discrimination),
            (Double(x), Rational(y)) => Self::cmp_double_rational(*x, y, type_discrimination),
            (Double(x), Double(y)) => {
                if type_discrimination {
                    x.total_cmp(y)
                } else if x < y {
                    Ordering::Less
                } else if x > y {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
            _ => {
                return Err("complex numbers are not orderable".to_string());
            }
        };
        // Type discrimination is handled entirely inside the per-kind arms below
        // (each `cmp_double_*` returns `Less` on a td-tie, and the `(Long|BigInt|
        // Rational, Double)` arms negate that result to mirror Kotlin's `-compareDoubleTo*`)
        // — so no extra post-hoc flip is needed here.
        Ok(res)
    }

    /// Kap numeric type sort position (Kotlin `typeSortOrder`): Long=0, BigInt=1,
    /// Rational=2, Double=3, Complex=4.
    fn type_position(&self) -> usize {
        match self {
            KapNumber::Long(_) => 0,
            KapNumber::BigInt(_) => 1,
            KapNumber::Rational(_) => 2,
            KapNumber::Double(_) => 3,
            KapNumber::Complex(_, _) => 4,
        }
    }

    /// True when this number can be value-compared (Kotlin `numericCompareValid`):
    /// finite, and a complex only when its imaginary part is zero.
    fn numeric_compare_valid(&self) -> bool {
        match self {
            KapNumber::Complex(_, im) => *im == 0.0,
            KapNumber::Double(d) => d.is_finite(),
            _ => true,
        }
    }

    /// Total-ordering comparison between two `KapNumber`s, mirroring Kotlin's
    /// `compareTotalOrdering` (used by `cmp` and `≡`/`≢` with typeDiscrimination=true).
    ///
    /// Kotlin short-circuits when *both* operands are `numericCompareValid`; a complex with
    /// non-zero imaginary is **not** valid, which makes the pair fall through to the
    /// type-position branch (Complex=4, after Double=3) — so `2j3 cmp 1` → Greater and
    /// `cmp` never errors on complex. Only complex-vs-complex goes through component
    /// comparison (`compareSameType`). The `numeric_cmp` helper above is the value-level
    /// compare and must not be called for an invalid (im≠0) complex here.
    pub fn number_ordering(a: &KapNumber, b: &KapNumber) -> Ordering {
        let a_valid = a.numeric_compare_valid();
        let b_valid = b.numeric_compare_valid();
        if a_valid && b_valid {
            if let Ok(o) = a.numeric_cmp(b, true) {
                return o;
            }
        }
        if let (KapNumber::Complex(ar, ai), KapNumber::Complex(br, bi)) = (a, b) {
            // compareSameType for complex: imag first, then real (NaN-aware).
            let im = Self::cmp_doubles_nan(*ai, *bi);
            if im != Ordering::Equal {
                return im;
            }
            return Self::cmp_doubles_nan(*ar, *br);
        }
        // Distinct kinds (or an invalid complex) → by Kap type sort position
        // (Long=0, BigInt=1, Rational=2, Double=3, Complex=4).
        a.type_position().cmp(&b.type_position())
    }

    /// NaN-aware double compare matching Kotlin `compareDoublesNaNAware`.
    fn cmp_doubles_nan(a: f64, b: f64) -> Ordering {
        if a.is_nan() {
            if b.is_nan() {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        } else if b.is_nan() {
            Ordering::Less
        } else {
            a.total_cmp(&b)
        }
    }

    /// Whether two numbers are of exactly the same Kap numeric kind.
    fn same_kind(a: &KapNumber, b: &KapNumber) -> bool {
        matches!((a, b),
            (KapNumber::Long(_), KapNumber::Long(_))
            | (KapNumber::Double(_), KapNumber::Double(_))
            | (KapNumber::BigInt(_), KapNumber::BigInt(_))
            | (KapNumber::Rational(_), KapNumber::Rational(_)))
    }

    /// `Double` (a) vs `Long` (b): mirrors Kotlin `compareDoubleToLong`.
    /// The td-tiebreak (`res==0 && !sameKind → Less`) is applied uniformly to both
    /// the integer path and the rational path; the `-0.0` vs `0` case is handled
    /// explicitly first because casting `-0.0` to `i64` yields `0` and would tie.
    fn cmp_double_long(a: f64, b: i64, td: bool) -> Ordering {
        if !a.is_finite() {
            return if a > 0.0 { Ordering::Greater } else { Ordering::Less };
        }
        if td && a == -0.0 && b == 0 {
            return Ordering::Less;
        }
        let r = if a.fract() == 0.0 && a >= i64::MIN as f64 && a <= i64::MAX as f64 {
            (a as i64).cmp(&b)
        } else {
            Self::double_to_rational(a).cmp(&BigRational::new(num_bigint::BigInt::from(b), num_bigint::BigInt::from(1)))
        };
        if td && r == Ordering::Equal {
            Ordering::Less
        } else {
            r
        }
    }

    /// `Double` (a) vs `BigInt` (b): mirrors Kotlin `compareDoubleToBigint`.
    fn cmp_double_bigint(a: f64, b: &num_bigint::BigInt, td: bool) -> Ordering {
        if !a.is_finite() {
            return if a > 0.0 { Ordering::Greater } else { Ordering::Less };
        }
        if td && a == -0.0 && *b == num_bigint::BigInt::from(0) {
            return Ordering::Less;
        }
        let r = if a.fract() == 0.0 && a >= i64::MIN as f64 && a <= i64::MAX as f64 {
            num_bigint::BigInt::from(a as i64).cmp(b)
        } else {
            Self::double_to_rational(a).cmp(&BigRational::new(b.clone(), num_bigint::BigInt::from(1)))
        };
        if td && r == Ordering::Equal {
            Ordering::Less
        } else {
            r
        }
    }

    /// `Double` (a) vs `Rational` (b): mirrors Kotlin `compareDoubleToRational`.
    fn cmp_double_rational(a: f64, b: &BigRational, td: bool) -> Ordering {
        if !a.is_finite() {
            return if a > 0.0 { Ordering::Greater } else { Ordering::Less };
        }
        if td && a == -0.0 && *b == BigRational::new(num_bigint::BigInt::from(0), num_bigint::BigInt::from(1)) {
            return Ordering::Less;
        }
        let r = Self::double_to_rational(a).cmp(b);
        if td && r == Ordering::Equal {
            Ordering::Less
        } else {
            r
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
                // A whole-number rational renders as a bare integer in Kap (oracle:
                // `1r2 + 1r2` -> `1`, `2r4` -> `1/2`, `3r3` -> `1`). The `num/den`
                // form is used only when the denominator is not 1.
                if *v.denom() == num_bigint::BigInt::from(1) {
                    let s = v.numer().to_string();
                    neg(if readable {
                        s.replace('-', "¯")
                    } else {
                        s
                    })
                } else {
                    let num = v.numer().to_string();
                    let den = v.denom().to_string();
                    neg(format!("{}/{}", num, den))
                }
            }
            KapNumber::Complex(re, im) => {
                // APL `J` notation: `re Jim` (Kotlin `formatComplex`).
                // A zero imaginary part still renders `J0.0` (e.g. `2j0` -> `2.0J0.0`),
                // and a positive imaginary part carries NO `+` sign (e.g. `2j3÷1j1`
                // -> `2.5J0.5`).
                let rs = format_double(*re);
                let ims = format_double(*im);
                neg(format!("{}J{}", rs, ims))
            }
        }
    }
}

impl std::fmt::Display for KapNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Non-readable form (plain ASCII minus signs) — used for error messages that
        // must match the JVM oracle's "Value does not fit in an int: <n>" text.
        write!(f, "{}", self.format(false))
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
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    // Rust's f64 Display (Ryu) yields the shortest round-trip decimal, in either plain
    // or lowercase-scientific form (e.g. "123456789", "0.0001", "1e-7",
    // "6.123233995736766e-17"). Kotlin's `Double.formatDouble()` is Java
    // `Double.toString()`, which renders scientifically whenever |v| < 1e-3 || |v| >= 1e7,
    // uses uppercase 'E', and puts a ".0" on the mantissa when it has no fractional
    // part. Normalise to that convention here so output matches the Kotlin oracle.
    let s = format!("{}", v); // shortest round-trip — same digits Java emits
    let av = v.abs();
    let force_exp = av < 1e-3 || av >= 1e7;
    if let Some(idx) = s.bytes().position(|b| b == b'e' || b == b'E') {
        // Already scientific: uppercase the marker, ensure mantissa has a '.'.
        let (mant, exp) = s.split_at(idx);
        let exp = &exp[1..]; // drop 'e'/'E' (sign is preserved in exp)
        let mant = if mant.contains('.') {
            mant.to_string()
        } else {
            format!("{}.0", mant)
        };
        format!("{}E{}", mant, exp)
    } else if force_exp {
        let (sign, body) = if s.starts_with('-') {
            ("-", &s[1..])
        } else {
            ("", s.as_ref())
        };
        if body.starts_with("0.") {
            // 0.0001 -> 1.0E-4
            let frac = &body[2..];
            let zeros = frac.bytes().take_while(|b| *b == b'0').count();
            let digits = &frac[zeros..];
            let first = &digits[0..1];
            let rest = &digits[1..];
            let mant = if rest.is_empty() {
                format!("{}.0", first)
            } else {
                format!("{}.{}", first, rest)
            };
            let p = -((zeros + 1) as i32);
            format!("{}{}E{}", sign, mant, p)
        } else {
            // integer >= 1e7: 123456789 -> 1.23456789E8
            let p = (body.len() as i32) - 1;
            let first = &body[0..1];
            let rest = &body[1..];
            let mant = if rest.is_empty() {
                format!("{}.0", first)
            } else {
                format!("{}.{}", first, rest)
            };
            format!("{}{}E{}", sign, mant, p)
        }
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
            // Mixed Long/Double: operand order matters. `self - other`:
            //   Long - Double ⇒ a - b;  Double - Long ⇒ b - a.
            (Long(a), Double(b)) => Double(*a as f64 - *b),
            (Double(a), Long(b)) => Double(*a - *b as f64),
            (Double(a), Double(b)) => Double(a - b),
            (BigInt(a), BigInt(b)) => BigInt(a - b),
            (Long(a), BigInt(b)) => BigInt(num_bigint::BigInt::from(*a) - b.clone()),
            (BigInt(a), Long(b)) => BigInt(a.clone() - num_bigint::BigInt::from(*b)),
            (Rational(a), Rational(b)) => Rational(a - b),
            (Long(a), Rational(b)) => Rational(
                BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(1)) - b,
            ),
            (Rational(b), Long(a)) => Rational(
                b - BigRational::new(num_bigint::BigInt::from(*a), num_bigint::BigInt::from(1)),
            ),
            (Complex(ar, ai), Complex(br, bi)) => Complex(ar - br, ai - bi),
            (Long(a), Complex(br, bi)) => Complex(*a as f64 - *br, -*bi),
            (Complex(br, bi), Long(a)) => Complex(*br - *a as f64, *bi),
            (Double(a), Complex(br, bi)) => Complex(*a - *br, -*bi),
            (Complex(br, bi), Double(a)) => Complex(*br - *a, *bi),
            _ => Double(self.as_double() - other.as_double()),
        }
    }

    /// Division. Faithful port of Kotlin `DivAPLFunction.combine2Arg` /
    /// `numericRelationOperation2` dispatch order:
    /// Long/Long -> Long or Rational; ANY Complex operand -> complex divide;
    /// ANY Double operand -> double divide (0.0/0.0 -> 0.0); Rational -> rational
    /// divide (divisor zero -> 0); BigInt -> bigint divide (divisor zero -> 0).
    /// Division by zero follows the oracle: real/0 -> 0, Double/0 -> Infinity
    /// (0.0/0.0 -> 0.0), complex/0 -> NaNJNaN unless both are exactly zero -> 0.0.
    pub fn div(&self, other: &KapNumber) -> KapNumber {
        use KapNumber::*;
        // Long ÷ Long special-cased (matches Kotlin's fnLong branch).
        if let (Long(a), Long(b)) = (self, other) {
            if *b == 0 {
                return Long(0);
            }
            if *a == i64::MIN && *b == -1 {
                return BigInt(num_bigint::BigInt::from(i64::MAX) + num_bigint::BigInt::from(1));
            }
            if a % b == 0 {
                return Long(a / b);
            }
            return Rational(BigRational::new(
                num_bigint::BigInt::from(*a),
                num_bigint::BigInt::from(*b),
            ));
        }
        // Complex operand (either side) -> complex divide. Match the *variant* so that
        // `0j0` (which has im == 0) still routes here, not into the bigint path.
        if matches!(self, Complex(_, _)) || matches!(other, Complex(_, _)) {
            let (ar, ai) = self.as_complex();
            let (br, bi) = other.as_complex();
            let both_zero = ar == 0.0 && ai == 0.0 && br == 0.0 && bi == 0.0;
            if both_zero {
                return Double(0.0);
            }
            let den = br * br + bi * bi;
            if den == 0.0 {
                return Complex(f64::NAN, f64::NAN);
            }
            return Complex((ar * br + ai * bi) / den, (ai * br - ar * bi) / den);
        }
        // Double operand (either side) -> double divide.
        if self.is_double() || other.is_double() {
            let a = self.as_double();
            let b = other.as_double();
            if a == 0.0 && b == 0.0 {
                return Double(0.0);
            }
            return Double(a / b);
        }
        // Rational operand (either side) -> rational divide.
        if self.is_rational() || other.is_rational() {
            let a = self.as_rational();
            let b = other.as_rational();
            if *b.numer() == num_bigint::BigInt::from(0) {
                return Long(0);
            }
            return Rational(a / b);
        }
        // BigInt ÷ BigInt.
        let a = self.as_bigint();
        let b = other.as_bigint();
        if b == num_bigint::BigInt::from(0) {
            return Long(0);
        }
        if (&a % &b) == num_bigint::BigInt::from(0) {
            return BigInt(&a / &b);
        }
        Rational(BigRational::new(a, num_bigint::BigInt::from(1))
            / BigRational::new(b, num_bigint::BigInt::from(1)))
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
/// Two's-complement population count for a non-negative `BigInt` (matches Kotlin's
/// `BigInt.popcnt()`). Caller must pass the unsigned magnitude; for negative inputs
/// Kotlin uses `popcnt(-1 - a)`, which the caller computes before calling here.
pub fn popcount_bigint(v: &BigInt) -> u64 {
    // Count set bits across the little-endian byte view.
    v.to_bytes_le().1.iter().map(|byte| byte.count_ones() as u64).sum()
}

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
    /// Reduce an exact-integer rational to an integer, mirroring Kotlin's
    /// `makeAPLNumberWithReduction()` (used by the rational branches of `⌊`/`⌈` —
    /// math_functions.kt:1278/:1341). A `BigRational` with denominator 1 becomes a
    /// `Long` when it fits, else a `BigInt`; anything else stays `Rational`.
    /// Without this, `⌈3÷2` was typed `kap:rational` where the oracle says
    /// `kap:integer`, which broke integer-requiring consumers such as
    /// `(⌈3÷2)↑ ⍳6` ("↑/↓ counts must be integers").
    pub fn reduce_rational(v: BigRational) -> KapNumber {
        use KapNumber::*;
        if *v.denom() == num_bigint::BigInt::from(1) {
            let n = v.numer().clone();
            match n.to_string().parse::<i64>() {
                Ok(l) => Long(l),
                Err(_) => BigInt(n),
            }
        } else {
            Rational(v)
        }
    }

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
                // Kotlin uses `makeAPLNumberWithReduction()` for the rational branch
                // (math_functions.kt:1341), which REDUCES an exact-integer rational to
                // an integer. Returning a bare `Rational` here left `⌈3÷2` typed
                // `kap:rational` while the oracle says `kap:integer`, which then broke
                // consumers that require an integer (`(⌈3÷2)↑ ⍳6` → "↑/↓ counts must
                // be integers"). Mirrors the whole-Double normalisation above.
                Self::reduce_rational(-c)
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
            // Kotlin's rational branch also reduces (math_functions.kt:1278).
            Rational(v) => Self::reduce_rational(v.floor()),
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
    /// Oracle behaviour (math_functions.kt PowerAPLFunction):
    /// - The result must fit an integer (Long) for integer base + integer non-negative
    ///   exponent; if it would overflow (e.g. `10*1234567890123457`) Kotlin throws
    ///   "Value does not fit in an int".
    /// - Negative integer powers of an integer base yield a *rational* (`2*¯3 → 1/8`,
    ///   not `0.125`).
    pub fn pow(&self, exp: &KapNumber) -> KapNumber {
        use KapNumber::*;
        match (self, exp) {
            // Positive integer power of an integer base: stay exact via BigInt so large
            // but representable results (`10*100`) are correct, and overflow (exponent
            // too large to even attempt) errors like Kotlin.
            (Long(a), Long(b)) if *b >= 0 => {
                if let Some(e) = u32::try_from(*b).ok() {
                    let big = num_bigint::BigInt::from(*a).pow(e);
                    return bigint_to_kap(&big);
                }
                // Exponent is a positive i64 too large to fit u32 — result cannot fit an int.
                Double(f64::INFINITY)
            }
            (Long(a), Long(b)) => {
                // Negative integer power of an integer base → rational 1/(a^|b|).
                if let Some(e) = u32::try_from(-*b).ok() {
                    let denom = num_bigint::BigInt::from(*a).pow(e);
                    return Rational(num_rational::BigRational::new(
                        num_bigint::BigInt::from(1),
                        denom,
                    ));
                }
                // Exponent magnitude too large to attempt as an exact integer power.
                Double(f64::INFINITY)
            }
            (BigInt(a), Long(b)) if *b >= 0 => {
                if let Some(e) = u32::try_from(*b).ok() {
                    return bigint_to_kap(&a.pow(e));
                }
                Double(f64::INFINITY)
            }
            (BigInt(a), Long(b)) => {
                if let Some(e) = u32::try_from(-*b).ok() {
                    let denom = a.pow(e);
                    return Rational(num_rational::BigRational::new(
                        num_bigint::BigInt::from(1),
                        denom,
                    ));
                }
                Double(f64::INFINITY)
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
                    // Kotlin SqrtAPLFunction.sqrtDouble routes negatives through
                    // `x.toComplex().pow(Complex.ONE_HALF)`, i.e. the polar form. For
                    // `√¯1` that yields `cos(π/2)J sin(π/2)` = `6.12e-17J1.0` (a tiny
                    // real-part artifact), NOT a clean `0.0J1.0`. Reuse the same
                    // polar computation so the display matches the oracle.
                    let z = (x, 0.0);
                    let (mag, arg) = (babs(z), barg(z));
                    let rm = mag.sqrt();
                    KapNumber::Complex(rm * bcos(arg * 0.5), rm * bsin(arg * 0.5))
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
        // Kotlin ModAPLFunction.opLong/opDouble: `if (x==0) y else (y%x).let {
        // if ((x<0)!=(y<0) && result!=0) x+result else result }`. NOT rem_euclid:
        // the sign of the result follows the DIVISOR (x), not the dividend (y).
        use KapNumber::*;
        match (self, other) {
            (Long(a), Long(b)) => {
                if *a == 0 {
                    Long(*b)
                } else {
                    let result = *b % *a;
                    if (*a < 0) != (*b < 0) && result != 0 {
                        Long(a + result)
                    } else {
                        Long(result)
                    }
                }
            }
            (Long(a), BigInt(b)) => {
                if *a == 0 {
                    BigInt(b.clone())
                } else {
                    let result = b % *a;
                    // NOTE: `BigInt` here shadows num_bigint::BigInt (use KapNumber::* above),
                    // so use the full path for the type.
                    if (*a < 0) != (&result < &num_bigint::BigInt::from(0))
                        && result != num_bigint::BigInt::from(0)
                    {
                        BigInt(num_bigint::BigInt::from(*a) + &result)
                    } else {
                        BigInt(result)
                    }
                }
            }
            _ => {
                let a = self.as_double();
                let b = other.as_double();
                if a == 0.0 {
                    // Kotlin: `if (xSign == 0) y` — return the dividend with its ORIGINAL
                    // type, not converted to Double. `0 | 5` → `5` (Long), not `5.0`.
                    return other.clone();
                }
                let result = b % a;
                if (a < 0.0) != (b < 0.0) && result != 0.0 {
                    Double(a + result)
                } else {
                    Double(result)
                }
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
            KapNumber::Long(1).numeric_cmp(&KapNumber::Double(1.0), false).unwrap(),
            Ordering::Equal
        );
        // 2 (Long) < 2.1 (Double)
        assert_eq!(
            KapNumber::Long(2).numeric_cmp(&KapNumber::Double(2.1), false).unwrap(),
            Ordering::Less
        );
    }

    #[test]
    fn complex_not_orderable() {
        let c = KapNumber::Complex(1.0, 1.0);
        assert!(c.numeric_cmp(&KapNumber::Long(1), false).is_err());
    }
}

