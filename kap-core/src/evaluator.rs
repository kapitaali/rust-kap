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
use std::rc::Rc;

/// Adjust a (possibly negative) index into a valid 0-based position within `axis_size`,
/// matching Kap's `Dimensions.checkAndAdjustSelectedIndex` (dimension.kt):
/// non-negative `i` must satisfy `0 <= i < axis_size`; negative `i` counts from the end
/// (`axis_size + i`), valid when `-axis_size <= i`. Anything else is out of bounds.
fn check_and_adjust_selected_index(index: i64, axis_size: usize) -> Result<usize, AplError> {
    let n = axis_size as i64;
    if index >= 0 {
        if index >= n {
            return Err(AplError::runtime(format!(
                "index {} is outside valid range (axis size {})",
                index, n
            )));
        }
        Ok(index as usize)
    } else {
        if index < -n {
            return Err(AplError::runtime(format!(
                "index {} is outside valid range (axis size {})",
                index, n
            )));
        }
        Ok((n + index) as usize)
    }
}

/// Compute the stride (elements per axis unit) for each axis of a shape, in
/// row-major order. `strides[0]` = product of all dims except the first, etc.;
/// `strides[rank-1] == 1`. Used by `transpose`.
fn strides(dims: &[usize]) -> Vec<usize> {
    let r = dims.len();
    let mut s = vec![1usize; r];
    if r == 0 {
        return s;
    }
    for k in (0..r - 1).rev() {
        s[k] = s[k + 1] * dims[k + 1];
    }
    s
}




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

    /// Names of all symbols in this scope (and parents) that currently hold a
    /// user-defined or native function value. Used by the parser to distinguish a
    /// function symbol (which applies) from a value symbol (which strands).
    pub fn function_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut cur: Option<&Environment> = Some(self);
        while let Some(env) = cur {
            for ((name, _ns), val) in env.symbols.borrow().iter() {
                if matches!(val.as_ref(), APLValue::UserFn { .. }) && !names.contains(name) {
                    names.push(name.clone());
                }
            }
            cur = env.parent.as_deref();
        }
        names
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
        // Parse and evaluate one statement at a time, refreshing the set of known function
        // names between statements. This lets a function defined in one statement
        // (`∇ foo ...`) be visible to later statements (`foo 10`), so the parser can tell a
        // value symbol (`a c` strands) from a function symbol (`foo 10` applies).
        let toks = tokenise(src);
        let mut pos = 0;
        let mut last: AplRef<APLValue> = Rc::new(APLValue::Null);
        loop {
            // Fresh function-name set each statement, so a function defined earlier in
            // this same input (`∇ foo ...`) is visible to later statements (`foo 10`).
            // The parser also grows this set as it parses `∇`/`⇐` defs, so a function
            // body that references its own name parses as an application (recursion).
            let fn_names: Vec<String> = env.function_names();
            let mut p = parser::Parser {
                toks: &toks,
                pos,
                known_functions: fn_names,
            };
            match p.parse_statements()? {
                Some(instr) => {
                    pos = p.pos;
                    last = self.eval_instr(&instr, env)?;
                }
                None => break,
            }
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
                // The last param is the right argument (⍵); any preceding params are
                // left arguments (⍺). So `λ(a)` is monadic (split 0) and `λ(a b)` is
                // dyadic (split 1), matching Kap dfn conventions.
                split: params.len().saturating_sub(1),
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
            Instr::Block { body } => self.eval_block(body, env),
            Instr::If {
                cond,
                then_block,
                else_block,
            } => {
                let c = self.eval_instr(cond, env)?;
                if self.truthy(&c) {
                    self.eval_instr(then_block, env)
                } else if let Some(alt) = else_block {
                    self.eval_instr(alt, env)
                } else {
                    Ok(Rc::new(APLValue::Null))
                }
            }
            Instr::While { cond, body } => {
                let mut last = Rc::new(APLValue::Null);
                loop {
                    let c = self.eval_instr(cond, env)?;
                    if !self.truthy(&c) {
                        break;
                    }
                    last = self.eval_instr(body, env)?;
                }
                Ok(last)
            }
            Instr::When { clauses } => {
                for (c, b) in clauses {
                    let cv = self.eval_instr(c, env)?;
                    if self.truthy(&cv) {
                        return self.eval_instr(b, env);
                    }
                }
                Ok(Rc::new(APLValue::Null))
            }
            Instr::Train { .. } => {
                // A standalone train (no args) is an error; trains apply via eval_apply.
                Err(AplError::runtime(
                    "train used without arguments (apply it: (f g) x)".into(),
                ))
            }
            Instr::UserFnDef {
                name,
                namespace,
                left_params,
                right_params,
                body,
            } => {
                // Combine left+right params; `split` separates the dyadic left bind.
                let mut params = left_params.clone();
                params.extend(right_params.iter().cloned());
                let split = left_params.len();
                let v = Rc::new(APLValue::UserFn {
                    params,
                    split,
                    body: Rc::new(*body.clone()),
                    env: env.clone(),
                });
                env.define(name, namespace, v.clone());
                Ok(v)
            }
            Instr::FnAssign {
                name,
                namespace,
                value,
            } => {
                // `name ⇐ <fn-expr>`: compile RHS to a UserFn. If it is already a
                // lambda, store directly with split=0; otherwise wrap as a dyadic
                // delegation `⍺ <rhs> ⍵` (split=1), so the new name is callable as
                // `x name y` (and monadically only if the RHS supports it).
                let v = match value.as_ref() {
                    Instr::Lambda { params, body } => Rc::new(APLValue::UserFn {
                        params: params.clone(),
                        split: params.len().saturating_sub(1),
                        body: Rc::new(*body.clone()),
                        env: env.clone(),
                    }),
                    // A bare block `{ … }` is a function with no named parameters; its
                    // body is the block itself (it may reference `⍵`/`⍺`).
                    Instr::Block { body } => Rc::new(APLValue::UserFn {
                        params: vec![],
                        split: 0,
                        body: Rc::new(Instr::Block { body: body.clone() }),
                        env: env.clone(),
                    }),
                    _ => {
                        let deleg = Instr::Apply {
                            fn_expr: Box::new(*value.clone()),
                            left: Some(Box::new(Instr::Symbol {
                                name: "⍺".into(),
                                namespace: None,
                            })),
                            right: Box::new(Instr::Symbol {
                                name: "⍵".into(),
                                namespace: None,
                            }),
                        };
                        Rc::new(APLValue::UserFn {
                            params: vec![],
                            split: 1,
                            body: Rc::new(deleg),
                            env: env.clone(),
                        })
                    }
                };
                env.define(name, namespace, v.clone());
                Ok(v)
            }
            Instr::Value(v) => Ok(v.clone()),
            Instr::Index { array, selector } => {
                let arr = self.eval_instr(array, env)?;
                let sel = self.eval_instr(selector, env)?;
                self.pick(&arr, &sel)
            }
            Instr::Guard { cond, truthy, falsy } => {
                let c = self.eval_instr(cond, env)?.force(self)?;
                if self.truthy(&c) {
                    self.eval_instr(truthy, env)
                } else {
                    self.eval_instr(falsy, env)
                }
            }
        }
    }

    /// Evaluate a block: statements in order; the value is the last statement's value.
    fn eval_block(
        &self,
        body: &[Instr],
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let mut result = Rc::new(APLValue::Null);
        for stmt in body {
            result = self.eval_instr(stmt, env)?;
        }
        Ok(result)
    }

    /// Kap truthiness: a number is truthy iff non-zero; an array is truthy iff non-empty;
    /// null/empty string is falsy; a non-empty string is truthy.
    fn truthy(&self, v: &APLValue) -> bool {
        match v {
            APLValue::Number(n) => n.as_boolean(),
            APLValue::Array(a) => a.element_count() > 0,
            APLValue::Str(s) => !s.is_empty(),
            APLValue::Char(_) => true,
            APLValue::Null => false,
            APLValue::UserFn { .. } => true,
            APLValue::Deferred { .. } => false,
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
            Instr::Lambda { params, body } => Some((
                params.clone(),
                params.len().saturating_sub(1),
                Rc::new((**body).clone()),
            )),
            Instr::Symbol { name, namespace } => match env.lookup(name, namespace) {
                Some(v) if matches!(v.as_ref(), APLValue::UserFn { .. }) => {
                    if let APLValue::UserFn { params, split, body, env: _fenv } = v.as_ref() {
                        Some((params.clone(), *split, Rc::new((**body).clone())))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((params, split, body)) = lambda {
            return self.apply_user_fn(&params, split, &body, left, right, env, fn_name.as_deref());
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
        // --- Trains: (f g h) as a derived function ---
        // Monadic: right-to-left composition  (f g h) y = f (g (h y)).
        // Dyadic 2-train (atop):       x (A B) y = A x (B y).
        // Dyadic 3-train (fork):       x (A B C) y = (x A y) B (x C y).
        if let Instr::Train { funcs, reverse, compose } = fn_expr {
            return self.apply_train(funcs, *reverse, *compose, left, right, env);
        }
        let name = match fn_name {
            Some(ref n) => n.clone(),
            None => {
                return Err(AplError::runtime("only symbol/lambda functions supported yet".into()));
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
            "÷" | "/" => match left_val {
                None => self.scalar1(right_val, |x| x.recip(), "÷"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.div(b), "÷"),
            },
            "=" => self.cmp2(left_val, right_val, |o| o == Ordering::Equal, "="),
            "≠" => self.cmp2(left_val, right_val, |o| o != Ordering::Equal, "≠"),
            "<" => self.cmp2(left_val, right_val, |o| o == Ordering::Less, "<"),
            ">" => self.cmp2(left_val, right_val, |o| o == Ordering::Greater, ">"),
            "≤" => self.cmp2(left_val, right_val, |o| o != Ordering::Greater, "≤"),
            "≥" => self.cmp2(left_val, right_val, |o| o != Ordering::Less, "≥"),
            "⍳" | "iota" => self.iota(right_val),
            "⍴" | "rho" => match left_val {
                None => self.shape(right_val),
                Some(l) => self.reshape(l, right_val),
            },
            "≢" | "tally" => self.tally(right_val),
            "⊃" | "first" => self.first(right_val),
            "," => self.catenate(left_val, right_val),
            "⌽" | "rotateright" => self.reverse_horizontal(left_val, right_val),
            "⊖" | "rotateleft" => self.reverse_vertical(left_val, right_val),
            "⍉" => self.transpose(left_val, right_val),
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
            "*" => match left_val {
                None => self.scalar1(right_val, |x| x.exp(), "*"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.pow(b), "*"),
            },
            "×" => match left_val {
                None => self.scalar1(right_val, |x| x.signum(), "×"),
                Some(_) => self.num2(left_val, right_val, |a, b| a.mul(b), "×"),
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

    /// Apply a *train* `(f g h ...)` as a derived function.
    /// - Monadic: right-to-left composition `(f g h) y` = `f (g (h y))`.
    /// - Dyadic 2-train (atop): `x (A B) y` = `A x (B y)`.
    /// Apply a *train* (derived function from a sequence of functions / a bound value).
    ///
    /// Reference semantics (ComposeTest.kt / operator.kt):
    /// - Compose `f ∘ g` (compose=true):
    ///     monadic `(f∘g) y` = `f(y, g(y))`;  dyadic `x (f∘g) y` = `f(x, g(y))`  (g monadic).
    /// - Reverse-compose `f ⍛ g` (reverse=true):
    ///     monadic `(f⍛g) y` = `g(f(y), y)`;  dyadic `x (f⍛g) y` = `g(f(x), y)`  (f monadic, g dyadic).
    /// - Bare 2-train (atop, compose=false, reverse=false):
    ///     monadic `(f g) y` = `f(g(y))`;  dyadic `x (f g) y` = `f(x g y)`.
    /// - Fork `A « B » C` (3-train):
    ///     monadic `(A y) B (C y)`;  dyadic `(x A y) B (x C y)`.
    /// - Left-bind `[value, fn]` (2-train, first member a value): `(c f) y` = `f(c, y)`
    ///   (the outer left arg is ignored).
    fn apply_train(
        &self,
        funcs: &[Instr],
        reverse: bool,
        compose: bool,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        match left {
            None => {
                // --- Monadic ---
                if reverse {
                    // (f ⍛ g) y = g(f(y), y)
                    let fy = self.eval_apply(&funcs[0], &None, right, env)?;
                    return self.eval_apply(
                        &funcs[1],
                        &Some(Box::new(Instr::Value(fy))),
                        right,
                        env,
                    );
                }
                if compose {
                    // (f ∘ g) y = f(y, g(y))
                    let gy = self.eval_apply(&funcs[1], &None, right, env)?;
                    return self.eval_apply(
                        &funcs[0],
                        &Some(right.clone()),
                        &Box::new(Instr::Value(gy)),
                        env,
                    );
                }
                // Left-bind: [value, fn]
                if funcs.len() == 2 && Self::is_value(&funcs[0]) {
                    return self.eval_apply(
                        &funcs[1],
                        &Some(Box::new(funcs[0].clone())),
                        right,
                        env,
                    );
                }
                match funcs.len() {
                    2 => {
                        // Atop: (f g) y = f(g(y))
                        let gy = self.eval_apply(&funcs[1], &None, right, env)?;
                        self.eval_apply(&funcs[0], &None, &Box::new(Instr::Value(gy)), env)
                    }
                    3 => {
                        // Fork: (A y) B (C y)
                        let ay = self.eval_apply(&funcs[0], &None, right, env)?;
                        let cy = self.eval_apply(&funcs[2], &None, right, env)?;
                        self.eval_apply(
                            &funcs[1],
                            &Some(Box::new(Instr::Value(ay))),
                            &Box::new(Instr::Value(cy)),
                            env,
                        )
                    }
                    n => Err(AplError::runtime(format!(
                        "monadic trains of length {} are not supported (use 2 or 3 functions)",
                        n
                    ))),
                }
            }
            Some(l) => {
                let left_val = self.eval_instr(l, env)?;
                if reverse {
                    // x (f ⍛ g) y = g(f(x), y)
                    let fx = self.eval_apply(&funcs[0], &None, &Box::new(Instr::Value(left_val.clone())), env)?;
                    return self.eval_apply(
                        &funcs[1],
                        &Some(Box::new(Instr::Value(fx))),
                        right,
                        env,
                    );
                }
                if compose {
                    // x (f ∘ g) y = f(x, g(y))
                    let gy = self.eval_apply(&funcs[1], &None, right, env)?;
                    return self.eval_apply(
                        &funcs[0],
                        &Some(Box::new(Instr::Value(left_val.clone()))),
                        &Box::new(Instr::Value(gy)),
                        env,
                    );
                }
                match funcs.len() {
                    2 => {
                        let (a, b) = (&funcs[0], &funcs[1]);
                        if Self::is_value(a) {
                            // x (c f) y : left-bind ignores the outer left, uses c.
                            return self.eval_apply(
                                b,
                                &Some(Box::new(a.clone())),
                                right,
                                env,
                            );
                        }
                        // Atop: f(x g y)  (g is dyadic)
                        let xgy = self.eval_apply(
                            b,
                            &Some(Box::new(Instr::Value(left_val.clone()))),
                            right,
                            env,
                        )?;
                        self.eval_apply(a, &None, &Box::new(Instr::Value(xgy)), env)
                    }
                    3 => {
                        let (a, b, c) = (&funcs[0], &funcs[1], &funcs[2]);
                        // (x A y) B (x C y)
                        let ay = self.eval_apply(a, &Some(Box::new(Instr::Value(left_val.clone()))), right, env)?;
                        let cy = self.eval_apply(c, &Some(Box::new(Instr::Value(left_val))), right, env)?;
                        self.eval_apply(
                            b,
                            &Some(Box::new(Instr::Value(ay))),
                            &Box::new(Instr::Value(cy)),
                            env,
                        )
                    }
                    n => Err(AplError::runtime(format!(
                        "dyadic trains of length {} are not yet supported (use 2 or 3 functions)",
                        n
                    ))),
                }
            }
        }
    }

    /// Whether an instr is a *value* (suitable for left-bind first member): a literal
    /// or array, but not a function/operator.
    fn is_value(e: &Instr) -> bool {
        matches!(e, Instr::Literal(_) | Instr::Array { .. } | Instr::Empty)
    }

    /// Apply a user-defined lambda. `split` = number of leading params that are bound to
    /// the *left* (dyadic) argument; the remainder are bound to the right argument.
    /// For a monadic function `split == 0`. Builds a child scope from the closure env
    /// and binds params, also exposing `⍺` (left, if any) and `⍵` (right) as defaults.
    fn apply_user_fn(
        &self,
        params: &[String],
        split: usize,
        body: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        closure_env: &AplRef<Environment>,
        self_name: Option<&str>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let child = Rc::new(Environment {
            symbols: Default::default(),
            parent: Some(closure_env.clone()),
        });
        // Evaluate args in the *calling* env (Kap passes by value/sharing).
        let right_val = self.eval_instr(right, closure_env)?.force(self)?;
        // Bind named params: first `split` to the left arg, the rest to the right arg.
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, closure_env)?.force(self)?),
            None => None,
        };
        // Argument-count validation (Kap raises if arity is wrong).
        let needed_left = split;
        let needed_right = params.len().saturating_sub(split);
        let have_left = match &left_val {
            Some(v) => self.element_count(v),
            None => 0,
        };
        let have_right = self.element_count(&right_val);
        if have_left != needed_left || have_right != needed_right {
            return Err(AplError::runtime(format!(
                "function called with wrong number of arguments: expected {} left and {} right, got {} left and {} right",
                needed_left, needed_right, have_left, have_right
            )));
        }
        if split > 0 {
            if let Some(lv) = &left_val {
                for p in &params[..split.min(params.len())] {
                    child.define(p, &None, lv.clone());
                }
            }
        }
        for p in &params[split.min(params.len())..] {
            child.define(p, &None, right_val.clone());
        }
        // Default `⍵`/`⍺` names (Kap's omega/alpha). `⍵` = right arg; `⍺` = left (if present).
        child.define("⍵", &None, right_val);
        if let Some(lv) = &left_val {
            child.define("⍺", &None, lv.clone());
        }
        // Self-binding: so a function can recurse by name (e.g. `fib` calling `fib`).
        if let Some(name) = self_name {
            let self_fn = APLValue::UserFn {
                params: params.to_vec(),
                split,
                body: Rc::new(body.clone()),
                env: closure_env.clone(),
            };
            child.define(name, &None, Rc::new(self_fn));
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
                if xa.element_count() != ya.element_count() {
                    return Err(AplError::runtime(format!(
                        "{}: arrays of different length ({} vs {})",
                        sym,
                        xa.element_count(),
                        ya.element_count()
                    )));
                }
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

    fn reshape(&self, left_val: AplRef<APLValue>, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        // Dyadic `⍴`: `(dims) ⍴ data` builds an array of shape `dims`, filled by
        // cycling through the flat elements of `data` (Kap/APL reshape semantics).
        let dims_val = left_val.force(self)?;
        let mut dims: Vec<usize> = Vec::new();
        match dims_val.as_ref() {
            APLValue::Array(a) => {
                for e in a.elements() {
                    if let APLValue::Number(KapNumber::Long(v)) = e.as_ref() {
                        if *v < 0 {
                            return Err(AplError::runtime("reshape dimensions must be non-negative".into()));
                        }
                        dims.push(*v as usize);
                    } else {
                        return Err(AplError::runtime("reshape dimensions must be integers".into()));
                    }
                }
            }
            APLValue::Number(KapNumber::Long(v)) => {
                if *v < 0 {
                    return Err(AplError::runtime("reshape dimensions must be non-negative".into()));
                }
                dims.push(*v as usize);
            }
            _ => return Err(AplError::runtime("reshape dimensions must be an array or integer".into())),
        }
        if dims.is_empty() {
            return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(vec![], ArrayData::Nested(vec![]))))));
        }
        let total: usize = dims.iter().product();
        // Guard against absurd sizes (the OOM case was an ~1e12 product). Empty arrays
        // (total == 0) are VALID in Kap and used widely, so do not reject them here.
        if total > 100_000_000 {
            return Err(AplError::runtime("reshape result too large".into()));
        }
        let data = right_val.force(self)?;
        let mut src: Vec<AplRef<APLValue>> = match data.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };
        if src.is_empty() {
            // Filling with a prototype: use a scalar 0 (matches Kap's empty-fill behaviour for
            // numeric reshape sources).
            src = vec![Rc::new(APLValue::Number(KapNumber::Long(0)))];
        }
        let total: usize = dims.iter().product();
        let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(total);
        for i in 0..total {
            out.push(Rc::new(src[i % src.len()].as_ref().clone()));
        }
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(out))))))
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

    fn reverse_horizontal(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // `⌽` reverses/rotates along the *last* axis (default for `⌽`).
        self.reverse_axis(left_val, right_val, None, true)
    }

    fn reverse_vertical(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // `⊖` reverses/rotates along the *first* axis (default for `⊖`).
        self.reverse_axis(left_val, right_val, Some(0), true)
    }

    /// Reverse or rotate one axis of an n-D array (Kap `⌽`/`⊖`).
    /// - `axis`: fixed axis (Some(k)) or the last axis (None ⇒ rank-1).
    /// - monadic (left=None) ⇒ reverse that axis;
    /// - dyadic (left=scalar n) ⇒ rotate every cell along the axis by n;
    /// - dyadic (left=vector v) ⇒ rotate cell i by v[i] (length must equal #cells).
    fn reverse_axis(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
        axis: Option<usize>,
        _reverse_when_none: bool,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right = right_val.force(self)?;
        match right.as_ref() {
            APLValue::Number(_) | APLValue::Null => Ok(Rc::new(right.as_ref().clone())),
            APLValue::Array(a) => {
                let dims = a.dimensions.clone();
                let rank = dims.len();
                if rank == 0 {
                    return Ok(Rc::new(right.as_ref().clone()));
                }
                let axis = axis.unwrap_or(rank - 1).min(rank - 1);
                let n = dims[axis];
                let stride: usize = dims[axis + 1..].iter().product();
                let cells: usize = dims[..axis].iter().copied().product::<usize>().max(1);

                // Build a per-cell shift (rotate amount). None left ⇒ reverse.
                let mut do_reverse = false;
                let mut shifts: Vec<i64> = Vec::with_capacity(cells);
                match left_val {
                    None => {
                        do_reverse = true;
                        shifts = vec![0; cells];
                    }
                    Some(l) => {
                        let lv = l.force(self)?;
                        match lv.as_ref() {
                            APLValue::Number(KapNumber::Long(x)) => {
                                shifts = vec![*x; cells];
                            }
                            APLValue::Array(va) => {
                                for e in va.elements() {
                                    match e.as_ref() {
                                        APLValue::Number(KapNumber::Long(x)) => shifts.push(*x),
                                        _ => {
                                            return Err(AplError::runtime(
                                                "⌽/⊖ shift must be integers".into(),
                                            ))
                                        }
                                    }
                                }
                                if !shifts.is_empty() && shifts.len() != cells {
                                    return Err(AplError::runtime(
                                        "⌽/⊖ shift vector length must match number of cells".into(),
                                    ));
                                }
                            }
                            _ => {
                                return Err(AplError::runtime(
                                    "⌽/⊖ shift must be an integer or vector".into(),
                                ))
                            }
                        }
                    }
                }

                let elems = a.elements();
                let mut out = elems.clone();
                let n64 = n as i64;
                for c in 0..cells {
                    let base = c * n * stride;
                    let shift = if do_reverse { 0 } else { shifts[c % shifts.len().max(1)] };
                    for k in 0..n {
                        let src = if do_reverse {
                            n - 1 - k
                        } else {
                            (((k as i64) + shift).rem_euclid(n64)) as usize
                        };
                        for s in 0..stride {
                            out[base + k * stride + s] =
                                elems[base + src * stride + s].clone();
                        }
                    }
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    dims,
                    ArrayData::Nested(out),
                )))))
            }
            other => Err(AplError::runtime(
                "⌽/⊖ not implemented for this value type".into(),
            )),
        }
    }

    fn transpose(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Monadic `⍉ A` reverses the axis order. Dyadic `axes ⍉ A` permutes axes
        // per `axes` (must be a permutation of 0..rank-1, else InvalidDimensions).
        let right = right_val.force(self)?;
        match right.as_ref() {
            APLValue::Number(_) | APLValue::Null => Ok(Rc::new(right.as_ref().clone())),
            APLValue::Array(a) => {
                let dims = a.dimensions.clone();
                let rank = dims.len();
                if rank == 0 {
                    return Ok(Rc::new(right.as_ref().clone()));
                }
                // Resolve the axis permutation.
                let perm: Vec<usize> = match left_val {
                    None => (0..rank).rev().collect(),
                    Some(l) => {
                        let lv = l.force(self)?;
                        let axes: Vec<i64> = match lv.as_ref() {
                            APLValue::Number(KapNumber::Long(x)) => vec![*x],
                            APLValue::Array(va) => {
                                let mut v = Vec::with_capacity(va.element_count());
                                for e in va.elements() {
                                    match e.as_ref() {
                                        APLValue::Number(KapNumber::Long(x)) => v.push(*x),
                                        _ => {
                                            return Err(AplError::runtime(
                                                "⍉ axes must be integers".into(),
                                            ))
                                        }
                                    }
                                }
                                v
                            }
                            _ => {
                                return Err(AplError::runtime(
                                    "⍉ axes must be an integer or vector".into(),
                                ))
                            }
                        };
                        if axes.len() != rank {
                            return Err(AplError::runtime(format!(
                                "⍉ axis count {} does not match array rank {}",
                                axes.len(),
                                rank
                            )));
                        }
                        let mut seen = vec![false; rank];
                        let mut perm = Vec::with_capacity(rank);
                        for &x in &axes {
                            if x < 0 || x as usize >= rank || seen[x as usize] {
                                return Err(AplError::runtime(
                                    "⍉ axes must be a permutation of 0..rank-1".into(),
                                ));
                            }
                            seen[x as usize] = true;
                            perm.push(x as usize);
                        }
                        perm
                    }
                };

                let elems = a.elements();
                // Kotlin `TransposedAPLValue`: result axis k has dimension
                // `d[perm⁻¹[k]]` (it stores inverseTransposedAxis = perm⁻¹).
                let mut inv = vec![0usize; rank];
                for (k, &p) in perm.iter().enumerate() {
                    inv[p] = k;
                }
                let new_dims: Vec<usize> = (0..rank).map(|k| dims[inv[k]]).collect();
                let old_stride = strides(&dims);
                let new_stride = strides(&new_dims);
                let total: usize = new_dims.iter().product();
                if total > 100_000_000 {
                    return Err(AplError::runtime("transpose result too large".into()));
                }
                let mut out: Vec<AplRef<APLValue>> = vec![elems[0].clone(); total];
                for pos in 0..total {
                    let mut rem = pos;
                    let mut new_coords = vec![0usize; rank];
                    for k in 0..rank {
                        new_coords[k] = rem / new_stride[k];
                        rem %= new_stride[k];
                    }
                    let mut old_coords = vec![0usize; rank];
                    for k in 0..rank {
                        // Kotlin `TransposedAPLValue.translateIndex`:
                        // `s[index] = c[transposeAxis[index]]`, i.e. source coord
                        // `k` equals result coord `perm[k]`.
                        old_coords[k] = new_coords[perm[k]];
                    }
                    let mut oflat = 0usize;
                    for k in 0..rank {
                        oflat += old_coords[k] * old_stride[k];
                    }
                    out[pos] = elems[oflat].clone();
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    new_dims,
                    ArrayData::Nested(out),
                )))))
            }
            other => Err(AplError::runtime("⍉ not implemented for this value type".into())),
        }
    }

    fn take(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Dyadic `↑`: `counts ↑ array`. Counts is a scalar or vector; each axis
        // count may be negative (take from the end). If `counts` is shorter than
        // the array rank the remaining axes are taken in full; if longer, it is an
        // error. A scalar right argument is reshaped to `|counts|` (padded with 0).
        // Monadic `↑` (no left) takes 1 along the leading axis.
        // Reference: TakeTest.kt.
        let counts = match left_val {
            None => vec![1i64],
            Some(l) => self.count_vector(l)?,
        };
        self.take_or_drop(true, &counts, right_val)
    }

    fn drop(
        &self,
        left_val: Option<AplRef<APLValue>>,
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Dyadic `↓`: `counts ↓ array`. Mirror of `take` but removes elements.
        // Monadic `↓` drops 1 along the leading axis. Reference: TakeTest.kt.
        let counts = match left_val {
            None => vec![1i64],
            Some(l) => self.count_vector(l)?,
        };
        self.take_or_drop(false, &counts, right_val)
    }

    /// Parse the left argument of `↑`/`↓` into a vector of (signed) axis counts.
    fn count_vector(&self, v: AplRef<APLValue>) -> Result<Vec<i64>, AplError> {
        match v.as_ref() {
            APLValue::Number(KapNumber::Long(n)) => Ok(vec![*n]),
            APLValue::Array(a) => {
                let mut out = Vec::with_capacity(a.element_count());
                for e in a.elements() {
                    match e.as_ref() {
                        APLValue::Number(KapNumber::Long(n)) => out.push(*n),
                        _ => return Err(AplError::runtime("↑/↓ counts must be integers".into())),
                    }
                }
                Ok(out)
            }
            _ => Err(AplError::runtime("↑/↓ counts must be integers".into())),
        }
    }

    /// Core multi-dimensional take/drop. `take`=true means keep-from-edge
    /// (with padding for oversized positive counts); `take`=false means drop.
    fn take_or_drop(
        &self,
        take: bool,
        counts: &[i64],
        right_val: AplRef<APLValue>,
    ) -> Result<AplRef<APLValue>, AplError> {
        let right = right_val.force(self)?;
        match right.as_ref() {
            APLValue::Null => {
                // `↑⍬` -> 0 (scalar fill); `↓⍬` -> ⍬ (null).
                if take {
                    Ok(Rc::new(APLValue::Number(KapNumber::Long(0))))
                } else {
                    Ok(Rc::new(APLValue::Null))
                }
            }
            APLValue::Number(_) => {
                // Scalar right argument.
                // Take: build an array of shape `|counts|` filled with the scalar,
                // padding with 0; an all-zero count yields the empty value (null).
                // Drop: dropping from a scalar removes it entirely — any non-zero
                // count empties it (null); a zero count keeps `(right)`.
                let all_zero = counts.iter().all(|c| *c == 0);
                if !take && !all_zero {
                    return Ok(Rc::new(APLValue::Null));
                }
                if !take && all_zero {
                    // Drop 0 of a scalar: keep it as a 1-element vector `(right)`.
                    return Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                        vec![1],
                        ArrayData::Nested(vec![right.clone()]),
                    )))));
                }
                let dims: Vec<usize> = counts.iter().map(|c| c.unsigned_abs() as usize).collect();
                let total: usize = dims.iter().product();
                if take && total == 0 {
                    return Ok(Rc::new(APLValue::Null));
                }
                if total > 100_000_000 {
                    return Err(AplError::runtime("take/drop result too large".into()));
                }
                let content = right.clone();
                let fill = Rc::new(APLValue::Number(KapNumber::Long(0)));
                let mut out = Vec::with_capacity(total);
                for i in 0..total {
                    out.push(if i == 0 { content.clone() } else { fill.clone() });
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(out))))))
            }
            APLValue::Array(a) => {
                let mut dims = a.dimensions.clone();
                let rank = dims.len();
                if counts.len() > rank && rank > 0 {
                    return Err(AplError::runtime(
                        "↑/↓ count has more elements than the array rank".into(),
                    ));
                }
                // Compute the final shape up front and reject runaway results
                // (e.g. `1e6 1e6 ↑ small`) *before* slicing, so we never attempt to
                // allocate an OOM-sized vector — `slice_axis` requests a `Vec`
                // capacity of `outer * target * stride`, and for a padded multi-axis
                // take that can be ~1e12 elements, which aborts the process.
                let mut final_dims = dims.clone();
                for axis in 0..rank {
                    let spec = counts.get(axis).copied();
                    let n = dims[axis];
                    final_dims[axis] = match spec {
                        None => n,
                        Some(c) if take => c.unsigned_abs() as usize,
                        Some(c) => n - n.min(c.unsigned_abs() as usize),
                    };
                }
                let total: usize = final_dims.iter().product();
                if total > 100_000_000 {
                    return Err(AplError::runtime("take/drop result too large".into()));
                }
                let mut flat = a.elements();
                for axis in 0..rank {
                    let spec = counts.get(axis).copied();
                    let (new_flat, new_dims) = self.slice_axis(&flat, &dims, axis, take, spec)?;
                    flat = new_flat;
                    dims = new_dims;
                }
                let total: usize = dims.iter().product();
                if total > 100_000_000 {
                    return Err(AplError::runtime("take/drop result too large".into()));
                }
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(dims, ArrayData::Nested(flat))))))
            }
            other => Err(AplError::runtime(
                "↑/↓ not implemented for this value type".into(),
            )),
        }
    }

    /// Slice one axis of a flat (row-major) element list.
    /// `take`=true: keep `|spec|` blocks starting from the edge (0 for positive,
    /// from the end for negative), padding with the 0-fill for oversized counts.
    /// `take`=false: drop `|spec|` blocks from the edge, no padding.
    /// `spec`=None means "take all" (axis left unspecified by the count vector).
    fn slice_axis(
        &self,
        flat: &[AplRef<APLValue>],
        dims: &[usize],
        axis: usize,
        take: bool,
        spec: Option<i64>,
    ) -> Result<(Vec<AplRef<APLValue>>, Vec<usize>), AplError> {
        let n = dims[axis];
        let stride: usize = dims[axis + 1..].iter().product();
        let outer: usize = dims[..axis].iter().product();
        let fill = Rc::new(APLValue::Number(KapNumber::Long(0)));

        // Determine [start, keep, pad_before, pad_after] in units of `stride` blocks.
        let (start, keep, pad_before, pad_after, target): (usize, usize, usize, usize, usize) =
            match spec {
                None => (0, n, 0, 0, n),
                Some(c) if take => {
                    let abs = c.unsigned_abs() as usize;
                    if c >= 0 {
                        (0, n.min(abs), 0, abs.saturating_sub(n), abs)
                    } else {
                        let keep = n.min(abs);
                        (n - keep, keep, abs.saturating_sub(n), 0, abs)
                    }
                }
                Some(c) => {
                    let abs = c.unsigned_abs() as usize;
                    let d = n.min(abs);
                    (if c >= 0 { d } else { 0 }, n - d, 0, 0, n - d)
                }
            };

        let mut out = Vec::with_capacity(outer * target * stride);
        for g in 0..outer {
            let base = g * n * stride;
            for _ in 0..pad_before {
                for _ in 0..stride {
                    out.push(fill.clone());
                }
            }
            for k in 0..keep {
                let b = start + k;
                let off = base + b * stride;
                for i in 0..stride {
                    out.push(flat[off + i].clone());
                }
            }
            for _ in 0..pad_after {
                for _ in 0..stride {
                    out.push(fill.clone());
                }
            }
        }
        let mut new_dims = dims.to_vec();
        new_dims[axis] = target;
        Ok((out, new_dims))
    }

    fn enclose(&self, right_val: AplRef<APLValue>) -> Result<AplRef<APLValue>, AplError> {
        // `⊂` wraps its argument in a 1-element nested array (scalar enclosure).
        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
            vec![1],
            ArrayData::Nested(vec![right_val]),
        )))))
    }

    /// Convert an `APLValue` into a 0-based integer index. Accepts Long and Double
    /// (truncated toward zero, matching Kap's numeric->int coercion).
    fn index_to_i64(&self, v: &APLValue) -> Result<i64, AplError> {
        match v {
            APLValue::Number(KapNumber::Long(i)) => Ok(*i),
            APLValue::Number(KapNumber::Double(f)) => Ok(*f as i64),
            _ => Err(AplError::runtime("array index must be an integer".into())),
        }
    }

    /// Kap bracket indexing `target[selector]`, matching the Kotlin reference
    /// (`lookup.kt`): `PickResultValue` for a single `;`-section, and
    /// `AccessFromIndexAPLFunction` (per-axis selection) for `;`-separated sections.
    ///
    /// Single section (no `;`): result shape = the index's shape; each scalar cell of the
    /// index selects from `target` (a scalar index discloses to a scalar). Negative
    /// indices count from the end; out-of-range errors.
    ///
    /// Multiple sections (`a[r;c]`): per target axis
    ///   - `⍬`/empty section -> select the *entire* axis ("all"),
    ///   - scalar            -> collapse to that position (axis dropped from result),
    ///   - vector of rank s  -> pick those positions; contributes s axes (its shape).
    /// A fully-collapsed selection (every axis a scalar) errors, matching Kap.
    fn pick(&self, target: &APLValue, selector: &APLValue) -> Result<AplRef<APLValue>, AplError> {
        let target = target.force(self)?;
        let selector = selector.force(self)?;

        // Selector -> axis specs; `⍬`/Null means "all" for that axis.
        let specs: Vec<AplRef<APLValue>> = match selector.as_ref() {
            APLValue::Array(a) => a.elements(),
            other => vec![Rc::new(other.clone())],
        };

        // ---- Single section: PickResultValue ----
        if specs.len() == 1 {
            let index = specs.into_iter().next().unwrap();
            let index = index.force(self)?;
            if matches!(index.as_ref(), APLValue::Null) {
                return Ok(Rc::new(APLValue::Null));
            }
            let target_elems = match target.as_ref() {
                APLValue::Array(a) => a.elements(),
                _ => vec![Rc::new(target.as_ref().clone())],
            };
            let target_rank = match target.as_ref() {
                APLValue::Array(a) => a.rank(),
                _ => 0,
            };
            if target_rank != 1 {
                return Err(AplError::runtime(format!(
                    "pick into rank-{} array requires coordinate indices",
                    target_rank
                )));
            }
            let idx_dims = match index.as_ref() {
                APLValue::Array(a) => a.dimensions.clone(),
                _ => vec![],
            };
            let idx_elems = match index.as_ref() {
                APLValue::Array(a) => a.elements(),
                other => vec![Rc::new(other.clone())],
            };
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(idx_elems.len());
            for e in &idx_elems {
                let i = self.index_to_i64(e.as_ref())?;
                let adj = check_and_adjust_selected_index(i, target_elems.len())?;
                out.push(Rc::new(target_elems[adj].as_ref().clone()));
            }
            if idx_dims.is_empty() {
                Ok(out.into_iter().next().unwrap())
            } else {
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                    idx_dims,
                    ArrayData::Nested(out),
                )))))
            }
        } else {
            // ---- Multi-axis: AccessFromIndex ----
            let bdims = match target.as_ref() {
                APLValue::Array(a) => a.dimensions.clone(),
                _ => vec![1],
            };
            let r = bdims.len();
            let na = specs.len();
            if na > r {
                return Err(AplError::runtime(format!(
                    "too many index axes ({} specifiers for rank-{})",
                    na, r
                )));
            }
            let bstride: Vec<usize> = {
                let mut s = vec![1usize; r];
                for k in (0..r).rev() {
                    if k + 1 < r {
                        s[k] = s[k + 1] * bdims[k + 1];
                    }
                }
                s
            };

            enum Spec {
                All(usize),
                Collapsed(usize),
                VecIndex {
                    v_dims: Vec<usize>,
                    v_elems: Vec<AplRef<APLValue>>,
                    out_start: usize,
                },
            }

            let mut plans: Vec<(usize, Spec)> = Vec::with_capacity(r);
            let mut out_dims: Vec<usize> = Vec::new();
            let mut next_out = 0usize;
            for k in 0..r {
                let spec_val = if k < na {
                    specs[k].force(self)?
                } else {
                    Rc::new(APLValue::Null)
                };
                match spec_val.as_ref() {
                    APLValue::Null => {
                        let axis = next_out;
                        out_dims.push(bdims[k]);
                        next_out += 1;
                        plans.push((k, Spec::All(axis)));
                    }
                    APLValue::Array(v) => {
                        let v_dims = v.dimensions.clone();
                        let v_elems = v.elements();
                        let out_start = next_out;
                        for &d in &v_dims {
                            out_dims.push(d);
                        }
                        next_out += v_dims.len();
                        plans.push((
                            k,
                            Spec::VecIndex {
                                v_dims,
                                v_elems,
                                out_start,
                            },
                        ));
                    }
                    other => {
                        let i = self.index_to_i64(other)?;
                        let adj = check_and_adjust_selected_index(i, bdims[k])?;
                        plans.push((k, Spec::Collapsed(adj)));
                    }
                }
            }

            let target_elems = match target.as_ref() {
                APLValue::Array(a) => a.elements(),
                _ => vec![Rc::new(target.as_ref().clone())],
            };

            if out_dims.is_empty() {
                // Fully-collapsed selection (every axis a scalar): disclose the single element.
                let mut src_coords = vec![0usize; r];
                for (k, spec) in &plans {
                    if let Spec::Collapsed(c) = spec {
                        src_coords[*k] = *c;
                    }
                }
                let mut tflat = 0usize;
                for k in 0..r {
                    tflat += src_coords[k] * bstride[k];
                }
                return Ok(Rc::new(target_elems[tflat].as_ref().clone()));
            }

            let ostride: Vec<usize> = {
                let mut s = vec![1usize; out_dims.len()];
                for k in (0..out_dims.len()).rev() {
                    if k + 1 < out_dims.len() {
                        s[k] = s[k + 1] * out_dims[k + 1];
                    }
                }
                s
            };

            let result_size: usize = out_dims.iter().product();
            if result_size == 0 || result_size > 100_000_000 {
                return Err(AplError::runtime("index selection result too large".into()));
            }
            let mut out: Vec<AplRef<APLValue>> = Vec::with_capacity(result_size);
            let mut out_coords = vec![0usize; out_dims.len()];
            for pos in 0..result_size {
                let mut rem = pos;
                for ax in 0..out_dims.len() {
                    out_coords[ax] = rem / ostride[ax];
                    rem %= ostride[ax];
                }
                let mut src_coords = vec![0usize; r];
                for (k, spec) in &plans {
                    match spec {
                        Spec::All(axis) => src_coords[*k] = out_coords[*axis],
                        Spec::Collapsed(c) => src_coords[*k] = *c,
                        Spec::VecIndex {
                            v_dims,
                            v_elems,
                            out_start,
                        } => {
                            let mut vflat = 0usize;
                            for d in 0..v_dims.len() {
                                vflat = vflat * v_dims[d] + out_coords[out_start + d];
                            }
                            let vval = &v_elems[vflat];
                            let i = self.index_to_i64(vval.as_ref())?;
                            src_coords[*k] = check_and_adjust_selected_index(i, bdims[*k])?;
                        }
                    }
                }
                let mut tflat = 0usize;
                for k in 0..r {
                    tflat += src_coords[k] * bstride[k];
                }
                out.push(Rc::new(target_elems[tflat].as_ref().clone()));
            }
            Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(
                out_dims,
                ArrayData::Nested(out),
            )))))
        }
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

    /// Number of "user-function argument slots" a value fills: a scalar is 1, an array is
    /// its element count (matches Kap's `;`/`⍵`-destructuring semantics).
    fn element_count(&self, v: &APLValue) -> usize {
        match v {
            APLValue::Array(a) => a.element_count(),
            _ => 1,
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

    /// Grade up `⍋ x`: 0-based indices that would sort `x` ascending.
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
            .map(|i| Rc::new(APLValue::Number(KapNumber::Long(i as i64))))
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
        assert_eq!(eval("(1 + 2) × 3"), "9");
    }

    #[test]
    fn eval_parenthesised_groups() {
        // Grouping in arithmetic and with assignment/lambda/strand inside.
        assert_eq!(eval("(1 + 2) × 3"), "9");
        assert_eq!(eval("2 × (3 + 4)"), "14");
        assert_eq!(eval("(1 + 2) × (3 + 4)"), "21");
        assert_eq!(eval("((1 + 2))"), "3");
        assert_eq!(eval("(⍳3) + 10"), "(10 11 12)");
        assert_eq!(eval("1 + (2 × 3)"), "7");
        assert_eq!(eval("(x ← 5) + 1"), "6");
        assert_eq!(eval("f ← λ(x) x × 2 ⋄ (f 5) + 1"), "11");
        assert_eq!(eval("(1 2 3) + 10"), "(11 12 13)");
        assert_eq!(eval("f ← λ(x) x × 2 ⋄ f (3 + 4)"), "14");
    }

    #[test]
    fn eval_juxtaposed_groups_strand() {
        // A parenthesised group of values strands into a vector.
        assert_eq!(eval("(1 2 3)"), "(1 2 3)");
    }

    #[test]
    fn eval_nested_arrays() {
        // Nested arrays (depth > 1) parse as a vector containing a nested vector.
        assert_eq!(eval("(1 2 (3 4 5))"), "(1 2 (3 4 5))");
        // Deeper nesting.
        assert_eq!(eval("(1 (2 (3 (4 5))))"), "(1 (2 (3 (4 5))))");
        // A parenthesised group within a strand stays a single element — NOT flattened.
        assert_eq!(eval("((1 2 3) 4 5)"), "((1 2 3) 4 5)");
        assert_eq!(eval("(1 2 3) 4 5"), "((1 2 3) 4 5)");
        // A nested vector element can still be used as data (catenate strands it in).
        assert_eq!(eval("1 2 , (3 4)"), "(1 2 3 4)");
    }

    #[test]
    fn eval_monadic_arithmetic() {
        // `+` and `-` are ambivalent: monadic `- x` = negate, `+ x` = identity.
        assert_eq!(eval("-(1 + 2)"), "¯3");
        assert_eq!(eval("+(1 + 2)"), "3");
        assert_eq!(eval("-(3 1 4)"), "(¯3 ¯1 ¯4)");
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
        assert_eq!(eval("⍳5"), "(0 1 2 3 4)");
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
        assert_eq!(eval("[10; 20; 30]"), "(10 20 30)");
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
        assert_eq!(eval("1 , 2 , 3"), "(1 2 3)");
        assert_eq!(eval("[1; 2] , [3; 4]"), "(1 2 3 4)");
    }

    #[test]
    fn eval_reverse() {
        assert_eq!(eval("⌽ ⍳5"), "(4 3 2 1 0)");
    }

    #[test]
    fn eval_take_drop() {
        assert_eq!(eval("3 ↑ ⍳10"), "(0 1 2)");
        assert_eq!(eval("3 ↓ ⍳10"), "(3 4 5 6 7 8 9)");
    }

    #[test]
    fn eval_assign_and_var() {
        assert_eq!(eval("x ← 5 ⋄ x + 1"), "6");
    }

    #[test]
    fn eval_lambda_apply() {
        assert_eq!(eval("f ← λ(x) x × 2 ⋄ f 5"), "10");
        assert_eq!(eval("g ← λ(a b) a + b ⋄ 3 g 4"), "7");
    }

    #[test]
    fn eval_strand() {
        assert_eq!(eval("1 2 3 + 10"), "(11 12 13)");
    }

    // --- Phase 6: more builtins ---

    #[test]
    fn eval_ceil_floor() {
        assert_eq!(eval("⌈ 3.2"), "4.0");
        assert_eq!(eval("⌊ 3.8"), "3.0");
        assert_eq!(eval("⌈ 5"), "5");
        assert_eq!(eval("⌈ 1.5 2.5 3.5"), "(2.0 3.0 4.0)");
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
        assert_eq!(eval("~ 1 0 3"), "(0 1 0)");
    }

    #[test]
    fn eval_membership() {
        assert_eq!(eval("2 9 4 ∊ 1 2 3 4"), "(1 0 1)");
    }

    #[test]
    fn eval_grade_up() {
        // 0-based indices (Kap is 0-based): ⍋ 3 1 4 1 5 -> positions ascending by value
        assert_eq!(eval("⍋ 3 1 4 1 5"), "(1 3 0 2 4)");
    }

    #[test]
    fn eval_encode_decode() {
        // 2 2 2 ⊤ 5  -> binary-ish mixed radix of 5 = [1 0 1]
        assert_eq!(eval("2 2 2 ⊤ 5"), "(1 0 1)");
        // inverse: 2 2 2 ⊥ 1 0 1 -> 1*4 + 0*2 + 1 = 5
        assert_eq!(eval("2 2 2 ⊥ 1 0 1"), "5");
        // 24 60 ⊤ 90 -> 1 hour 30 min
        assert_eq!(eval("24 60 ⊤ 90"), "(1 30)");
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
        assert_eq!(eval("+\\ 1 2 3 4"), "(1 3 6 10)");
        assert_eq!(eval("×\\ 1 2 3 4"), "(1 2 6 24)");
    }

    #[test]
    fn eval_each_monadic() {
        assert_eq!(eval("⌈¨ 1.2 2.8 3.5"), "(2.0 3.0 4.0)");
        assert_eq!(eval("~¨ 1 0 3"), "(0 1 0)");
    }

    #[test]
    fn eval_each_dyadic() {
        // element-wise: 2 ×¨ 3 4 5  -> [6 8 10]
        assert_eq!(eval("2 ×¨ 3 4 5"), "(6 8 10)");
        // scalar-extended left, vector right
        assert_eq!(eval("1 2 3 +¨ 4 5 6"), "(5 7 9)");
        // vector × vector each
        assert_eq!(eval("1 2 3 ×¨ 4 5 6"), "(4 10 18)");
    }

    // --- Phase 8: control flow (if / while / when / block) ---

    #[test]
    fn eval_if_then() {
        assert_eq!(eval("if (1 < 2) { 42 }"), "42");
        assert_eq!(eval("if (1 > 2) { 42 } else { 7 }"), "7");
        // no else, false condition -> null
        assert_eq!(eval("if (0) { 1 }"), "null");
    }

    #[test]
    fn eval_block_value() {
        // block returns last statement's value
        assert_eq!(eval("{ 1 ⋄ 2 ⋄ 3 }"), "3");
        // assignment inside a block is visible after (same env)
        assert_eq!(eval("x ← 0 ⋄ { x ← 5 } ⋄ x"), "5");
    }

    #[test]
    fn eval_while_loop() {
        // sum 1..5 via while
        assert_eq!(
            eval("i ← 0 ⋄ s ← 0 ⋄ while (i < 5) { s ← s + i ⋄ i ← i + 1 } ⋄ s"),
            "10"
        );
    }

    #[test]
    fn eval_when() {
        // when with a trailing (1) default clause
        assert_eq!(eval("b ← 2 ⋄ when { (b=1){ \"one\" } (b=2){ \"two\" } (1){ \"other\" } }"), "two");
        assert_eq!(eval("b ← 9 ⋄ when { (b=1){ \"one\" } (b=2){ \"two\" } (1){ \"other\" } }"), "other");
    }

    // --- Phase 9: trains (function chains / forks) ---

    #[test]
    fn eval_train_fork() {
        // x (A « B » C) y = (x A y) B (x C y)
        assert_eq!(eval("3 (+ « × » -) 4"), "¯7");
    }

    #[test]
    fn eval_train_left_bind() {
        // x (c f) y = f(c, y)  (left-bind: bound value, then a function)
        assert_eq!(eval("(10+) 1"), "11");
        assert_eq!(eval("10 (-⍛+) 100"), "90"); // reverse-compose: g(f(x), y)
        assert_eq!(eval("((1+)⍛-) 1"), "1");
    }

    #[test]
    fn eval_train_compose() {
        // x (f ∘ g) y = f(y, g(y))  (compose is dyadic: f(y, g(y)))
        assert_eq!(eval("¯2 3 4 (×∘-) 1000"), "(2000 ¯3000 ¯4000)");
        // monadic compose with reciprocal: (×∘÷) y = y × (1/y) = y, exactly 1 for all y≠0.
        assert_eq!(eval("(×∘÷) ¯1 2 3"), "(1 1r1 1r1)");
    }

    #[test]
    fn eval_train_atop() {
        // x (f g) y = f(x g y)  (atop: g dyadic between x and y)
        assert_eq!(eval("2 (-*) 5"), "¯32"); // -(2*5)
        assert_eq!(eval("10 (-,) 20"), "(¯10 ¯20)"); // -(10,20) = (-10,-20)
    }
}
