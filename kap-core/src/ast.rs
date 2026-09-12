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
    /// `namespace` mirrors `Symbol`: `None` for bare `'foo`, `Some("kap")` for `'kap:array`.
    SymbolValue { name: String, namespace: Option<String> },
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
    /// Like `Array`, but carries an explicit `dims` vector. Used ONLY by
    /// `Evaluator::apl_to_instr` when round-tripping an already-evaluated
    /// `APLValue::Array` back into an `Instr` (e.g. as a reduce/scan/inner-product
    /// fold operand). The parser never emits this, so parser strands stay rank-1.
    /// `elements` is the **ravel** of the array (row-major), matching `dims`.
    ArrayWithShape { dims: Vec<usize>, elements: Vec<Instr> },
    /// A `;`-separated *list* literal: `(1;2;3)`. Distinct from `Array` (which is
    /// space-stranded) because Kap's destructuring assignment `(a;b;c)←RHS`
    /// requires the RHS to be a *list* (`;`-separated), not a plain array.
    /// Kotlin: `LiteralAPLList` (instr.kt:68) → `APLList`.
    List { elements: Vec<Instr> },
    /// A lambda / anonymous function: `λ(params) body`. `params` are argument names;
    /// `body` is the unevaluated expression. Evaluated (Phase 4) into an `APLValue::UserFn`.
    Lambda { params: Vec<String>, body: Box<Instr> },
    /// A non-binding macro function argument (`:nfunction` / `:nexprfunction`
    /// in `defsyntax`). `body` is the UNEVALUATED argument source. Evaluated
    /// (Phase 4, only via `MacroExpand` bindings) into an
    /// `APLValue::NonBoundFn`, which ignores call arguments (Kotlin
    /// `DeclaredNonBoundFunction`: "ignores its arguments" — the body runs in
    /// the caller's context, so ambient `⍵`/`⍺` show through).
    NonBoundFn { body: Box<Instr> },
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
    /// A *closed-scope wrapper* around a `∇`-defined function/operator body. The parser
    /// never emits this: the evaluator wraps the body at `UserFnDef`/`UserOpDef` time
    /// (Kotlin parses every `∇` body in a `closed=true` env, parser.kt:757/771/783).
    /// `apply_user_fn`/`apply_user_op` set the call frame's `is_call_frame` barrier on
    /// sight of this wrapper, so a bare `←` inside binds in the frame (function-local)
    /// instead of leaking into caller scope or the namespace table — while `⇐`
    /// lambdas and bare blocks (never wrapped) keep write-through to defining scope
    /// (`updateableConstValue`, closure counters, `{ a +← ⍵ }` under lock). The wrapper
    /// travels WITH the body value, so recursion, `⍞`-dispatch, and method calls all
    /// preserve closedness with no signature threading.
    ClosedScope(Box<Instr>),
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
    /// A *computed* dynamic function reference (Kap's `⍞(expr)` /
    /// `parseApplyDefinition` with OpenParen, parser.kt:1194-1201): evaluates
    /// `expr` to a *value*, which must be a function, and applies it. Unlike a
    /// bare application (a value), this is FUNCTION-shaped (Kotlin wraps it in
    /// a `DynamicFunctionDescriptor`), so `a ⇐ ⍞(foo 1)` binds the computed
    /// function instead of building an `⍺/⍵` delegation.
    DynamicRefExpr { expr: Box<Instr> },
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
    /// `right_value` is Kotlin's `ValueCall` (op.kt:193-207): when the token after a
    /// 2-arg operator is NOT a function (e.g. the `20` in `(×foo 20)`), it is a VALUE
    /// operand, evaluated once in the caller env and bound to `op_right` directly
    /// (so `typeof(y)` → `kap:integer`). Mutually exclusive with `right_fn`;
    /// `None`/`None` = missing (1-arg op, or the B1 error shape).
    OpCall {
        op: Box<Instr>,
        left_fn: Box<Instr>,
        right_fn: Option<Box<Instr>>,
        right_value: Option<Box<Instr>>,
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
    /// `Over` operator `f ⍥ g` (Kotlin `OverOp` / `OverDerivedFunction`,
    /// builtins/operator.kt:383). A 2-arg operator producing a derived function:
    ///   monadic `(f⍥g) y`   = `f(g(y))`
    ///   dyadic  `x (f⍥g) y` = `f(g(x), g(y))`
    /// `⍥` is NOT compose (`∘`): compare oracle `3 +⍥× 4` = 2 vs `3 +∘× 4` = 4.
    OverOp { left_fn: Box<Instr>, right_fn: Box<Instr> },
    /// `Obverse` operator `f ⍫ g` (Kotlin `ObverseOp` / `ObverseFunction`,
    /// op.kt:304, engine.kt:503). A 2-arg operator producing a derived function:
    ///   forward  `(f⍫g) y`   = `f(y)`;   `x (f⍫g) y` = `x f y`
    ///   inverse  `(f⍫g)˝ y`  = `g(y)`;   `x (f⍫g)˝ y` = `x g y`
    /// (ObverseFunctionImpl: eval1Arg/2Arg → fn0, evalInverse1Arg/2ArgB → fn1.)
    Obverse { left_fn: Box<Instr>, right_fn: Box<Instr> },
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
    /// The nil singleton `⦻` (Kotlin `NilToken` → `EmptyValueMarker` →
    /// `APLNilValue`; renders `null`). Distinct from `Empty` (the `()`
    /// group / missing-element marker → `APLValue::Null`): `⦻` is an
    /// arithmetic identity (`⦻ 3 × 4 ⦻` → `(4 3)`).
    Nil,
    /// Indexed assignment: `array[index] ← value` (Kotlin `processAssignment`
    /// with an `ArrayIndex` dest — `deriveLvalueReader()` on an index produces
    /// an lvalue reader that updates the array at the selected positions).
    IndexAssign { array: Box<Instr>, selector: Box<Instr>, value: Box<Instr> },
    /// (a scalar or vector of integers) parsed from `[...]`. Semantics (Kap `PickAPLFunction`
    /// / `PickResultValue`): 1-D, each index `i` is adjusted with negative-from-end support
    /// (`checkAndAdjustSelectedIndex`); a 1-element selection is disclosed to a scalar, a
    /// multi-element selection is a vector of the picked elements. Binds tightly to the
    /// preceding primary (postfix), so `a b (c d)[0] e` indexes `(c d)`, not the whole strand.
    Index { array: Box<Instr>, selector: Box<Instr> },
    /// Member dereference: `object.member` (Kotlin `MemberDereferenceToken` →
    /// `MemberDereferenceInstruction` / `MemberDereferenceNameArgumentInstruction`).
    /// `member` is either a bare symbol name (`object.name`) or a parenthesised value
    /// expression (`object.(expr)`). Parsed as a postfix suffix on a primary, mirroring
    /// index access `object[sel]`.
    /// `value_form` distinguishes the two: `false` for name-form (the member IS a
    /// symbol literal used as a column label / map key), `true` for value-form
    /// (`(expr)` is EVALUATED first and the result is used as the key/index).
    /// Without this flag the evaluator cannot tell `a.col1` (name-form: lookup column
    /// "col1") from `a.(col1)` (value-form: evaluate the variable `col1` first).
    MemberDeref { object: Box<Instr>, member: Box<Instr>, value_form: bool },
    /// Method call: `object⍠name` (Kotlin `MethodCallToken` → `processMethodCall`,
    /// parser.kt:923 + `MethodCallFunction`, `method-calls.kt`). The parser pops the
    /// last left-arg as `object` and reads the next symbol as `method`; the node is
    /// function-shaped (a `MethodCallFunction` descriptor) and applies monadically
    /// to a right arg: `a⍠valuePlusN 200`. A left arg is a Kotlin
    /// `Unimplemented2ArgException` ("Function cannot be called with two
    /// arguments"). `method_namespace` is `None` for a bare name (which formats as
    /// `default:name` in `Method not found` errors).
    MethodCall { object: Box<Instr>, method: String, method_namespace: Option<String> },
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
    /// corresponding element of the (vector) RHS (Kotlin
    /// `DestructureAssignInstruction` + `deriveLvalueReader`, instr.kt:82/276/518).
    /// `target` mirrors the LHS surface shape, nested to any depth: a
    /// space-stranded group is `Array`, a `;`-separated group is `List`, leaves
    /// are `Symbol` (e.g. `((a b) c)` → `Array[Array[a,b],c]`). A bare-symbol
    /// target never reaches this variant (it is a plain `Assign`).
    DestructAssign {
        target: Box<Instr>,
        value: Box<Instr>,
    },
    /// Destructuring *modified* assignment `(a b) op← expr` — apply `op` to the
    /// current values, then bind back elementwise (oracle: `(bar foo) +← 3`
    /// with bar=1,foo=8 yields `⟨4 11⟩` and updates both names).
    DestructModifiedAssign {
        names: Vec<(String, Option<String>)>,
        op: Box<Instr>,
        value: Box<Instr>,
    },
    /// Dynamic assignment `b dynamicequal expr` (Kotlin `DynamicAssignmentInstruction`,
    /// dynamic-assign.kt): binds `name` to a thunk re-evaluating `body` on every read.
    /// A plain `←` overwrites the slot (dropping reactivity); re-entering the same
    /// thunk while forcing errors `Circular dynamic assignment` (common.kt:156).
    /// Only single-symbol targets are legal (Kotlin `processDynamicAssignment`).
    DynAssign {
        name: String,
        namespace: Option<String>,
        body: Box<Instr>,
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
/// `:constant` (a literal name that must match verbatim, binding nothing),
/// `:function`/`:nfunction`/`:nexprfunction` (a `{…}` block parsed as a no-param lambda),
/// `:value` (a `(…)` parenthesised expression), `:string` (a string literal),
/// `:special :openBrace|:closeBrace|:newline` (a literal token), `:optional (…)`
/// (try the inner rules), and `:repeat (name subName)` (repeat a sub-macro while it matches).
#[derive(Debug, Clone)]
pub enum SyntaxRule {
    /// A literal name that must match verbatim (Kotlin `ConstantSyntaxRule`):
    /// binds NOTHING. Mismatch errors `In custom syntax rule: Expected: <want>.
    /// Found: <got>` (Kotlin `SyntaxRuleMismatch`, common.kt:175).
    Constant { name: String },
    /// A `{…}` function block → bound to `var` as a no-param `Instr::Lambda`.
    Function { var: String },
    /// Same shape, but the body becomes an `Instr::NonBoundFn` (Kotlin
    /// `DeclaredNonBoundFunction`): invocation ignores arguments and runs the
    /// body in the caller's context. NOT identical to `Function`.
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
    /// Namespace the macro was defined in (explicit trigger ns, else the
    /// current namespace at `defsyntax` eval). Gates cross-namespace use:
    /// same-ns always visible; elsewhere needs export + import.
    pub namespace: String,
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
