# PROGRESS 2026-08-26 (g) — structural-under OVERLAY family (`↑`/`↓` wrappers)

## [2026-08-26] `⍢` structural-under: the overlay rule, not just the inverse rule

Continuation of the roadmap. Target was the sole remaining `output3.kap` blocker
left by `202aae5`: `error: ˝: inverse not supported for this function` from
`↓⍢(10↓)`.

### Root cause — the port implemented ONE rule where Kotlin has TWO

`evalWithStructuralUnder1Arg` is a **per-function override**, not a single generic
algorithm. `functions.kt:241` is only the *unsupported default*
(`StructuralUnderNotSupported`, common.kt:155). Grepping the overrides shows two
distinct families:

1. **Inverse-based** — math functions, `⍉`, `⍨`: they call
   `inversibleStructuralUnder1Arg` (functions.kt:267) =
   `wrapper⁻¹(base(wrapper(a)))`. Call sites: `math_functions.kt:645/731/888/1208/1507/1918`,
   `transpose.kt:251/555`, `commute.kt:36`.
2. **Overlay-based** — `↑` (drop.kt:84), `↓` (drop.kt:351), pick (lookup.kt:232):
   the wrapper **SELECTS a region**, base transforms it, and the result is written
   **back into the original** via `replaceForUnder` (drop.kt:174/:260) →
   `OverlayReplacementValue` (array_functions.kt:126).

The port had only family 1 (a generic `˝` inverse for every wrapper), so every
take/drop wrapper failed. **This was a semantic misconception, not a missing case:**
structural under is fundamentally *region replacement*, and the inverse rule is
just the special case that happens to work for self-inverse wrappers.

Operand order confirmed at `operator.kt:387-401`: `combineFunction(fn0, fn1)` ⇒
`baseFn=fns[0]`, `wrapperFn=fns[1]`, and `eval1Arg` delegates to
`wrapperFn.evalWithStructuralUnder1Arg(baseFn, …)`.

`OverlayReplacementValue` mechanics (array_functions.kt):
- result shape per axis (`:158`) = `src[i] - src_replacement[i] + replacement[i]`
  — so a base fn that resizes the selected region **resizes that axis**;
- `valueAt` (`:204`) reads the replacement inside the offset window, else `src`
  with the outside coordinate shifted by the per-axis size delta;
- `isWithinReplacement` (`:222`) is the window test;
- a **scalar** replacement against a non-scalar source is resized to fill the
  region (`:143`).

Offset derivation (`replaceForUnder`): for **take** (drop.kt:174) a positive count
selects from the front (offset 0), a negative count selects the tail
(offset = `n + count`); for **drop** (drop.kt:260) it is inverted — a positive
drop leaves the tail (offset = count).

### Fixes — `kap-core/src/evaluator.rs`

- **New `overlay_replacement`** (~:7859) — faithful port of
  `OverlayReplacementValue`: rank check with Kotlin's exact message
  ("Replacement value must have the same rank as the original data. Got rank=…,
  original=…"), the `:158` result shape, the `:222` window test, the `:210`
  outside-coordinate shift, and the `:143` scalar resize. Odometer walk over the
  result shape; 100M element guard **before** allocation (per the skill's
  OOM-guard rule).
- **New `under_take_drop_spec`** (~:8061) — recognises a take/drop wrapper,
  including the `Train[value, fn]` left-bind shape that `(10↓)` / `(¯1↑)` parse
  into, and returns `(is_take, counts)`. Bare monadic `↓` qualifies (drops 1 along
  the leading axis); bare monadic `↑` does **not** (it is "first", not the overlay
  form).
- **`apply_under_op`** (~:7986) — now routes take/drop wrappers through the
  overlay path and everything else through the existing inverse path, with the
  two families documented at the call site.
- **`⍬` handling** — `⍬` is `APLValue::Null` in the port but a **rank-1 length-0
  array** in Kotlin, so `value_dims` reported rank 0 and the scalar-resize branch
  spliced a literal `⍬` into every cell. Now treated as the empty array of the
  source's rank, so an empty replacement correctly shrinks the axis. Caught only
  because I probed `{⍬}⍢(1↑)` against the oracle — the first three probes all
  passed and would have hidden it.

### Verification (port vs oracle, side-by-side)

| expr | port | oracle | ok |
|---|---|---|---|
| `↓⍢(10↓) ⍳20` | `(0…9 11…19)` | `⟨0…9 11…19⟩` | ✓ |
| `⌽⍢(3↑) ⍳10` | `(2 1 0 3 4 5 6 7 8 9)` | same | ✓ |
| `{×2}⍢(2↑) ⍳6` | `(1 1 2 3 4 5)` | same | ✓ |
| `{100}⍢(2↑) ⍳6` | `(100 100 2 3 4 5)` | same | ✓ |
| `{⍵}⍢(2↑) ⍳6` | `(0 1 2 3 4 5)` | same | ✓ |
| `{1000+⍵}⍢(3↓) ⍳6` | `(0 1 2 1003 1004 1005)` | same | ✓ |
| `{99}⍢(¯2↓) ⍳6` | `(99 99 99 99 4 5)` | same | ✓ |
| `{⍵}⍢(¯2↑) ⍳6` | `(0 1 2 3 4 5)` | same | ✓ |
| `{,1}⍢(¯1↑) 1 0 0 1` | `(1 0 0 1)` | same | ✓ |
| `{,1 2}⍢(1↑) ⍳4` | `(1 2 1 2 3)` | same (axis grew) | ✓ |
| `{⍬}⍢(1↑) ⍳4` | `(1 2 3)` | `⟨1 2 3⟩` | ✓ |
| `{⍬}⍢(2↑) ⍳4` | `(2 3)` | `⟨2 3⟩` | ✓ |
| `{⍬}⍢(2↓) ⍳4` | `(0 1)` | `⟨0 1⟩` | ✓ |
| `↓⍢(2↓) 2 3⍴⍳6` | `((0 1 2) (3 4 5))` | 2×3 box, same values | ✓ |
| `⌽⍢(2↑) 2 3⍴⍳6` | `((2 1 0) (5 4 3))` | 2×3 box, same values | ✓ |
| `⌽⍢⌽ ⍳5` (inverse family) | `(4 3 2 1 0)` | same | ✓ |
| `{⍵+1} ⌽⍢⌽ ⍳5` | `(5 4 3 2 1)` | same | ✓ |

Rank-2 oracle rows needed multi-line capture (`sed -n '/⊢/,$p'`) — the oracle
box-draws rank≥2 (`┌→────┐`), so a `head -1` grep shows only the top border and
looks like a mismatch. Not a divergence.

### Regression rows added — `kap-core/tests/conformance.rs` (~:664)

17 new curated rows covering the overlay family (positive/negative counts,
take/drop, region resize, empty-replacement shrink, rank-2), the hex-literal
normalisation from `202aae5`, and the `⍢`-on-assignment-RHS / chained-atom parse
fixes. All pass.

### Gates (re-run, all GREEN)

- `cargo test -p kap-core --lib` → **96 passed; 0 failed**
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed; 0 failed**
- `cargo build -p kap-cli` clean (1 pre-existing warning).

### stdlib load state

| file | before this entry | after |
|---|---|---|
| `output3.kap` | 90/122 `˝: inverse not supported for this function` | 89/122 `parse: parse error at 118:37: expected a function in train` |
| `util.kap` / `stat.kap` / `structure.kap` / `io.kap` / `standard-lib.kap` | CLEAN | CLEAN |
| `map.kap` | 18/24 parse error 13:6 | unchanged (own mini-phase, ROADMAP §10) |
| `math.kap` | 1/9 `math:pi` | unchanged — oracle-consistent, deliberately not "fixed" |

The `⍢` error class is now **fully gone** from output3; the blocker has moved to a
plain parse error at line 118 col 37.

### Remaining / next

- **`output3.kap:118:37` `expected a function in train`** — new and unexamined;
  the natural next item.
- **Dyadic structural under** (`a base⍢wrapper b`) still returns the unsupported
  error. Kotlin has `evalWithStructuralUnder2Arg` +
  `inversibleStructuralUnder2Arg` (functions.kt:273, using `evalInverse2ArgB`) and
  overlay 2-arg variants (drop.kt:96/:360). Not needed by output3; unscoped.
- **Pick-based under** (lookup.kt:232 `replaceForUnder`) — the third overlay
  wrapper, not yet wired. `overlay_replacement` is reusable for it.
- Carried over: `typeof "hi"` → port `kap:string` vs oracle `kap:array`
  (pre-existing string-representation gap, see PROGRESS-20260826f).

### Process notes

- **`patch` mis-anchored into a neighbouring doc comment.** My large
  `apply_under_op` replacement spliced the three new functions into the MIDDLE of
  `apply_power_op`'s doc comment (truncating it after "evaluate the operand to a"
  and orphaning the rest ~250 lines later). It still *compiled* once repaired, but
  the tell was `grep -n 'fn apply_power_op'` showing the comment fragment
  detached from its `fn`. Repaired by rebuilding both seams. **Lesson: after a
  large `patch` whose `old_string` began with a `///` line, grep for the
  neighbouring `fn` names to confirm the anchor landed where intended — the
  rustfmt-noise diff is too large to eyeball.**
- Oracle rank≥2 output is box-drawn; use `sed -n '/⊢/,$p'` not `head -1`.
