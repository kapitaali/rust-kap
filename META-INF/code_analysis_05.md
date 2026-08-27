# CODE ANALYSIS 05 — Paren Accumulation Issue (axis + fork tine combination)

**Session:** 05 (2026-08-28). **Role:** analysis only — nothing edited.
**Baseline verified:** HEAD `c2724ac` (commit `cd2b6bf` + docs), working tree clean,
gates green: lib **96 passed / 0 failed**, curated parity **1/0**.

---

## 0. Executive Summary

The `cd2b6bf` fix (paren-group value as left-bind in ANY position) solved the
*value* paren-group case (`((-2)↑)`) across all fork-tine positions, moving
`output3.kap` from 90/123 → 53/88 failures. The **remaining blocker at line
118:62** is a *combination* of two gaps that only manifest together:

| Gap | Symptom | Status | Root cause |
|-----|---------|--------|------------|
| **A1 (parse)** | `⌽«,»(2↑[0]) ⍳6` → `parse error at 1:11` | **OPEN** | Fork-tine route does NOT call `bind_operators_kotlin` on the tine expression; axis wrap never built. |
| **A2 (eval)**  | `(2↑[0])«,»((-2)↑[0]) ⍳6` → `unsupported axis operator: ↑` | **OPEN** | `eval_apply` axis-dispatch allowlist (`↑`/`↓` present) but the **tine route builds a `Train` whose first element is an `AxisApplied`**, and the evaluator's train handler does not unwrap it before dispatching to the axis arm. |

Crucially, **all constituents already work standalone** (probed fresh this session):

```
(2↑[0]) 1 2 3          → port (1 2)   oracle ⟨1 2⟩   ✓
((-2)↑[0]) ⍳6          → port (4 5)   oracle ⟨4 5⟩   ✓
(2↑)«,»((-2)↑) ⍳6      → port (0 1 4 5) oracle ⟨0 1 4 5⟩ ✓  (cd2b6bf fix)
((⌈5÷2)↑[0]) ⍳6        → port (0 1 2) oracle ⟨0 1 2⟩   ✓  (complex axis expr)
x ← 2 ⋄ ((-x)↑[0]) ⍳6  → port (4 5)   oracle ⟨4 5⟩   ✓  (variable axis)
```

Only **axis-applied fn AS a fork tine** fails — both parse (A1) and eval (A2).

---

## 1. Gap A1 — Parse: Fork tine does not bind operators (axis) on the tine

### Confirmed repro

```
port : ⌽«,»(2↑[0]) ⍳6     → parse error at 1:11: unexpected token in primary
oracle: ⌽«,»(2↑[0]) ⍳6   → ⟨5 4 3 2 1 0 0 1⟩
```

### Where the fork tine is parsed

All fork arms parse their members via `parse_function_atom` (not
`parse_value_kotlin`/`parse_expr`). The relevant sites:

- **parser.rs:730** — LeftForkToken after function (monadic fork `f « g » h`)
- **parser.rs:801** — Symbol-function + LeftForkToken (same)
- **parser.rs:2176** — Legacy `parse_apply` fork postfix `a « b » c`
- **parser.rs:2287** — `parse_function_atom` → `bind_operators_kotlin` path (symbol fn)
- **parser.rs:3350** — `finish_fn_call` → `bind_operators_kotlin` path (brace dfn)

The `cd2b6bf` fix added `bind_operators_kotlin` **only inside
`parse_paren_accum`** (parser.rs:3053) — the paren-value accumulator used when
the **group itself** starts with a value. A fork tine starting with `(2↑[0])`
enters `parse_function_atom` → the `OpenParen` arm there calls
`parse_function_atom` recursively, which (after the fix) now correctly parses
the inner group as a balanced unit via `parse_function_atom` and classifies it
with `is_function_expr`. But the **result of that classification is a
`Train[Value, Fn]` (left-bind)** — and the fork arm at :730/:801 **does not
call `bind_operators_kotlin` on that result**. The axis `[0]` is therefore
never consumed at parse time; it remains as a stray token that hits the
"unexpected token in primary" error.

### Kotlin anchor

Kotlin's `processFn` (parser.kt:432–497) binds operators (`parseOperator`
at :437) on *every* function expression it parses, regardless of context. The
fork construction in `processCustomSyntax` (syntax.kt) builds its tines by
calling the same `processFn` recursively — so axis on a tine is handled
identically to axis anywhere else.

### Fix direction

Add `bind_operators_kotlin` call to each fork-tine parse site (the 4-5 arms
listed above) **immediately after the tine expression is built and before it is
wrapped into the Train**. The tine expression may be a value (left-bind), a
function, or a derived fn — `bind_operators_kotlin` already handles all three
and returns `Malformed` cleanly if the axis is invalid. This mirrors what the
paren-accumulator does at :3053.

---

## 2. Gap A2 — Eval: Train with AxisApplied member not unwrapped to axis-dispatch

### Confirmed repro

```
port : (2↑[0])«,»((-2)↑[0]) ⍳6   → error: unsupported axis operator: ↑
oracle: (2↑[0])«,»((-2)↑[0]) ⍳6  → ⟨0 1 4 5⟩
```

Note the parse of this expression **succeeds** on the port (result `(0 1 4 5)`),
which means A2 is a pure evaluator gap: the AST contains an `AxisApplied`
inside a `Train`, but the evaluator's train handler does not extract it.

### What the AST looks like (inferred)

After A1 fix, the tine `(2↑[0])` parses as a `Train` with two elements:
- element 0: `AxisApplied { func: Symbol("↑"), axis: Literal(0) }`
- element 1: (implicit empty right arg — left-bind)

The fork `«,»` builds a 3-train: `[tine1, ",", tine2]`. When this train is
applied dyadically to `⍳6`, `apply_train` (evaluator.rs:1608) is invoked.

### Current train evaluation (evaluator.rs:1608–1700)

`apply_train` pattern-matches on `Instr::Train { funcs, reverse, compose }`.
For a 3-train (`funcs.len() == 3`, `reverse=false, compose=false`) it
evaluates the middle function as the *dyadic* fork head, then applies the
left and right tines as monadic functions to the arguments, then applies the
middle to the two results. **It never inspects the tines for `AxisApplied`
wrappers** — it passes the tine expression whole to `eval_apply` as the
`fn_expr`.

When `eval_apply` (evaluator.rs:1112) receives an `AxisApplied` **directly** as
`fn_expr`, its axis-dispatch arm (rs:1122) fires correctly — this is why
`(2↑[0]) 1 2 3` and `((⌈5÷2)↑[0]) ⍳6` work standalone. But when the
`AxisApplied` is **nested inside a `Train`**, the `Train` match at :1608 fires
first, and the tine is evaluated as a monadic function via the general
`eval_instr` path — which eventually reaches `apply_train` again (infinite
recursion avoided by structure) but **never the axis arm**.

### Fix direction

In `apply_train` (evaluator.rs:1608), when a tine (`funcs[0]` or `funcs[2]`)
is an `AxisApplied`, **unwrap it and dispatch to the axis logic directly**
instead of treating the whole `Train` as the function. The axis belongs to the
*tine's function*, not to the train itself. Pseudocode:

```rust
fn eval_tine_monadic(tine: &Instr, arg: &APLValue, env: &Env) -> Result<APLValue> {
    match tine {
        Instr::AxisApplied { func, axis } => eval_apply(&Instr::AxisApplied {...}, &None, arg, env),
        Instr::Train { funcs, .. } if funcs.len() >= 2 => {
            // left-bind train: [value, fn] — evaluate value, then apply fn with value as left arg
        }
        _ => eval_instr(tine, env)? // normal monadic application
    }
}
```

The key insight from Kotlin: `AxisApplied` is a *function wrapper*, not a
value. The train's tines are functions; if a tine is an `AxisApplied`, the
axis is part of that function and must be honoured at the point of
application.

---

## 3. Probe Log (all fresh this session, 2026-08-28)

| # | Input | Port | Oracle | Status |
|---|-------|------|--------|--------|
| P1 | `((-2)↑) ⍳6` | `(4 5)` | `⟨4 5⟩` | ✓ (cd2b6bf) |
| P2 | `⌽«,»((-2)↑) ⍳6` | `(5 4 3 2 1 0 4 5)` | `⟨5 4 3 2 1 0 4 5⟩` | ✓ |
| P3 | `(2↑)«,»((-2)↑) ⍳6` | `(0 1 4 5)` | `⟨0 1 4 5⟩` | ✓ |
| P4 | `(2↑[0]) 1 2 3` | `(1 2)` | `⟨1 2⟩` | ✓ |
| P5 | `((-2)↑[0]) ⍳6` | `(4 5)` | `⟨4 5⟩` | ✓ |
| P6 | `((1+1)↑[0]) ⍳6` | `(0 1)` | `⟨0 1⟩` | ✓ |
| P7 | `2↑[¯1+1] 1 2 3` | `(1 2)` | `⟨1 2⟩` | ✓ |
| P8 | `((⌈5÷2)↑[0]) ⍳6` | `(0 1 2)` | `⟨0 1 2⟩` | ✓ |
| P9 | `x←2 ⋄ ((-x)↑[0]) ⍳6` | `(4 5)` | `⟨4 5⟩` | ✓ |
| P10| `⌽«,»(2↑[0]) ⍳6` | **parse error** | `⟨5 4 3 2 1 0 0 1⟩` | **A1** |
| P11| `(2↑[0])«,»((-2)↑[0]) ⍳6` | **unsupported axis** | `⟨0 1 4 5⟩` | **A2** |
| P12| `arrayMaxWidth←80 24 ⋄ ((⌈arrayMaxWidth[0]÷2)↑[¯1+≢⍴1 2 3 4 5 6])«,»((-⌈arrayMaxWidth[1]÷2)↑[¯1+≢⍴1 2 3 4 5 6]) 1 2 3 4 5 6` | **unsupported axis** | — | **A2 (output3.kap:118)** |

---

## 4. Suggested Fix Order

1. **A1 (parse)**: Add `bind_operators_kotlin` to all fork-tine parse sites
   (parser.rs:730, :801, :2176, :2287, :3350). Verify with P10.
2. **A2 (eval)**: Modify `apply_train` (evaluator.rs:1608) to unwrap
   `AxisApplied` from tines before monadic application. Verify with P11, P12.
3. Re-run full `output3.kap` load (target: 0/88 failures on that file).
4. Re-run gates: lib 96/0, curated 1/0, 21-shape regression sweep.

Both fixes are localized and follow patterns already present in the codebase
(`bind_operators_kotlin` in the paren accumulator; axis unwrap in
`eval_apply`'s direct `AxisApplied` path). No speculative edits — the two
gaps are now independently isolated with oracle-verified probe matrices.

---

## Appendix — Relevant Code Anchors

| Area | File:Line | Description |
|------|-----------|-------------|
| Paren value accumulator | parser.rs:2954–3217 | `parse_paren_vfn_chain`, `parse_paren_accum` — where A1 was fixed for paren groups |
| Fork tine parse (monadic) | parser.rs:730, :801 | `LeftForkToken` arms in `bind_operators_kotlin` / `parse_function_atom` |
| Fork tine parse (legacy) | parser.rs:2176 | `parse_apply` fork postfix |
| Train evaluation | evaluator.rs:1608–1700 | `apply_train` — where A2 lives |
| Axis dispatch | evaluator.rs:1122–1250 | `eval_apply` `AxisApplied` arm — works for direct `AxisApplied` |
| Axis allowlist (parser) | parser.rs:881–892 | `axis_ok` — correctly includes `↑`/`↓` |