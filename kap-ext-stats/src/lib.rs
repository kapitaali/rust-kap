//! `kap-ext-stats` — example Kap extension crate.
//!
//! This is the reference `kap-ext-*` crate from `docs/native-api-design.html` §11.
//! It adds three Kap primitives in the `stats:` namespace. Linking this crate is
//! registration — no `Engine::register()` call needed.
//!
//! ```toml
//! # kap-cli/Cargo.toml
//! [dependencies]
//! kap-ext-stats = { path = "../kap-ext-stats" }
//! ```
//!
//! Then in Kap:
//! ```kap
//! stats:mean 1 2 3 4      ⍝ 5/2
//! stats:median 1 2 3 4 5  ⍝ 3
//! stats:stdev 1 2 3 4     ⍝ 1.29…
//! ```

use std::rc::Rc;

use kap_core::native::helpers::{bad_arg, ok, ok_bool};
use kap_core::native::{Args, Arity, NativeContext, NativeFn, NativeReg, RankSpec};
use kap_core::{APLValue, AplError, AplRef, KapNumber};

// ---------------------------------------------------------------------------
// stats:mean — arithmetic mean
// ---------------------------------------------------------------------------

/// `stats:mean y` — mean of `y`. Monadic, rank-polymorphic (flattens).
/// Empty `⍬` → `⍬` (like Kap's `+/y ÷ ≢y` would, but without division-by-zero).
#[derive(Debug, Default)]
pub struct StatsMean;

impl NativeFn for StatsMean {
    fn name(&self) -> &str {
        "stats:mean"
    }
    fn arity(&self) -> Arity {
        Arity::Monadic
    }
    fn doc(&self) -> &str {
        "stats:mean y — arithmetic mean of y; ⍬ → ⍬"
    }
    fn call(&self, _ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
        let v = args.mono_named("stats:mean")?;
        let elems = v.elements();
        if elems.is_empty() {
            return ok(APLValue::Null);
        }
        // Fold with KapNumber::add — preserves BigInt/Rational exactness.
        let mut sum = KapNumber::Long(0);
        for e in &elems {
            let n = match e.as_ref() {
                APLValue::Number(n) => n.clone(),
                _ => return Err(bad_arg("stats:mean", "requires numbers")),
            };
            sum = sum.add(&n);
        }
        let n = KapNumber::Long(elems.len() as i64);
        let mean = sum.div(&n);
        ok(APLValue::Number(mean))
    }
}

inventory::submit! { NativeReg { name: "stats:mean", factory: || Box::new(StatsMean) } }

// ---------------------------------------------------------------------------
// stats:median — median (monadic)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct StatsMedian;

impl NativeFn for StatsMedian {
    fn name(&self) -> &str {
        "stats:median"
    }
    fn doc(&self) -> &str {
        "stats:median y — median of y (sorted, rank-flattened); ⍬ → ⍬"
    }
    fn call(&self, _ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
        let v = args.mono_named("stats:median")?;
        let elems = v.elements();
        if elems.is_empty() {
            return ok(APLValue::Null);
        }
        let mut nums: Vec<KapNumber> = Vec::with_capacity(elems.len());
        for e in &elems {
            match e.as_ref() {
                APLValue::Number(n) => nums.push(n.clone()),
                _ => return Err(bad_arg("stats:median", "requires numbers")),
            }
        }
        // Sort via KapNumber::number_ordering (type-discriminating numeric order).
        nums.sort_by(|a, b| KapNumber::number_ordering(a, b));
        let mid = nums.len() / 2;
        if nums.len() % 2 == 1 {
            ok(APLValue::Number(nums[mid].clone()))
        } else {
            // even: mean of two middles
            let a = nums[mid - 1].clone();
            let b = nums[mid].clone();
            let sum = a.add(&b);
            let two = KapNumber::Long(2);
            let med = sum.div(&two);
            ok(APLValue::Number(med))
        }
    }
}

inventory::submit! { NativeReg { name: "stats:median", factory: || Box::new(StatsMedian) } }

// ---------------------------------------------------------------------------
// stats:stdev — population standard deviation (monadic)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct StatsStdev;

impl NativeFn for StatsStdev {
    fn name(&self) -> &str {
        "stats:stdev"
    }
    fn doc(&self) -> &str {
        "stats:stdev y — population stdev of y; ⍬ → ⍬; requires ≥1 element"
    }
    fn call(&self, _ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
        let v = args.mono_named("stats:stdev")?;
        let elems = v.elements();
        if elems.is_empty() {
            return ok(APLValue::Null);
        }
        let nums: Vec<f64> = elems
            .iter()
            .map(|e| match e.as_ref() {
                APLValue::Number(n) => Ok(n.as_double()),
                _ => Err(bad_arg("stats:stdev", "requires numbers")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mean = nums.iter().sum::<f64>() / nums.len() as f64;
        let var = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
        let stdev = var.sqrt();
        ok(APLValue::Number(KapNumber::Double(stdev)))
    }
}

inventory::submit! { NativeReg { name: "stats:stdev", factory: || Box::new(StatsStdev) } }

// ---------------------------------------------------------------------------
// stats:contains — example dyadic (uses both args + scalar rank demo)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct StatsContains;

impl NativeFn for StatsContains {
    fn name(&self) -> &str {
        "stats:contains"
    }
    fn arity(&self) -> Arity {
        Arity::Dyadic
    }
    fn rank(&self) -> RankSpec {
        RankSpec::Any
    }
    fn doc(&self) -> &str {
        "x stats:contains y — 1 if y is in x (∊-like), else 0. Dyadic."
    }
    fn call(&self, _ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
        let (haystack, needle) = args.dyad_named("stats:contains")?;
        let hay = haystack.elements();
        let nd = needle.as_ref();
        // Needle is scalar for this demo; Kap's ∊ is vectorized but we keep it simple.
        // Use format_value for equality (APLValue doesn't derive PartialEq; total_cmp is numeric-focused).
        let needle_str = nd.format_value();
        let found = hay.iter().any(|e| e.format_value() == needle_str);
        ok_bool(found)
    }
}

inventory::submit! { NativeReg { name: "stats:contains", factory: || Box::new(StatsContains) } }

#[cfg(test)]
mod tests {
    use super::*;
    use kap_core::{APLValue, Engine};

    fn kap_one(s: &str) -> AplRef<APLValue> {
        // Minimal helper: build a single Kap value for unit tests without the full harness.
        // For scalar numbers / arrays we construct directly; for integration tests use
        // `Engine::eval_string`.
        match s {
            "1 2 3 4" => {
                let elems = vec![
                    Rc::new(APLValue::Number(KapNumber::Long(1))),
                    Rc::new(APLValue::Number(KapNumber::Long(2))),
                    Rc::new(APLValue::Number(KapNumber::Long(3))),
                    Rc::new(APLValue::Number(KapNumber::Long(4))),
                ];
                let arr = kap_core::array::KapArray::new(
                    vec![4],
                    kap_core::array::ArrayData::Nested(elems),
                );
                Rc::new(APLValue::Array(Rc::new(arr)))
            }
            _ => Rc::new(APLValue::Null),
        }
    }

    #[test]
    fn mean_direct_call() {
        let f = StatsMean;
        let ctx = NativeContext {
            engine: &Engine::new(),
            env: &kap_core::Environment::new_root(),
        };
        let v = kap_one("1 2 3 4");
        let got = f.call(&ctx, Args::Monad(v)).unwrap();
        // 1+2+3+4 = 10, /4 = 5/2
        assert_eq!(got.format_value(), "5/2");
    }

    #[test]
    fn median_direct_call() {
        let f = StatsMedian;
        let ctx = NativeContext {
            engine: &Engine::new(),
            env: &kap_core::Environment::new_root(),
        };
        let elems = vec![
            Rc::new(APLValue::Number(KapNumber::Long(3))),
            Rc::new(APLValue::Number(KapNumber::Long(1))),
            Rc::new(APLValue::Number(KapNumber::Long(2))),
        ];
        let arr = kap_core::array::KapArray::new(vec![3], kap_core::array::ArrayData::Nested(elems));
        let v = Rc::new(APLValue::Array(Rc::new(arr)));
        let got = f.call(&ctx, Args::Monad(v)).unwrap();
        assert_eq!(got.format_value(), "2");
    }
}
