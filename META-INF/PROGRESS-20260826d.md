# PROGRESS-20260826d — `⍢` non-self-inverse under + 2-train formation (P7 util.kap chain)

## Summary
Two distinct stdlib-chain blockers fixed on the `feature/wheres-extra` branch
(after `b088030`). Both gate green: `cargo test -p kap-core --lib` = **96/0**,
`cargo test -p kap-core --test conformance curated_kap_parity` = **1/0**.

### Fix 1 — `⍢` derived fn stored via `⇐` applied dyadically (evaluator.rs)
`FnAssign` and `apply_user_fn` only unwrapped `Derived`/`Train`/`Symbol` bodies
via `eval_apply`; a `ValueOp` (`⍢`) body fell into the `_` arm and was evaluated
as a *dyadic delegation* `⍺ <rhs> ⍵` (split=1). Calling it monadically bound `⍺`
as `left`, so `apply_under_op` saw `left.is_some()` and errored
`under not supported for function`.

- evaluator.rs:937 `FnAssign` — add `Instr::ValueOp { .. }` to the split=0
  direct-store arm (stores the derived fn verbatim, applied at call time).
- evaluator.rs:2962 `apply_user_fn` body unwrap — add `Instr::ValueOp { .. }`.

Oracle-verified: `trimRight ⇐ trimLeft⍢⌽ ⋄ trimRight "  ab c  "` → `"  ab c"` ✓
(oracle-exact). Direct `(trimLeft⍢⌽) "  ab c  "` already worked (self-inverse
wrapper path); the bug was only on assign-then-call.

### Fix 2 — `f g` 2-train (atop) not formed for user-defined fns (parser.rs)
Kotlin `processFn` parser.kt:479-484: when the right arg of an ambivalent fn
parses as `FnParseResult` (a FUNCTION), it forms `Chain2(fn, rightFn)` — a
2-train ATOP `(f g) x = f(g(x))` (instr.kt:588) — NOT `Apply{f, right}`.

Two port gaps both relied on `Self::is_primitive_op(name)`, which excludes
user-defined (`⇐`) functions, so `trim ⇐ trimRight trimLeft` (and `h ⇐ f g`
with `f ⇐ +`, `g ⇐ -`) built `Apply{trimRight, trimLeft}` → evaluated `trimLeft`
with no `⍵` → `undefined symbol: ⍺`.

- New helper `is_fn_atom_name(name, namespace)` = primitive-OR `is_known_fn`
  (and `:keyword` excluded).
- parser.rs:3645 (⍛ fall-through 2-train guard) and parser.rs:3786 (trailing-fn
  2-train guard) now use `is_fn_atom_name` instead of `is_primitive_op`.
- parser.rs:1061 (`finish_fn_call` left-args-empty arm): when the right arg is a
  function (`nested_right_is_fn_result`), form `Train{funcs:[fn, right]}` (2-train
  atop) — mirrors the Kotlin FnParseResult branch. Value right-args still apply
  normally (no train). NOTE: this arm was first prototyped with `is_function_expr`
  which is too broad (treats any bare Symbol — incl. value vars — as a fn and
  regressed `⍴⎕A` + the curated harness); switched to `nested_right_is_fn_result`
  (the stricter Symbol-as-function classifier).

Oracle-verified:
- `trim ⇐ trimRight trimLeft ⋄ trim "  ab c  "` → `"ab c"` ✓
- `f ⇐ + ⋄ g ⇐ - ⋄ h ⇐ f g ⋄ h 5` → `¯5` (=-(+5)) ✓ (atop)
- `3 h 5` → `¯2` (3 -(+5)) ✓
- `3 < 5` (value-not-train) still works; `foo x` (value var right arg) still a
  normal Apply (no regression).

## util.kap (P7 chain) status
`use("util.kap")` loads with **no util-specific `use()` warning** (previously
`1/12` `≠ requires numbers`, then `under not supported for function`).
`s:trimLeft`/`s:trimRight`/`s:trim` all oracle-exact via the standard-lib auto
load. Remaining `use()` warnings are all ROADMAP §10 out-of-scope:
`output3.kap`, `map.kap`, `fhelp.kap`, `http.kap`, `math.kap` (`math:pi` const),
`standard-lib.kap` (`int:libInitialised`).

## Side-by-side matrix (R = port, O = kap-jvm-text oracle; values equal, only
REPL glyphs differ: `¯`/`-`, `()`/`⟨⟩`)
| expr | R | O |
|---|---|---|
| `(-⍢-) 3` | `¯3` | `-3` |
| `(⌽⍢⌽) 1 2 3` | `(3 2 1)` | `⟨3 2 1⟩` |
| `(⌽⍢-) 3 4 5` | `(5 4 3)` | `⟨5 4 3⟩` |
| `f⇐+ ⋄ g⇐- ⋄ h⇐f g ⋄ h 5` | `¯5` | `-5` |
| `f⇐+ ⋄ g⇐- ⋄ h⇐f g ⋄ 3 h 5` | `¯2` | `-2` |
| `trim ⇐ trimRight trimLeft ⋄ trim "  ab c  "` | `"ab c"` | `"ab c"` |

## Files changed
- kap-core/src/evaluator.rs (FnAssign ValueOp split=0; apply_user_fn ValueOp unwrap)
- kap-core/src/parser.rs (is_fn_atom_name helper; 2-train guards; finish_fn_call atop)
