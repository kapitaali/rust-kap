//! Native function API for Kap — trait + registry for `kap-ext-*` crates.
//!
//! This is the **public SDK** for writing Kap builtins in Rust. One file = one
//! Kap name, no `evaluator.rs` patch, no two-gate copy-paste. See
//! `docs/native-api-design.html` §04–§11 for the full RFC and
//! `kap-ext-stats/src/lib.rs` for a complete example.
//!
//! # Quick start (extension crate)
//!
//! ```toml
//! [dependencies]
//! kap-core = { path = "../kap-core" }
//! ```
//!
//! ```rust,ignore
//! use kap_core::native::prelude::*;
//! #[derive(Debug)] struct MyMean;
//! impl NativeFn for MyMean {
//!     fn name(&self) -> &str { "my:mean" }
//!     fn call(&self, _: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
//!         let v = args.mono_named("my:mean")?;
//!         // ... element logic
//!         Ok(Rc::new(APLValue::Number(KapNumber::Long(42))))
//!     }
//! }
//! inventory::submit! { NativeReg { name: "my:mean", factory: || Box::new(MyMean) } }
//! // kap-cli Cargo.toml adds `kap-ext-my = { path = "../kap-ext-my" }` — linking is registration.
//! ```

use std::rc::Rc;

use crate::{APLValue, AplError, AplRef, Engine, Environment};

/// Kap function valence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// `f y` — e.g. `⍳`, `≢`, `⊂`
    Monadic,
    /// `x f y` — e.g. `map:with` (dyadic only)
    Dyadic,
    /// Either — e.g. `+` (conjugate / add), `−`, `×`
    Ambivalent,
}

/// Rank contract — how the framework broadcasts before calling your element function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankSpec {
    /// You handle rank yourself (default, like `⍴` `⍉` `⌽` with axes).
    Any,
    /// Rank-0 elementwise with full scalar-extension. `Str` is rank-1 `[len]` (empty `Str` is rank-0).
    Scalar,
    /// Rank-1 only, else `Rank mismatch`.
    Vector,
    /// Exact rank (`Fixed(2)` = matrix-only).
    Fixed(usize),
}

/// Arguments after `Deferred`/`Dynamic` forcing. Use `args.mono()` / `args.dyad()` for
/// oracle-exact valence errors (`"⍳: requires one argument"` etc.).
#[derive(Debug)]
pub enum Args {
    Monad(AplRef<APLValue>),
    Dyad(AplRef<APLValue>, AplRef<APLValue>),
}

impl Args {
    pub fn mono(self) -> Result<AplRef<APLValue>, AplError> {
        match self {
            Args::Monad(v) => Ok(v),
            Args::Dyad(_, _) => Err(AplError::runtime("native: requires one argument".to_string())),
        }
    }
    pub fn dyad(self) -> Result<(AplRef<APLValue>, AplRef<APLValue>), AplError> {
        match self {
            Args::Dyad(a, b) => Ok((a, b)),
            Args::Monad(_) => Err(AplError::runtime("native: requires two arguments".to_string())),
        }
    }
    /// Variant that includes the Kap name in the error (preferred).
    pub fn mono_named(self, name: &str) -> Result<AplRef<APLValue>, AplError> {
        match self {
            Args::Monad(v) => Ok(v),
            Args::Dyad(_, _) => Err(AplError::runtime(format!("{}: requires one argument", name))),
        }
    }
    pub fn dyad_named(self, name: &str) -> Result<(AplRef<APLValue>, AplRef<APLValue>), AplError> {
        match self {
            Args::Dyad(a, b) => Ok((a, b)),
            Args::Monad(_) => Err(AplError::runtime(format!("{}: requires two arguments", name))),
        }
    }
}

/// Call context — the caller's `Engine` + `Environment` for the few fns that need it
/// (`isLocallyBound`, `use`, allocation via `engine`).
pub struct NativeContext<'a> {
    pub engine: &'a Engine,
    pub env: &'a AplRef<Environment>,
}

/// Object-safe trait for a Kap native function.
///
/// One impl = one Kap name (`"≢"`, `"+"`, `"map:with"`, `"stats:mean"`). The
/// registry is `HashMap<String, Box<dyn NativeFn>>` built at `Engine::new()`
/// via `inventory` (compile-time collection — linking an ext crate is registration).
pub trait NativeFn: std::fmt::Debug + 'static {
    /// Kap spelling as seen in source: `"⍳"`, `"+"`, `"map:with"`, `"stats:mean"`.
    fn name(&self) -> &str;

    /// Valence. Defaults to `Monadic` — most new fns are monadic; override for dyadic/ambivalent.
    fn arity(&self) -> Arity {
        Arity::Monadic
    }

    /// Rank contract. Defaults to `Any` (you handle rank). Use `Scalar` for elementwise.
    fn rank(&self) -> RankSpec {
        RankSpec::Any
    }

    /// `false` = omitted when `Engine::set_secure_mode(true)` (mirrors `is_secure_gated`).
    fn secure_ok(&self) -> bool {
        true
    }

    /// One-line help for `)help` / `kap --list-natives`.
    fn doc(&self) -> &str {
        ""
    }

    /// The only required method. `args` is already forced (`Deferred`/`Dynamic` collapsed).
    fn call(&self, ctx: &NativeContext<'_>, args: Args) -> Result<AplRef<APLValue>, AplError>;
}

/// Registry entry collected via `inventory`.
pub struct NativeReg {
    pub name: &'static str,
    pub factory: fn() -> Box<dyn NativeFn>,
}

// inventory collection — every `inventory::submit! { NativeReg { .. } }` in kap-core
// and in any linked `kap-ext-*` crate is visible here after linking.
inventory::collect!(NativeReg);

/// Prelude for extension crates: `use kap_core::native::prelude::*;`
pub mod prelude {
    pub use crate::native::{Args, Arity, NativeContext, NativeFn, NativeReg, RankSpec};
    pub use crate::{APLValue, AplError, AplRef, KapNumber};
    pub use std::rc::Rc;
}

/// Helpers shared by all native fns.
pub mod helpers {
    use super::*;
    use crate::array::{ArrayData, KapArray};

    pub fn ok(v: APLValue) -> Result<AplRef<APLValue>, AplError> {
        Ok(Rc::new(v))
    }
    pub fn ok_long(n: i64) -> Result<AplRef<APLValue>, AplError> {
        Ok(Rc::new(APLValue::Number(crate::KapNumber::Long(n))))
    }
    pub fn ok_bool(b: bool) -> Result<AplRef<APLValue>, AplError> {
        Ok(Rc::new(APLValue::Number(crate::KapNumber::Long(if b {
            1
        } else {
            0
        }))))
    }
    pub fn bad_arg(name: &str, msg: &str) -> AplError {
        AplError::runtime(format!("{}: {}", name, msg))
    }
    pub fn bad_type(name: &str, expected: &str) -> AplError {
        AplError::runtime(format!("{}: requires {}", name, expected))
    }
    pub fn valence_err(name: &str, expected: &str) -> AplError {
        AplError::runtime(format!("{}: {}", name, expected))
    }
    /// Scalar helper — `Str` is rank-1 `[len]` (empty is rank-0), scalars are rank-0.
    pub fn is_scalar_str(v: &APLValue) -> bool {
        matches!(v, APLValue::Str(s) if s.is_empty())
    }
}

/// Scalar broadcast helper — see `docs/native-api-design.html` §04.
pub mod scalar {
    use super::*;

    pub trait ScalarImpl {
        fn monad(&self, a: &APLValue) -> Result<APLValue, AplError>;
        fn dyad(&self, a: &APLValue, b: &APLValue) -> Result<APLValue, AplError>;
    }

    pub fn apply_scalar(
        imp: &dyn ScalarImpl,
        name: &str,
        args: Args,
    ) -> Result<AplRef<APLValue>, AplError> {
        match args {
            Args::Monad(v) => {
                let rank = v.rank();
                if rank == 0 {
                    let res = imp.monad(&v)?;
                    return Ok(Rc::new(res));
                }
                // Rank ≥1: per-element
                let elems = v.elements();
                let mut out = Vec::with_capacity(elems.len());
                for e in elems {
                    out.push(Rc::new(imp.monad(&e)?));
                }
                let dims = v.dimensions();
                let arr = crate::array::KapArray::new(dims, crate::array::ArrayData::Nested(out));
                Ok(Rc::new(APLValue::Array(Rc::new(arr))))
            }
            Args::Dyad(a, b) => {
                let ra = a.rank();
                let rb = b.rank();
                // Scalar extension: rank-0 broadcasts
                if ra == 0 && rb == 0 {
                    return Ok(Rc::new(imp.dyad(&a, &b)?));
                }
                if ra == 0 {
                    let elems = b.elements();
                    let mut out = Vec::with_capacity(elems.len());
                    for e in elems {
                        out.push(Rc::new(imp.dyad(&a, &e)?));
                    }
                    let dims = b.dimensions();
                    let arr =
                        crate::array::KapArray::new(dims, crate::array::ArrayData::Nested(out));
                    return Ok(Rc::new(APLValue::Array(Rc::new(arr))));
                }
                if rb == 0 {
                    let elems = a.elements();
                    let mut out = Vec::with_capacity(elems.len());
                    for e in elems {
                        out.push(Rc::new(imp.dyad(&e, &b)?));
                    }
                    let dims = a.dimensions();
                    let arr =
                        crate::array::KapArray::new(dims, crate::array::ArrayData::Nested(out));
                    return Ok(Rc::new(APLValue::Array(Rc::new(arr))));
                }
                // Both rank ≥1: elementwise, lengths must match
                let ea = a.elements();
                let eb = b.elements();
                if ea.len() != eb.len() {
                    return Err(AplError::runtime(format!(
                        "{}: length mismatch ({} vs {})",
                        name,
                        ea.len(),
                        eb.len()
                    )));
                }
                let mut out = Vec::with_capacity(ea.len());
                for (x, y) in ea.iter().zip(eb.iter()) {
                    out.push(Rc::new(imp.dyad(x, y)?));
                }
                // Result dims = left dims if same, else left (caller can override)
                let dims = a.dimensions();
                let arr = crate::array::KapArray::new(dims, crate::array::ArrayData::Nested(out));
                Ok(Rc::new(APLValue::Array(Rc::new(arr))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arity_helpers() {
        let v = Rc::new(APLValue::Number(crate::KapNumber::Long(1)));
        let m = Args::Monad(v.clone());
        assert!(m.mono_named("⍳").is_ok());
        let d = Args::Dyad(v.clone(), v.clone());
        assert!(d.dyad_named("+").is_ok());
    }
}
