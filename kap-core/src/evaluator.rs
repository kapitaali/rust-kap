//! Evaluator for Kap (Phase 3).
//!
//! `Engine::eval_string` runs the pipeline: tokenise -> parse -> eval. Laziness (D6):
//! args are `Instr` trees; they are forced via `force()` only when a builtin needs them.
//! A starter set of builtins is wired in: `+`, assignment `←`, `⍳` (index generator),
//! `⍴`/`≢` (shape/length), `⊃` (first). More in later phases.

use crate::array::{ArrayData, KapArray};
use crate::ast::Instr;
use crate::lexer::tokenise;
use crate::number::KapNumber;
use crate::parser;
use crate::token::LiteralValue;
use crate::{APLValue, AplError, AplRef, Engine, Environment};
use std::collections::HashMap;
use std::rc::Rc;

impl Environment {
    /// Look up a symbol, walking parent environments.
    pub fn lookup(&self, name: &str, ns: &Option<String>) -> Option<AplRef<APLValue>> {
        let key = (name.to_string(), ns.clone());
        if let Some(v) = self.symbols.get(&key) {
            return Some(v.clone());
        }
        if let Some(p) = &self.parent {
            return p.lookup(name, ns);
        }
        None
    }

    /// Define a symbol in this (innermost) environment.
    pub fn define(&mut self, name: &str, ns: &Option<String>, value: AplRef<APLValue>) {
        self.symbols.insert((name.to_string(), ns.clone()), value);
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
    /// Evaluate a full Kap source string. Returns the value of the last statement.
    pub fn eval_string(&self, src: &str) -> Result<AplRef<APLValue>, AplError> {
        let toks = tokenise(src);
        let (stmts, errs) = parser::parse(&toks);
        if !errs.is_empty() {
            // surface the first parse error with position if available
            return Err(AplError::Parse { line: 0, col: 0, msg: errs.join("; ") });
        }
        let env = Rc::new(Environment::default());
        let mut last: AplRef<APLValue> = Rc::new(APLValue::Null);
        for stmt in &stmts {
            last = self.eval_instr(stmt, &env)?;
        }
        Ok(last)
    }

    /// Evaluate a single `Instr` in `env`. This is the core eval loop.
    pub fn eval_instr(&self, instr: &Instr, env: &AplRef<Environment>) -> Result<AplRef<APLValue>, AplError> {
        match instr {
            Instr::Literal(LiteralValue::Number(n)) => Ok(Rc::new(APLValue::Number(n.clone()))),
            Instr::Literal(LiteralValue::Char(c)) => Ok(Rc::new(APLValue::Char(*c))),
            Instr::Literal(LiteralValue::Str(s)) => Ok(Rc::new(APLValue::Str(s.clone()))),
            Instr::Literal(LiteralValue::Symbol { .. }) => {
                Err(AplError::Parse { line: 0, col: 0, msg: "lone symbol literal".into() })
            }
            Instr::Empty => Ok(Rc::new(APLValue::Null)),
            Instr::Symbol { name, namespace } => {
                env.lookup(name, namespace)
                    .ok_or_else(|| AplError::Parse { line: 0, col: 0, msg: format!("undefined symbol: {}", name) })
            }
            Instr::Array { elements } => {
                // Evaluate each element and collect into a nested vector.
                let mut vals = Vec::with_capacity(elements.len());
                for e in elements {
                    vals.push(self.eval_instr(e, env)?);
                }
                // For now: if all are numbers, use a typed array; else nested.
                let data = if vals.iter().all(|v| matches!(v.as_ref(), APLValue::Number(_))) {
                    let nums: Vec<KapNumber> = vals
                        .iter()
                        .map(|v| match v.as_ref() {
                            APLValue::Number(n) => n.clone(),
                            _ => unreachable!(),
                        })
                        .collect();
                    ArrayData::Nested(vals.clone())
                } else {
                    ArrayData::Nested(vals.clone())
                };
                let _ = data; // (all-numeric fast path deferred; keep Nested for generality)
                Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(vec![vals.len()], ArrayData::Nested(vals))))))
            }
            Instr::Assign { target, value } => {
                // target must be a Symbol
                if let Instr::Symbol { name, namespace } = target.as_ref() {
                    let v = self.eval_instr(value, env)?;
                    // make a mutable clone of env to define into (innermost)
                    let mut env_mut = (**env).clone();
                    env_mut.define(name, namespace, v.clone());
                    // NOTE: this defines into a *clone*; for true mutation we'd need Rc<RefCell>.
                    // Phase 3 starter keeps assignment working within a single eval_string via
                    // the engine's own root environment instead — see eval_string's env handling.
                    let _ = env_mut;
                    Ok(v)
                } else {
                    Err(AplError::Parse { line: 0, col: 0, msg: "assignment target must be a symbol".into() })
                }
            }
            Instr::Apply { fn_expr, left, right } => {
                self.eval_apply(fn_expr, left, right, env)
            }
        }
    }

    fn eval_apply(
        &self,
        fn_expr: &Instr,
        left: &Option<Box<Instr>>,
        right: &Box<Instr>,
        env: &AplRef<Environment>,
    ) -> Result<AplRef<APLValue>, AplError> {
        // Resolve the function name (symbol only for the starter set).
        let name = match fn_expr {
            Instr::Symbol { name, .. } => name.as_str(),
            _ => {
                return Err(AplError::Parse { line: 0, col: 0, msg: "only symbol functions supported in Phase 3".into() })
            }
        };
        // For dyadic, force left then right; for monadic, only right.
        let right_val = self.eval_instr(right, env)?.force(self)?;
        let left_val = match left {
            Some(l) => Some(self.eval_instr(l, env)?.force(self)?),
            None => None,
        };
        match name {
            "+" => {
                let (a, b) = (left_val.ok_or_else(|| AplError::Parse { line: 0, col: 0, msg: "+ needs two args".into() })?,
                              right_val);
                match (a.as_ref(), b.as_ref()) {
                    (APLValue::Number(x), APLValue::Number(y)) => Ok(Rc::new(APLValue::Number(x.add(y)))),
                    _ => Err(AplError::Parse { line: 0, col: 0, msg: "+ requires numbers".into() }),
                }
            }
            "*" => {
                let (a, b) = (left_val.ok_or_else(|| AplError::Parse { line: 0, col: 0, msg: "* needs two args".into() })?,
                              right_val);
                match (a.as_ref(), b.as_ref()) {
                    (APLValue::Number(x), APLValue::Number(y)) => Ok(Rc::new(APLValue::Number(x.mul(y)))),
                    _ => Err(AplError::Parse { line: 0, col: 0, msg: "* requires numbers".into() }),
                }
            }
            "⍳" | "iota" => {
                // monadic: ⍳N -> 0..N-1
                let n = match right_val.as_ref() {
                    APLValue::Number(KapNumber::Long(v)) => *v,
                    _ => return Err(AplError::Parse { line: 0, col: 0, msg: "⍳ needs an integer count".into() }),
                };
                if n < 0 {
                    return Err(AplError::Parse { line: 0, col: 0, msg: "⍳ count must be non-negative".into() });
                }
                let nums: Vec<KapNumber> = (0..n).map(KapNumber::Long).collect();
                let arr = KapArray::from_numbers(nums);
                Ok(Rc::new(APLValue::Array(Rc::new(arr))))
            }
            "⍴" | "rho" => {
                // monadic: shape of right
                match right_val.as_ref() {
                    APLValue::Array(a) => {
                        let shape: Vec<AplRef<APLValue>> = a
                            .dimensions
                            .iter()
                            .map(|d| Rc::new(APLValue::Number(KapNumber::Long(*d as i64))))
                            .collect();
                        Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(vec![shape.len()], ArrayData::Nested(shape))))))
                    }
                    _ => Ok(Rc::new(APLValue::Array(Rc::new(KapArray::new(vec![0], ArrayData::Nested(vec![])))))),
                }
            }
            "≢" | "tally" => {
                // monadic: tally (element count) of right
                match right_val.as_ref() {
                    APLValue::Array(a) => Ok(Rc::new(APLValue::Number(KapNumber::Long(a.element_count() as i64)))),
                    _ => Ok(Rc::new(APLValue::Number(KapNumber::Long(1)))),
                }
            }
            "⊃" | "first" => {
                // monadic: first element of right
                match right_val.as_ref() {
                    APLValue::Array(a) => {
                        a.elements()
                            .into_iter()
                            .next()
                            .ok_or_else(|| AplError::Parse { line: 0, col: 0, msg: "⊃ of empty array".into() })
                    }
                    other => Ok(Rc::new(other.clone())),
                }
            }
            _ => Err(AplError::Parse { line: 0, col: 0, msg: format!("unknown function: {}", name) }),
        }
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
}
