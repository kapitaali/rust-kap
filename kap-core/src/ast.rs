//! AST for Kap (Phase 2b).
//!
//! Mirrors `array/.../instr.kt` (`Instr` hierarchy) at a coarse grain. The Kotlin `Instr`
//! is a large sealed-ish hierarchy; for the Rust port we use a compact `Instr` enum that
//! captures the constructs the evaluator needs. Strategy §4.3.
//!
//! Reference: docs/reference.asciidoc (syntax sections).

use crate::KapNumber;
use crate::token::LiteralValue;

/// A parsed expression. This is what the evaluator (Phase 3) consumes.
#[derive(Debug, Clone)]
pub enum Instr {
    /// A literal scalar value (number, char, string, null).
    Literal(LiteralValue),
    /// A variable / function name reference. `namespace` is `Some` for `foo:bar`.
    Symbol { name: String, namespace: Option<String> },
    /// A quoted **symbol value** literal (`'foo`): evaluates to `APLValue::Symbol`
    /// (Kotlin `parser.kt` SymbolValue). Distinct from `Symbol` (a name reference).
    SymbolValue { name: String },
    /// Monadic or dyadic function application: `f x` (monadic) or `a f b` (dyadic).
    /// `fn_expr` is the function (a Symbol or parenthesised expr); `left`/`right` are args.
    Apply {
        fn_expr: Box<Instr>,
        left: Option<Box<Instr>>,
        right: Box<Instr>,
    },
    /// Assignment: `target ← value`.
    Assign {
        target: Box<Instr>,
        value: Box<Instr>,
    },
    /// An array/vector literal: `[a;b;c]` (explicit) or a stranded vector `a b c`.
    Array { elements: Vec<Instr> },
    /// A lambda / anonymous function: `λ(params) body`. `params` are argument names;
    /// `body` is the unevaluated expression. Evaluated (Phase 4) into an `APLValue::UserFn`.
    Lambda { params: Vec<String>, body: Box<Instr> },
    /// A *derived function* from an adverb: `func op` (e.g. `+/`, `×¨`). `func` is the
    /// function operand, `op` is the adverb (`/`, `\\`, `¨`). The evaluator resolves `op`
    /// to reduce/scan/each and applies `func` to the data arguments.
    Derived { func: Box<Instr>, op: Box<Instr> },
    /// A block: `{ stmt1 ⋄ stmt2 ⋄ ... }` — a sequence of statements evaluated in order;
    /// the value of the block is the value of its last statement. Used for control-flow
    /// bodies and as a standalone scoped expression.
    Block { body: Vec<Instr> },
    /// `if (cond) { then }` or `if (cond) { then } else { alt }`.
    If {
        cond: Box<Instr>,
        then_block: Box<Instr>,
        else_block: Option<Box<Instr>>,
    },
    /// `while (cond) { body }` — repeats `body` while `cond` is truthy.
    While { cond: Box<Instr>, body: Box<Instr> },
    /// `when { (cond){ body } … (1){ default } }` — first truthy clause's body is evaluated.
    When { clauses: Vec<(Instr, Instr)> },
    /// A *train*: a sequence of functions (and possibly a bound value), e.g. `(f g h)`,
    /// `f ∘ g`, `A « B » C`, `f ⍛ g`, or a left-bind `(c f)`.
    ///
    /// Semantics (from ComposeTest.kt / operator.kt):
    /// - Atop / bare 2-train `(f g)` (compose=false, reverse=false):
    ///     monadic `(f g) y` = `f(g(y))`; dyadic `x (f g) y` = `f(x g y)`.
    /// - Compose `f ∘ g` (compose=true, reverse=false):
    ///     monadic `(f∘g) y` = `f(y, g(y))`; dyadic `x (f∘g) y` = `f(x, g(y))` (g monadic).
    /// - Reverse-compose `f ⍛ g` (reverse=true):
    ///     monadic `(f⍛g) y` = `g(f(y), y)`; dyadic `x (f⍛g) y` = `g(f(x), y)` (f monadic, g dyadic).
    /// - Fork `A « B » C` (3-train): monadic `(A y) B (C y)`; dyadic `(x A y) B (x C y)`.
    /// - Left-bind `[value, fn]` (2-train, first member a value): `(c f) y` = `f(c, y)`.
    Train { funcs: Vec<Instr>, reverse: bool, compose: bool },
    /// A *user-defined function* definition: `∇ (leftargs) name (rightargs) { body }`.
    /// `left_params`/`right_params` are the parameter names (possibly empty). `split` =
    /// `left_params.len()` (the point at which the combined param list is divided into
    /// the dyadic left and right argument bindings). `body` is the unevaluated expression.
    /// The evaluator compiles this into an `APLValue::UserFn` capturing a closure env.
    UserFnDef {
        name: String,
        namespace: Option<String>,
        left_params: Vec<String>,
        right_params: Vec<String>,
        body: Box<Instr>,
    },
    /// An anonymous function *assignment* via `⇐`: `name ⇐ <fn-expr>`. The right side
    /// is any function-valued expression (a lambda, a train, a builtin name, or another
    /// named function). Compiled like `UserFnDef` but without a separate left/right split
    /// other than what the function expression itself carries.
    FnAssign { name: String, namespace: Option<String>, value: Box<Instr> },
    /// A *guarded expression* (Kap's `:` operator): `cond : truthy ⋄ falsy`.
    /// Evaluates `cond`; if truthful returns `truthy`, otherwise `falsy`. The `⋄` between
    /// the two branches is mandatory. Low precedence — each side is a full expression.
    Guard {
        cond: Box<Instr>,
        truthy: Box<Instr>,
        falsy: Box<Instr>,
    },
    /// A pre-evaluated runtime value wrapped as an expression (used internally to pass
    /// already-computed results back into `eval_apply`, e.g. by trains).
    Value(crate::AplRef<crate::APLValue>),
    /// A *dynamic function reference* (Kap's `⍞name` / ApplyToken): evaluates the named
    /// variable to a *value*, which must be a function, and applies it. `⍞x` is like
    /// writing `x` but forces `x` to be interpreted as a function even if `x` would
    /// otherwise be a value. Used inside operator bodies to apply a function-operand.
    DynamicRef { name: String, namespace: Option<String> },
    /// A *user-defined operator* definition: `∇ (x foo) a { … }` (1 function-operand) or
    /// `∇ (x foo y) a { … }` (2 function-operands). `op_left`/`op_right` are the names
    /// bound to the function-operands at the call site; `left_params`/`right_params` are
    /// the ordinary data arguments. The evaluator compiles this into an `APLValue::UserOp`.
    UserOpDef {
        name: String,
        op_left: Option<String>,
        op_right: Option<String>,
        left_params: Vec<String>,
        right_params: Vec<String>,
        body: Box<Instr>,
    },
    /// An *operator call* with function operands, e.g. `+foo 2` (one function-operand)
    /// or `-foo+ 3` (two function-operands). `op` names a user-defined operator;
    /// `left_fn`/`right_fn` are the function operands bound to its `op_left`/`op_right`
    /// parameters. The result is itself a function, applied to the data arguments that
    /// follow (so `+foo 2` = `Apply { fn_expr: OpCall{…}, right: 2 }`).
    OpCall {
        op: Box<Instr>,
        left_fn: Box<Instr>,
        right_fn: Option<Box<Instr>>,
    },
    /// Inner/outer product `f₁ ∙ f₂` (Kotlin OuterInnerJoinOp, engine.kt:489).
    /// ONE operator covers BOTH products (outer_join.kt:176): `left_fn == None`
    /// ⇒ OUTER product (`∘.f` equivalent, e.g. `1 2 ∘∙× 3 4`); else INNER join
    /// (`A +∙× B`). The result is a function applied to the data arguments.
    /// `∙` is U+2219 (BULLET OPERATOR) — NOT `.` (MemberDereferenceToken).
    InnerProduct {
        left_fn: Option<Box<Instr>>,
        right_fn: Box<Instr>,
    },
    /// A function with an explicit axis specifier: `f[axis]` (e.g. `+[0]`). Mirrors
    /// Kotlin's `AxisValAssignedFunctionDirect`, created by `parseOperator` when a
    /// `[axis]` follows the function. `eval_apply` unwraps it and threads the axis into
    /// the builtin (currently only scalar arithmetic functions support an axis).
    AxisApplied { func: Box<Instr>, axis: Box<Instr> },
    /// A *value-right-arg operator* binding: `f⍤rank` (Kotlin `APLOperatorValueRightArg`,
    /// engine.kt:494 `registerNativeOperator("⍤", RankOperator())`). Unlike an adverb,
    /// the right operand is a VALUE expression evaluated at application time. The
    /// evaluator resolves this as the rank operator: split each argument into cells of
    /// rank `k` and apply `func` to every cell, disclosing the result.
    ValueOp { func: Box<Instr>, op_name: String, operand: Box<Instr> },
    /// An empty array / nil.
    Empty,
    /// Array *pick* / selection: `array[selector]`. `selector` is an index expression
    /// (a scalar or vector of integers) parsed from `[...]`. Semantics (Kap `PickAPLFunction`
    /// / `PickResultValue`): 1-D, each index `i` is adjusted with negative-from-end support
    /// (`checkAndAdjustSelectedIndex`); a 1-element selection is disclosed to a scalar, a
    /// multi-element selection is a vector of the picked elements. Binds tightly to the
    /// preceding primary (postfix), so `a b (c d)[0] e` indexes `(c d)`, not the whole strand.
    Index { array: Box<Instr>, selector: Box<Instr> },
    /// Short-circuit boolean operator: `and` / `or` (Kotlin `AndToken`/`OrToken` →
    /// `BooleanAndFunction`/`BooleanOrFunction`). These are *not* the bitwise `∧`/`∨`
    /// functions — they sit at the **lowest precedence** (below assignment) and evaluate
    /// the left operand first; if its truthiness decides the result, the right operand is
    /// NOT evaluated (lazy). The result is the *raw* operand value, not a coerced 0/1.
    ///   `a and b` → if truthy(a) then b else a
    ///   `a or  b` → if truthy(a) then a else b
    BooleanOp { op: BooleanOpKind, left: Box<Instr>, right: Box<Instr> },
    /// `defsyntax name (rules…) { body }` — register a parse-time macro. The macro
    /// definition is evaluated at runtime (into the engine's syntax registry) and
    /// produces no value. The body `Instr` is stored so it can be re-parsed/expanded
    /// when the macro trigger name is later encountered as a call.
    DefSyntax {
        name: String,
        namespace: Option<String>,
        rules: Vec<SyntaxRule>,
        body: std::rc::Rc<Instr>,
    },
    /// `defsyntaxsub name (rules…) { body }` — a sub-rule used by a parent macro's
    /// `:repeat (sym subName)` clause. Registered into the engine's sub-syntax registry.
    DefSyntaxSub {
        name: String,
        namespace: Option<String>,
        rules: Vec<SyntaxRule>,
        body: std::rc::Rc<Instr>,
    },
    /// Destructuring assignment `(a b c) ← expr` — bind each LHS symbol to the
    /// corresponding element of the (vector) RHS (Kotlin `AssignmentInstruction` with
    /// multiple targets). `names` are the bare/grouped targets in order.
    DestructAssign {
        names: Vec<(String, Option<String>)>,
        value: Box<Instr>,
    },
    /// A macro *expansion*: splice `body` with `bindings` (var name → already-parsed
    /// `Instr`) defined in a child scope. Faithful port of Kotlin's
    /// `CallWithVarInstruction` (built by `processCustomSyntax`).
    MacroExpand {
        body: std::rc::Rc<Instr>,
        bindings: Vec<(String, Box<Instr>)>,
    },
}

/// Which short-circuit boolean operator a `BooleanOp` represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOpKind {
    And,
    Or,
}

/// A single `defsyntax` rule, mirroring Kotlin `syntax.kt`'s `SyntaxRule` subclasses.
/// Only the rule kinds the stdlib actually uses are modelled:
/// `:function`/`:nfunction`/`:nexprfunction` (a `{…}` block parsed as a no-param lambda),
/// `:value` (a `(…)` parenthesised expression), `:string` (a string literal),
/// `:special :openBrace|:closeBrace|:newline` (a literal token), `:optional (…)`
/// (try the inner rules), and `:repeat (name subName)` (repeat a sub-macro while it matches).
#[derive(Debug, Clone)]
pub enum SyntaxRule {
    /// A `{…}` function block → bound to `var` as a no-param `Instr::Lambda`.
    Function { var: String },
    /// Same, but the body is evaluated in the *current* environment (no new env). For our
    /// purposes identical to `Function` (we don't re-bind lexical scopes per rule).
    NFunction { var: String },
    /// An expression-function `(…)` → bound to `var` as the parsed inner `Instr`.
    ExprFunction { var: String },
    NExprFunction { var: String },
    /// A `(…)` value expression → bound to `var`.
    Value { var: String },
    /// A string literal → bound to `var` as a `Str` literal.
    String { var: String },
    /// A literal special token (`{`, `}`, newline) that must be consumed verbatim.
    Special { token: SpecialToken },
    /// Optional rules: if the next token matches the head rule's shape, consume them.
    Optional { inner: Vec<SyntaxRule> },
    /// Repeat a named sub-macro while it matches, binding `var` to the vector of results.
    Repeat { var: String, sub: String },
}

/// The literal tokens a `:special` rule can demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialToken {
    OpenBrace,
    CloseBrace,
    Newline,
}

/// A registered `defsyntax` macro (Kotlin `CustomSyntax`). `submacros` holds the
/// `defsyntaxsub` definitions referenced by `:repeat` clauses (keyed by bare sub-name).
/// The body is an `Rc<Instr>` so the macro can be cloned into the per-statement parser
/// snapshot without cloning the (non-`Clone`) `Instr` tree.
#[derive(Debug, Clone)]
pub struct SyntaxMacro {
    pub rules: Vec<SyntaxRule>,
    pub body: std::rc::Rc<Instr>,
}

impl Instr {
    /// Convenience: build a literal number Instr.
    pub fn number(n: KapNumber) -> Instr {
        Instr::Literal(LiteralValue::Number(n))
    }
    /// Convenience: build a symbol Instr.
    pub fn symbol(name: &str) -> Instr {
        Instr::Symbol { name: name.to_string(), namespace: None }
    }
}
