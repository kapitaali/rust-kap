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
use num_traits::ToPrimitive;
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
                if readable {
                    format!("{}r{}", num.replace('-', "¯"), den.replace('-', "¯"))
                } else {
                    format!("{}/{}", num, den)
                }
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
        assert_eq!(r.format(true), "1r2");
        let neg = KapNumber::Rational(BigRational::new((-3).into(), 4.into()));
        assert_eq!(neg.format(true), "¯3r4");
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
