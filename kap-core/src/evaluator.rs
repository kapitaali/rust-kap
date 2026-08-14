//! Evaluator for Kap (Phase 3 + 4).
//!
//! `Engine::eval_string` runs the pipeline: tokenise -> parse -> eval. Laziness (D6):
//! args are `Instr` trees; they are forced via `force()` only when a builtin needs them.
//! A starter set of builtins is wired in: arithmetic (`+ - * × ÷`), comparisons
//! (`= ≠ < > ≤ ≥`), structural (`⍳ rho tally first`, `,` catenate, `⌽/⊖` reverse,
//! `⍉` transpose, `↑/↓` take/drop, `⊂` enclose), assignment `←`, and user lambdas
//! (`λ(params) body`). More in later phases.

use crate::array::{ArrayData, KapArray};
use crate::ast::Instr;
use crate::lexer::tokenise;
use crate::number::KapNumber;
use crate::parser;
use crate::token::LiteralValue;
use std::cmp::Ordering;
use crate::{APLValue, AplError, AplRef, Engine, Environment};
use std::collections::HashMap;
use std::rc::Rc;

impl Environment {
    /// Look up a symbol, walking parent environments. Returns a shared ref to the value.
    pub fn lookup(&self, name: &str, ns: &Option<String>) -> Option<AplRef<APLValue>> {
        let key = (name.to_string(), ns.clone());
        if let Some(v) = self.symbols.borrow().get(&key) {
            return Some(v.clone());
        }
        if let Some(p) = &self.parent {
            return p.lookup(name, ns);
        }
        None
    }

    /// Define a symbol in this (innermost) environment. Mutates through `RefCell` so the
    /// definition is visible to all shared `Rc<Environment>` handles (assignment persists).
    pub fn define(&self, name: &str, ns: &Option<String>, value: AplRef<APLValue>) {
        self.symbols
            .borrow_mut()
            .insert((name.to_string(), ns.clone()), value);
    }
}

impl APLValue {
    /// Force a (possibly deferred) value. For a `Deferred`, evaluates its `instr` in its
    /// captured `env` (call-by-need: not memoised yet — see D6 revisit). Non-deferred
    /// values are returned unchanged.
    pub fn force(&self, engine: &Engine) -> Result<AplRef<APLValue>, AplError> {
        match self {
            APLValue::Deferred { instr, env } => {
                let env = env.clone();
                engine.eval_instr(instr, &env)
            }
            other => Ok(Rc::new(other.clone())),
        }
    }
}

impl Engine {
    /// Stateless "single expression" mode (Mode 1): evaluate `src` in a *fresh*
    /// environment. Any variables assigned inside `src` do not persist.
    /// For persistent state across evaluations, use [`crate::Session`] instead.
    pub fn eval_string(&self, src: &str) -> Result<AplRef<APLValue>, AplError> {
        let env = Rc::new(Environment::default());
        self.eval_string_in_env(src, &env)
    }

    /// Convenience (Mode 1): stateless eval returning the formatted REPL-style string.
    pub fn eval_to_string(&self, src: &str) -> Result<String, AplError> {
        Ok(self.eval_string(src)?.format_value())
    }

    /// Shared evaluation core. Runs `src` in the given `env`. Variables assigned
    /// here persist in `env` — this is what [`crate::Session`] relies on to keep
    /// state across calls.
    pub fn eval_string_in_env(
        &self,
        src: &str,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let toks = tokenise(src);
        let (stmts, errs) = parser::parse(&toks);
        if !errs.is_empty() {
            // Surface the first error with its real source position.
            return Err(errs.into_iter().next().unwrap());
        }
        let mut last: AplRef<APLValue> = Rc::new(APLValue::Null);
        for stmt in &stmts {
            last = self.eval_instr(stmt, env)?;
        }
        Ok(last)
    }

    /// Evaluate a single `Instr` in `env`. This is the core eval loop.
    pub fn eval_instr(
        &self,
        instr: &Instr,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match instr {
            Instr::Literal(LiteralValue::Number(n)) => Ok(Rc::new(APLValue::Number(n.clone()))),
            Instr::Literal(LiteralValue::Char(c)) => Ok(Rc::new(APLValue::Char(*c))),
            Instr::Literal(LiteralValue::Str(s)) => Ok(Rc::new(APLValue::Str(s.clone()))),
            Instr::Literal(LiteralValue::Symbol { .. }) => Err(AplError::runtime("lone symbol literal".into())),
            Instr::Empty => Ok(Rc::new(APLValue::Null)),
            Instr::Symbol { name, namespace } => {
                let found = env.lookup(name, namespace).ok_or_else(|| AplError::runtime(format!("undefined symbol: {}", name)))?;
                // clone the inner value out of the shared ref
                Ok(Rc::new(found.as_ref().clone()))
            }
            Instr::Array { elements } => {
                let mut vals = Vec::with_capacity(elements.len());
                for e in elements {
                    vals.push(self.eval_instr(e, env)?);
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![vals.len()],
                    ArrayData::Nested(vals),
                )))))
            }
            Instr::Lambda { params, body } => Ok(Rc::new(APLValue::UserFn {
                params: params.clone(),
                body: Rc::new(*body.clone()),
                env: env.clone(),
            })),
            Instr::Assign { target, value } => {
                if let Instr::Symbol { name, namespace } = target.as_ref() {
                    let v = self.eval_instr(value, env)?;
                    env.define(name, namespace, v.clone());
                    Ok(v)
                } else {
                    Err(AplError::runtime("assignment target must be a symbol".into()))
                }
            }
            Instr::Apply {
                fn_expr,
                left,
                right,
            } => self.eval_apply(fn_expr, left, right, env),
            Instr::Derived { .. } => Err(AplError::runtime(
                "derived function used without an argument".into(),
            )),
        }
    }

    fn eval_apply(
        &self,
        fn_expr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Resolve the function: a builtin name, a direct lambda, or a user function
        // bound to a symbol.
        let fn_name: Option<String> = match fn_expr {
            Instr::Symbol { name, .. } => Some(name.clone()),
            _ => None,
        };
        let lambda = match fn_expr {
            Instr::Lambda { params, body } => Some((params.clone(), Rc::new((**body).clone()))),
            Instr::Symbol { name, namespace } => match env.lookup(name, namespace) {
                Some(v) if matches!(v.as_ref(), APLValue::UserFn { .. }) => {
                    if let APLValue::UserFn { params, body, env: fenv } = v.as_ref() {
                        Some((params.clone(), Rc::new((**body).clone())))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((params, body)) = lambda {
            return self.apply_user_fn(&params, &body, left, right, env);
        }
        // --- Derived functions from adverbs (e.g. `+/`, `×¨`) ---
        // `fn_expr` is an `Instr::Derived { func, op }`; `op` is the adverb name (`/`,
        // `\`, `¨`) and `func` is the function operand. `left`/`right` are the data
        // arguments (dyadic each has both; reduce/scan/each usually just `right`).
        if let Instr::Derived { func, op } = fn_expr {
            let adv_name = match op.as_ref() {
                Instr::Symbol { name, .. } => name.clone(),
                _ => return Err(AplError::runtime("adverb must be a symbol".into())),
            };
            return match adv_name.as_str() {
                "/" | "reduce" => self.adverb_reduce(func, left, right, env),
                "\\" | "scan" => self.adverb_scan(func, left, right, env),
                "¨" | "each" => self.adverb_each(func, left, right, env),
                other => Err(AplError::runtime(format!("unknown adverb: {}", other))),
            };
        }
        let name = match fn_name {
            Some(n) => n,
            None => {
                return Err(AplError::runtime("only symbol/lambda functions supported yet".into()))
            }
        };
        // For dyadic, force left then right; for monadic, only right.
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        match name.as_str() {
            "+" => {
                // Ambivalent: monadic `+ x` = identity (return x); dyadic = add.
                match left_val {
                    None => Ok(right_val),
                    Some(_) => self.num2(left_val, right_val, |a, b| a.add(b), "+"),
                }
            }
            "-" => {
                // Ambivalent: monadic `- x` = negate; dyadic = subtract.
                match left_val {
                    None => self.negate(right_val),
                    Some(_) => self.num2(left_val, right_val, |a, b| a.sub(b), "-"),
                }
            }
            "÷" | "/" => self.num2(left_val, right_val, |a, b| a.div(b), "÷"),
            "=" => self.cmp2(left_val, right_val, |o| o == Ordering::Equal, "="),
            "≠" => self.cmp2(left_val, right_val, |o| o != Ordering::Equal, "≠"),
            "<" => self.cmp2(left_val, right_val, |o| o == Ordering::Less, "<"),
            ">" => self.cmp2(left_val, right_val, |o| o == Ordering::Greater, ">"),
            "≤" => self.cmp2(left_val, right_val, |o| o != Ordering::Greater, "≤"),
            "≥" => self.cmp2(left_val, right_val, |o| o != Ordering::Less, "≥"),
            "⍳" | "iota" => self.iota(right_val),
            "⍴" | "rho" => self.shape(right_val),
            "≢" | "tally" => self.tally(right_val),
            "⊃" | "first" => self.first(right_val),
            "," => self.catenate(left_val, right_val),
            "⌽" | "⊖" => self.reverse(right_val),
            "⍉" => self.transpose(right_val),
            "↑" => self.take(left_val, right_val),
            "↓" => self.drop(left_val, right_val),
            "⊂" => self.enclose(right_val),
            // --- more builtins (Phase 6) ---
            "⌈" | "ceil" => match left_val {
                None => self.scalar1(right_val, |x| x.ceil(), "⌈"),
                Some(_) => self.num2(left_val, right_val, |a, b| {
                    match a.numeric_cmp(&b) {
                        Ok(Ordering::Greater) => a.clone(),
                        Ok(_) => b.clone(),
                        // Mismatched numeric kinds: fall back to the right operand.
                        Err(_) => b.clone(),
                    }
                }, "⌈"),
            },
            "⌊" | "floor" => match left_val {
                None => self.scalar1(right_val, |x| x.floor(), "⌊"),
                Some(_) => self.num2(left_val, right_val, |a, b| {
                    match a.numeric_cmp(&b) {
                        Ok(Ordering::Less) => a.clone(),
                        Ok(_) => b.clone(),
                        Err(_) => b.clone(),
                    }
                }, "⌊"),
            },
            "|" | "mod" => self.num2(left_val, right_val, |a, b| a.modulo(b), "|"),
            "*" | "×" => match left_val {
                None => self.scalar1(right_val, |x| x.exp(), "*"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.mul(b), "*"),
            },
            "⍟" | "log" => match left_val {
                None => self.scalar1(right_val, |x| x.nat_log(), "⍟"),
                Some(_) => self.num2(left_val, right_val, |a, b| b.log(a), "⍟"),
            },
            "∧" => self.bool2(left_val, right_val, |a, b| a & b, "∧"),
            "∨" => self.bool2(left_val, right_val, |a, b| a | b, "∨"),
            "~" | "not" => self.scalar1(right_val, |x| x.not(), "~"),
            "∊" | "in" => self.membership(left_val, right_val),
            "⍋" | "grade" => self.grade_up(right_val),
            "⊤" | "encode" => self.encode(left_val, right_val),
            "⊥" | "decode" => self.decode(left_val, right_val),
            _ => Err(AplError::runtime(format!("unknown function: {}", name))),
        }
    }

    /// Apply a user-defined lambda. Dyadic: left=first param, right=second. Monadic:
    /// right=first param. Builds a child scope from the closure env and binds params.
    fn apply_user_fn(
        &self,
        params: &[String],
        body: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        closure_env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let child = Rc::new(Environment {
            symbols: Default::default(),
            parent: Some(closure_env.clone()),
        });
        // Evaluate args in the *calling* env (Kap passes by value/sharing).
        let right_val = self.eval_instr(right, closure_env)?.force(self)?;
        let mut i = 0;
        if let Some(l) = left {
            let left_val = self.eval_instr(l, closure_env)?.force(self)?;
            if i < params.len() {
                child.define(&params[i], &None, left_val);
                i += 1;
            }
        }
        if i < params.len() {
            child.define(&params[i], &None, right_val);
        }
        self.eval_instr(body, &child)
    }

    // --- helpers ---------------------------------------------------------------

    fn num2(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        f: impl Fn(&KapNumber, &KapNumber) -> KapNumber,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        match (a.as_ref(), right_val.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => {
                Ok(Rc::new(APLValue::Number(f(x, y))))
            }
            // Scalar extension: array <op> scalar, scalar <op> array, array <op> array.
            (APLValue::Array(xa), APLValue::Number(y)) | (APLValue::Number(y), APLValue::Array(xa)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                for e in xa.elements() {
                    if let APLValue::Number(x) = e.as_ref() {
                        out.push(Rc::new(APLValue::Number(f(x, y))));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            (APLValue::Array(xa), APLValue::Array(ya)) => {
                let mut out = Vec::with_capacity(xa.element_count());
                let ye = ya.elements();
                for (i, e) in xa.elements().into_iter().enumerate() {
                    if let (APLValue::Number(x), APLValue::Number(y)) = (e.as_ref(), ye[i].as_ref()) {
                        out.push(Rc::new(APLValue::Number(f(x, y))));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    xa.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            _ => Err(AplError::runtime(format!("{} requires numbers", sym))),
        }
    }

    /// Monadic negation: `- x` over a number or an array of numbers (scalar extension).
    fn negate(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Number(x) => Ok(Rc::new(APLValue::Number(x.neg()))),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    if let APLValue::Number(x) = e.as_ref() {
                        out.push(Rc::new(APLValue::Number(x.neg())));
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    a.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            _ => Err(AplError::runtime("- requires a number".into())),
        }
    }

    fn cmp2(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        pred: impl Fn(Ordering) -> bool,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let ord = match (a.as_ref(), right_val.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => x.numeric_cmp(y).map_err(|e| {
                AplError::runtime(e)
            })?,
            _ => {
                return Err(AplError::runtime(format!("{} requires numbers", sym)))
            }
        };
        // Kap booleans are 1 (true) / 0 (false).
        Ok(Rc::new(APLValue::Number(KapNumber::Long(if pred(ord) { 1 } else { 0 }))))
    }

    fn iota(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let n = match right_val.as_ref() {
            APLValue::Number(KapNumber::Long(v)) => *v,
            _ => {
                return Err(AplError::runtime("⍳ needs an integer count".into()))
            }
        };
        if n < 0 {
            return Err(AplError::runtime("⍳ count must be non-negative".into()));
        }
        let nums: Vec<KapNumber> = (0..n).map(KapNumber::Long).collect();
        let arr = KapArray::from_numbers(nums);
        Ok(Rc::new(APLValue::Array(Rc::new(arr))))
    }

    fn shape(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Array(a) => {
                let shape: Vec<AplRef<APLValue>> = a
                    .dimensions
                    .iter()
                    .map(|d| Rc::new(APLValue::Number(KapNumber::Long(*d as i64))))
                    .collect();
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![shape.len()],
                    ArrayData::Nested(shape),
                )))))
            }
            _ => Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                vec![0],
                ArrayData::Nested(vec![]),
            ))))),
        }
    }

    fn tally(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Array(a) => Ok(Rc::new(APLValue::Number(KapNumber::Long(
                a.element_count() as i64,
            )))),
            _ => Ok(Rc::new(APLValue::Number(KapNumber::Long(1)))),
        }
    }

    fn first(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Array(a) => a
                .elements()
                .into_iter()
                .next()
                .ok_or_else(|| AplError::runtime("⊃ of empty array".into())),
            other => Ok(Rc::new(other.clone())),
        }
    }

    fn catenate(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| {
            AplError::runtime(", needs two args".into())
        })?;
        let mut elems = Vec::new();
        self.collect_elements(&a, &mut elems);
        self.collect_elements(&right_val, &mut elems);
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![elems.len()],
            ArrayData::Nested(elems),
        )))))
    }

    /// Flatten a value's elements for catenation/stranding: scalars become a 1-element
    /// list; arrays contribute their elements (one level, not deep).
    fn collect_elements(&self, v: &AplRef<APLValue>, out: &mut Vec<AplRef<APLValue>>) {
        match v.as_ref() {
            APLValue::Array(a) => out.extend(a.elements()),
            other => out.push(Rc::new(other.clone())),
        }
    }

    fn reverse(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Array(a) => {
                let mut elems = a.elements();
                elems.reverse();
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![elems.len()],
                    ArrayData::Nested(elems),
                )))))
            }
            other => Ok(Rc::new(other.clone())),
        }
    }

    fn transpose(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        // Phase 4: a vector transposes to itself; a 2-D matrix is transposed.
        match right_val.as_ref() {
            APLValue::Array(a) if a.rank() == 2 => {
                let (r, c) = (a.dimensions[0], a.dimensions[1]);
                let elems = a.elements();
                let mut out = Vec::with_capacity(r * c);
                for cc in 0..c {
                    for rr in 0..r {
                        out.push(elems[rr * c + cc].clone());
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![c, r],
                    ArrayData::Nested(out),
                )))))
            }
            other => Ok(Rc::new(other.clone())),
        }
    }

    fn take(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let left_val = left_val.ok_or_else(|| AplError::runtime("↑ needs two args".into()))?;
        let n = match left_val.as_ref() {
            APLValue::Number(KapNumber::Long(v)) => *v,
            _ => {
                return Err(AplError::runtime("↑ count must be an integer".into()))
            }
        };
        match right_val.as_ref() {
            APLValue::Array(a) => {
                let elems = a.elements();
                let take = n.unsigned_abs() as usize;
                let sliced: Vec<AplRef<APLValue>> = if n >= 0 {
                    elems.iter().take(take).cloned().collect()
                } else {
                    elems.iter().rev().take(take).rev().cloned().collect()
                };
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![sliced.len()],
                    ArrayData::Nested(sliced),
                )))))
            }
            _ => Ok(right_val),
        }
    }

    fn drop(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let left_val = left_val.ok_or_else(|| AplError::runtime("↓ needs two args".into()))?;
        let n = match left_val.as_ref() {
            APLValue::Number(KapNumber::Long(v)) => *v,
            _ => {
                return Err(AplError::runtime("↓ count must be an integer".into()))
            }
        };
        match right_val.as_ref() {
            APLValue::Array(a) => {
                let elems = a.elements();
                let drop = n.unsigned_abs() as usize;
                let sliced: Vec<AplRef<APLValue>> = if n >= 0 {
                    elems.iter().skip(drop).cloned().collect()
                } else {
                    let keep = elems.len().saturating_sub(drop);
                    elems.iter().take(keep).cloned().collect()
                };
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![sliced.len()],
                    ArrayData::Nested(sliced),
                )))))
            }
            _ => Ok(right_val),
        }
    }

    fn enclose(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        // `⊂` wraps its argument in a 1-element nested array (scalar enclosure).
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![1],
            ArrayData::Nested(vec![right_val]),
        )))))
    }

    /// Convert an evaluated `APLValue` back into an `Instr` so we can re-dispatch a
    /// function application via `eval_apply` (used by adverbs). Handles scalars and
    /// nested arrays; user functions are not inlineable (error if hit).
    fn apl_to_instr(&self, v: &APLValue) -> Result<Instr, AplError> {
        match v {
            APLValue::Number(n) => Ok(Instr::Literal(LiteralValue::Number(n.clone()))),
            APLValue::Char(c) => Ok(Instr::Literal(LiteralValue::Char(*c))),
            APLValue::Str(s) => Ok(Instr::Literal(LiteralValue::Str(s.clone()))),
            APLValue::Array(a) => {
                let mut elems = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    elems.push(self.apl_to_instr(e.as_ref())?);
                }
                Ok(Instr::Array { elements: elems })
            }
            APLValue::UserFn { .. } => {
                Err(AplError::runtime("cannot use a function as an array element".into()))
            }
            APLValue::Null => Ok(Instr::Literal(LiteralValue::Str(String::new()))),
            APLValue::Deferred { .. } => {
                Err(AplError::runtime("cannot use a deferred value as an array element".into()))
            }
        }
    }

    /// Apply the function described by `fn_instr` to `left`/`right` values, by building
    /// an `Instr::Apply` and recursing into `eval_apply`. `left` is optional (monadic).
    fn apply_fn_instr(
        &self,
        fn_instr: &Instr,
        left: Option<&AplRef<APLValue>>,
        right: &AplRef<APLValue>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let left_instr = match left {
            Some(v) => Some(Box::new(self.apl_to_instr(v)?)),
            None => None,
        };
        let right_instr = Box::new(self.apl_to_instr(right)?);
        self.eval_apply(fn_instr, &left_instr, &right_instr, env)
    }

    /// Reduce `f/array`: fold left over the elements (`((a f b) f c) ...`).
    fn adverb_reduce(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        if left.is_some() {
            return Err(AplError::runtime("reduce / is monadic (use f/array)".into()));
        }
        let data = self.eval_instr(right, env)?.force(self)?;
        let elems = self.flat_elements(&data);
        match elems.split_first() {
            Some((first, rest)) => {
                let mut acc = first.clone();
                for e in rest {
                    acc = self.apply_fn_instr(fn_instr, Some(&acc), e, env)?;
                }
                Ok(acc)
            }
            None => Err(AplError::runtime("reduce /: empty array".into())),
        }
    }

    /// Scan `f\array`: like reduce but keep every intermediate accumulator
    /// (`[a, a f b, (a f b) f c, ...]`).
    fn adverb_scan(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        if left.is_some() {
            return Err(AplError::runtime("scan \\ is monadic (use f\\array)".into()));
        }
        let data = self.eval_instr(right, env)?.force(self)?;
        let elems = self.flat_elements(&data);
        match elems.split_first() {
            Some((first, rest)) => {
                let mut acc = first.clone();
                let mut out = vec![acc.clone()];
                for e in rest {
                    acc = self.apply_fn_instr(fn_instr, Some(&acc), e, env)?;
                    out.push(acc.clone());
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    vec![out.len()],
                    ArrayData::Nested(out),
                )))))
            }
            None => Err(AplError::runtime("scan \\: empty array".into())),
        }
    }

    /// Each `f¨array` (monadic) or `a f¨ b` (dyadic, element-wise with scalar extension).
    fn adverb_each(
        &self,
        fn_instr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        let right_elems = self.flat_elements(&right_val);
        let mut out = Vec::with_capacity(right_elems.len());
        match left_val {
            // Dyadic each: apply f to (left_element, right_element) for each right element.
            Some(lv) => {
                let left_elems = self.flat_elements(&lv);
                for (i, re) in right_elems.iter().enumerate() {
                    let le = left_elems.get(i).cloned().unwrap_or_else(|| lv.clone());
                    out.push(self.apply_fn_instr(fn_instr, Some(&le), re, env)?);
                }
            }
            // Monadic each: apply f to each right element.
            None => {
                for e in &right_elems {
                    out.push(self.apply_fn_instr(fn_instr, None, e, env)?);
                }
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Flatten a value into a list of element refs. Scalars become a 1-element list;
    /// arrays become their elements (one level).
    fn flat_elements(&self, v: &AplRef<APLValue>) -> Vec<AplRef<APLValue>> {
        match v.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        }
    }

    /// Apply a unary numeric function to a scalar, or element-wise to an array.
    fn scalar1(
        &self,
        right_val: AplRef<APLValue>,
        f: impl Fn(&KapNumber) -> KapNumber,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        match right_val.as_ref() {
            APLValue::Number(x) => Ok(Rc::new(APLValue::Number(f(x)))),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(x) => out.push(Rc::new(APLValue::Number(f(x)))),
                        _ => return Err(AplError::runtime(format!("{} requires numbers", sym))),
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    a.dimensions.clone(),
                    ArrayData::Nested(out),
                )))))
            }
            _ => Err(AplError::runtime(format!("{} requires a number", sym))),
        }
    }

    /// Dyadic boolean AND/OR: operands are 0/1 (truthy: non-zero). Result 0/1.
    fn bool2(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        f: impl Fn(bool, bool) -> bool,
        sym: &str,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime(format!("{} needs two args", sym)))?;
        let (x, y) = match (a.as_ref(), right_val.as_ref()) {
            (APLValue::Number(x), APLValue::Number(y)) => (x.as_boolean(), y.as_boolean()),
            _ => return Err(AplError::runtime(format!("{} requires booleans", sym))),
        };
        Ok(Rc::new(APLValue::Number(KapNumber::Long(if f(x, y) { 1 } else { 0 }))))
    }

    /// Membership `a ∊ b`: for each element of `a`, 1 if present in `b`, else 0.
    /// Returns a vector (same shape as `a`) of 0/1.
    fn membership(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let a = left_val.ok_or_else(|| AplError::runtime("∊ needs two args".into()))?;
        // Collect the set of "keys" in b (compare by formatted value for simplicity).
        let mut keys = std::collections::HashSet::new();
        match right_val.as_ref() {
            APLValue::Array(b) => {
                for e in b.elements() {
                    keys.insert(e.format_value());
                }
            }
            other => {
                keys.insert(other.format_value());
            }
        }
        let mut out = Vec::new();
        match a.as_ref() {
            APLValue::Array(aa) => {
                for e in aa.elements() {
                    let hit = if keys.contains(&e.format_value()) { 1 } else { 0 };
                    out.push(Rc::new(APLValue::Number(KapNumber::Long(hit))));
                }
            }
            other => {
                let hit = if keys.contains(&other.format_value()) { 1 } else { 0 };
                out.push(Rc::new(APLValue::Number(KapNumber::Long(hit))));
            }
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Grade up `⍋ x`: 1-based indices that would sort `x` ascending.
    fn grade_up(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        let elems: Vec<(usize, AplRef<APLValue>)> = match right_val.as_ref() {
            APLValue::Array(a) => a
                .elements()
                .into_iter()
                .enumerate()
                .collect(),
            other => vec![(0, Rc::new(other.clone()))],
        };
        // Stable sort by formatted value (orderable across all types via numeric_cmp where possible).
        let mut idx: Vec<usize> = (0..elems.len()).collect();
        idx.sort_by(|&i, &j| {
            let vi = &elems[i].1;
            let vj = &elems[j].1;
            match (vi.as_ref(), vj.as_ref()) {
                (APLValue::Number(x), APLValue::Number(y)) => x
                    .numeric_cmp(y)
                    .unwrap_or(std::cmp::Ordering::Equal),
                _ => vi
                    .format_value()
                    .cmp(&vj.format_value()),
            }
        });
        let out: Vec<AplRef<APLValue>> = idx
            .into_iter()
            .map(|i| Rc::new(APLValue::Number(KapNumber::Long((i + 1) as i64))))
            .collect();
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Encode `base ⊤ value`: represent `value` in the mixed radix given by `base`
    /// (a vector of radices, most significant first). Returns a vector.
    fn encode(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let base = left_val.ok_or_else(|| AplError::runtime("⊤ needs two args".into()))?;
        let radices: Vec<i64> = match base.as_ref() {
            APLValue::Array(a) => a
                .elements()
                .iter()
                .filter_map(|e| match e.as_ref() {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                })
                .collect(),
            APLValue::Number(n) => match n.as_long() {
                Ok(v) => vec![v],
                Err(_) => return Err(AplError::runtime("⊤ base must be integer".into())),
            },
            _ => return Err(AplError::runtime("⊤ base must be a number".into())),
        };
        let total: i64 = match right_val.as_ref() {
            APLValue::Number(n) => n.as_long().unwrap_or(0),
            _ => return Err(AplError::runtime("⊤ value must be a number".into())),
        };
        // Standard APL mixed-radix: digits d_k = floor(total / prod(radices[k+1..])) mod radices[k]
        let mut prod_after = 1i64;
        for r in radices.iter().rev() {
            prod_after *= (*r).max(1);
        }
        let mut out = Vec::with_capacity(radices.len());
        let mut rem = total;
        for r in &radices {
            let r = (*r).max(1);
            prod_after /= r;
            let digit = (rem / prod_after) % r;
            out.push(Rc::new(APLValue::Number(KapNumber::Long(digit))));
            rem %= prod_after;
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![out.len()],
            ArrayData::Nested(out),
        )))))
    }

    /// Decode `base ⊥ digits`: mixed-radix value of `digits` under radices `base`.
    fn decode(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let base = left_val.ok_or_else(|| AplError::runtime("⊥ needs two args".into()))?;
        let radices: Vec<i64> = match base.as_ref() {
            APLValue::Array(a) => a
                .elements()
                .iter()
                .filter_map(|e| match e.as_ref() {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                })
                .collect(),
            APLValue::Number(n) => match n.as_long() {
                Ok(v) => vec![v],
                Err(_) => return Err(AplError::runtime("⊥ base must be integer".into())),
            },
            _ => return Err(AplError::runtime("⊥ base must be a number".into())),
        };
        let digits: Vec<i64> = match right_val.as_ref() {
            APLValue::Array(a) => a
                .elements()
                .iter()
                .filter_map(|e| match e.as_ref() {
                    APLValue::Number(n) => n.as_long().ok(),
                    _ => None,
                })
                .collect(),
            APLValue::Number(n) => match n.as_long() {
                Ok(v) => vec![v],
                Err(_) => return Err(AplError::runtime("⊥ digits must be integer".into())),
            },
            _ => return Err(AplError::runtime("⊥ digits must be a number".into())),
        };
        let mut total = 0i64;
        for (r, d) in radices.iter().zip(digits.iter()) {
            total = total * (*r).max(1) + d;
        }
        Ok(Rc::new(APLValue::Number(KapNumber::Long(total))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> String {
        let e = Engine::new();
        let v = e.eval_string(src).expect("eval failed");
        v.format_value()
    }

    #[test]
    fn eval_addition() {
        assert_eq!(eval("1 + 2"), "3");
    }

    #[test]
    fn eval_nested_addition() {
        assert_eq!(eval("(1 + 2) * 3"), "9");
    }

    #[test]
    fn eval_parenthesised_groups() {
        // Grouping in arithmetic and with assignment/lambda/strand inside.
        assert_eq!(eval("(1 + 2) * 3"), "9");
        assert_eq!(eval("2 * (3 + 4)"), "14");
        assert_eq!(eval("(1 + 2) * (3 + 4)"), "21");
        assert_eq!(eval("((1 + 2))"), "3");
        assert_eq!(eval("(⍳3) + 10"), "[10 11 12]");
        assert_eq!(eval("1 + (2 * 3)"), "7");
        assert_eq!(eval("(x ← 5) + 1"), "6");
        assert_eq!(eval("f ← λ(x) x * 2 ⋄ (f 5) + 1"), "11");
        assert_eq!(eval("(1 2 3) + 10"), "[11 12 13]");
        assert_eq!(eval("f ← λ(x) x * 2 ⋄ f (3 + 4)"), "14");
    }

    #[test]
    fn eval_juxtaposed_groups_strand() {
        // Two juxtaposed parenthesised groups form a strand (vector), not application.
        assert_eq!(eval("(1 + 2)(3 + 4)"), "[3 7]");
        assert_eq!(eval("(1 2 3)"), "[1 2 3]");
    }

    #[test]
    fn eval_monadic_arithmetic() {
        // `+` and `-` are ambivalent: monadic `- x` = negate, `+ x` = identity.
        assert_eq!(eval("-(1 + 2)"), "¯3");
        assert_eq!(eval("+(1 + 2)"), "3");
        assert_eq!(eval("-(3 1 4)"), "[¯3 ¯1 ¯4]");
    }

    #[test]
    fn eval_paren_operator() {
        // A parenthesised operator `(OP)` is a derived function usable in dyadic position.
        assert_eq!(eval("2 (+) 3"), "5");
        assert_eq!(eval("3 (×) 4"), "12");
    }

    #[test]
    fn eval_unclosed_paren_errors() {
        // An unclosed group is a parse error, not a hang or panic.
        let e = Engine::new();
        let r = e.eval_string("(2 + 3");
        assert!(r.is_err());
    }

    #[test]
    fn eval_iota() {
        assert_eq!(eval("⍳5"), "[0 1 2 3 4]");
    }

    #[test]
    fn eval_tally() {
        assert_eq!(eval("≢ ⍳5"), "5");
    }

    #[test]
    fn eval_first() {
        assert_eq!(eval("⊃ ⍳5"), "0");
    }

    #[test]
    fn eval_array_literal() {
        assert_eq!(eval("[10; 20; 30]"), "[10 20 30]");
    }

    #[test]
    fn eval_unknown_function_errors() {
        let e = Engine::new();
        assert!(e.eval_string("1 foo 2").is_err());
    }

    // --- Phase 4 ---

    #[test]
    fn eval_sub_neg() {
        assert_eq!(eval("5 - 2"), "3");
        assert_eq!(eval("2 - 5"), "¯3");
    }

    #[test]
    fn eval_div_rational() {
        assert_eq!(eval("1 ÷ 2"), "1r2");
        assert_eq!(eval("4 ÷ 2"), "2");
    }

    #[test]
    fn eval_comparison() {
        assert_eq!(eval("3 = 3"), "1");
        assert_eq!(eval("3 ≠ 4"), "1");
        assert_eq!(eval("2 < 5"), "1");
        assert_eq!(eval("5 < 2"), "0");
        assert_eq!(eval("5 ≥ 5"), "1");
    }

    #[test]
    fn eval_catenate() {
        assert_eq!(eval("1 , 2 , 3"), "[1 2 3]");
        assert_eq!(eval("[1; 2] , [3; 4]"), "[1 2 3 4]");
    }

    #[test]
    fn eval_reverse() {
        assert_eq!(eval("⌽ ⍳5"), "[4 3 2 1 0]");
    }

    #[test]
    fn eval_take_drop() {
        assert_eq!(eval("3 ↑ ⍳10"), "[0 1 2]");
        assert_eq!(eval("3 ↓ ⍳10"), "[3 4 5 6 7 8 9]");
    }

    #[test]
    fn eval_assign_and_var() {
        assert_eq!(eval("x ← 5 ⋄ x + 1"), "6");
    }

    #[test]
    fn eval_lambda_apply() {
        assert_eq!(eval("f ← λ(x) x * 2 ⋄ f 5"), "10");
        assert_eq!(eval("g ← λ(a b) a + b ⋄ 3 g 4"), "7");
    }

    #[test]
    fn eval_strand() {
        assert_eq!(eval("1 2 3 + 10"), "[11 12 13]");
    }

    // --- Phase 6: more builtins ---

    #[test]
    fn eval_ceil_floor() {
        assert_eq!(eval("⌈ 3.2"), "4.0");
        assert_eq!(eval("⌊ 3.8"), "3.0");
        assert_eq!(eval("⌈ 5"), "5");
        assert_eq!(eval("⌈ 1.5 2.5 3.5"), "[2.0 3.0 4.0]");
    }

    #[test]
    fn eval_exp_log() {
        assert_eq!(eval("* 0"), "1.0");
        assert_eq!(eval("* 1"), "2.718281828459045");
        // natural log (monadic ⍟): ln(1) = 0 exactly (but typed Double -> "0.0")
        assert_eq!(eval("⍟ 1"), "0.0");
        // dyadic log: log base 2 of 8 = 3
        assert_eq!(eval("2 ⍟ 8"), "3.0");
    }

    #[test]
    fn eval_modulo() {
        assert_eq!(eval("7 | 3"), "1");
        assert_eq!(eval("8 | 3"), "2");
        assert_eq!(eval("10 | 3"), "1");
    }

    #[test]
    fn eval_boolean_and_or() {
        assert_eq!(eval("1 ∧ 1"), "1");
        assert_eq!(eval("1 ∧ 0"), "0");
        assert_eq!(eval("0 ∨ 1"), "1");
        assert_eq!(eval("0 ∨ 0"), "0");
        // comparisons yield booleans, combine with ∧/∨
        assert_eq!(eval("(3 < 5) ∧ (2 < 4)"), "1");
    }

    #[test]
    fn eval_not() {
        assert_eq!(eval("~ 0"), "1");
        assert_eq!(eval("~ 5"), "0");
        assert_eq!(eval("~ 1 0 3"), "[0 1 0]");
    }

    #[test]
    fn eval_membership() {
        assert_eq!(eval("2 9 4 ∊ 1 2 3 4"), "[1 0 1]");
    }

    #[test]
    fn eval_grade_up() {
        assert_eq!(eval("⍋ 3 1 4 1 5"), "[2 4 1 3 5]");
    }

    #[test]
    fn eval_encode_decode() {
        // 2 2 2 ⊤ 5  -> binary-ish mixed radix of 5 = [1 0 1]
        assert_eq!(eval("2 2 2 ⊤ 5"), "[1 0 1]");
        // inverse: 2 2 2 ⊥ 1 0 1 -> 1*4 + 0*2 + 1 = 5
        assert_eq!(eval("2 2 2 ⊥ 1 0 1"), "5");
        // 24 60 ⊤ 90 -> 1 hour 30 min
        assert_eq!(eval("24 60 ⊤ 90"), "[1 30]");
    }

    // --- Phase 7: adverbs (/ reduce, \ scan, ¨ each) ---

    #[test]
    fn eval_reduce() {
        assert_eq!(eval("+/ 1 2 3 4"), "10");
        assert_eq!(eval("×/ 1 2 3 4"), "24");
        assert_eq!(eval("⌈/ 3 9 2 7"), "9");
        assert_eq!(eval("⌊/ 3 9 2 7"), "2");
    }

    #[test]
    fn eval_scan() {
        assert_eq!(eval("+\\ 1 2 3 4"), "[1 3 6 10]");
        assert_eq!(eval("×\\ 1 2 3 4"), "[1 2 6 24]");
    }

    #[test]
    fn eval_each_monadic() {
        assert_eq!(eval("⌈¨ 1.2 2.8 3.5"), "[2.0 3.0 4.0]");
        assert_eq!(eval("~¨ 1 0 3"), "[0 1 0]");
    }

    #[test]
    fn eval_each_dyadic() {
        // element-wise: 2 ×¨ 3 4 5  -> [6 8 10]
        assert_eq!(eval("2 ×¨ 3 4 5"), "[6 8 10]");
        // scalar-extended left, vector right
        assert_eq!(eval("1 2 3 +¨ 4 5 6"), "[5 7 9]");
        // vector × vector each
        assert_eq!(eval("1 2 3 ×¨ 4 5 6"), "[4 10 18]");
    }
}
