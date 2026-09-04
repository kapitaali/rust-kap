# NON-IMPLEMENTED.md — explicit "did NOT implement X" decisions

Per ROADMAP §0.1 law 4: "no leniency creep". When a port task surfaces
a feature whose proper implementation would touch code outside its
scope, the proper thing is to STOP and document why — not to expand
the diff.

This file records every decision in T1.1 (and adjacent) to LEAVE A FEATURE
UNIMPLEMENTED with full reasoning, so future sessions don't re-litigate
it from scratch or silently pick it up.

Each entry:
- **Feature**: what was NOT added
- **Why deferred**: the immediate blocker
- **Blast radius**: what would change if I added it
- **Status quo**: what the user sees today (without the feature)
- **Unblock path**: minimum work to ship it safely

---

## 1. ⊃[axis] (DisclosedArrayValue + TransposedAPLValue) — 11 cases

**Feature**: monadic `⊃[k] x` and dyadic `x ⊃[k] y` — disclose-along-axis.

**Why deferred**:
- The existing port's `⊃` (reveal/disclose) uses a single nested-loop
  flatten. Kotlin's `⊃[k]` does NOT flatten — it returns a rank-1
  reduction (rank(x) - 1) with a TRANSPOSED layout: for each non-axis
  position, it picks one element from the axis-`k` dim, controlled by
  the index array.
- This requires a brand-new `DisclosedArrayValue` variant in
  `kap-core/src/value.rs`, plus a `TransposedAPLValue` for the perm-
  axis layout, plus plumbing through `valueAtLinear`, `dimensions`,
  `rank`, `iterator`, `equals`, `hash`, `display`, `clone` (≥ 8 trait
  impls).
- The first attempt (a `disclose_axis` helper) was written and then
  REMOVED because the rank-1 layout vs the oracle's rank+1 layout
  produced a different shape — silent wrong-shape bug.

**Blast radius**:
- New value type touches: evaluator dispatch, value trait impls,
  display formatter, `valueAt`/`get*` paths, `dimensions` propagation,
  ResizedArray/reshape interop, type-specialised fast paths.
- Estimated: 200-400 lines + new test cases. Half-day minimum.

**Status quo**: parser accepts `⊃[k]` (in the `0c09c80` allowlist
extension), evaluator dispatches to the catchall arm and emits
`Error: ⊃[axis]: axis specifier not supported` (i.e. graceful fail,
not a panic).

**Unblock path**:
1. Read `DisclosedArrayValue` and `TransposedArrayValue` in
   `array/src/commonMain/kotlin/com/dhsdevelopments/kap/values/`
   and `array/src/commonMain/kotlin/com/dhsdevelopments/kap/util/transpose.kt`
2. Decide whether to add a new value variant or represent disclosed
   as `APLArray { data, dims }` with a special "disclosed" flag.
3. Implement, with parity tests from `conformance/disclose*`.

**Conformance cases deferred**: 11 (8 monadic, 3 dyadic).

---

## 2. +[axis] monadic arm — REVERTED

**Feature**: monadic `+[k] x` (numeric resolution along axis).

**Why deferred**:
- First-pass implementation was added to `AxisApplied` match.
- Discovered: Kotlin's `+[k] x` (monadic) uses `ResizedArrayImpls.
  makeResizedArray`, NOT the `computeTransformation` path used by
  dyadic. The semantics are "pad/truncate to a shape where the
  axis-`k` dim has length 1" (essentially `,1` in the axis-`k` slot).
- This is structurally different from the dyadic `x +[k] y` and
  from the `⊂[k]` enclose-axis path.
- The naive port produced wrong shapes. Arm was REVERTED before any
  commit landed.

**Blast radius**:
- `num2_axis` helper would need a "monadic" branch with different
  shape math (always produces rank-1 result along axis).
- Touches `ResizedArray` interop in evaluator.rs.
- Risk of regressing the dyadic path (which is fully working for
  21 cases).

**Status quo**: monadic `+[axis]` falls into the catchall arm and
emits "axis specifier not supported" (graceful fail). The 21 dyadic
`+[axis]` cases all pass.

**Unblock path**:
1. Read `MathCombineAPLFunction.eval1Arg` in
   `array/src/commonMain/kotlin/com/dhsdevelopments/kap/builtins/math_functions.kt`
2. Identify the monadic `+` semantics (NOT arithmetic; it's
   "identity extended" or similar — verify against oracle).
3. Implement monadic arm separately from dyadic.

**Conformance cases deferred**: ~1 (the single monadic `+[axis]`
case in `MathCombineAPLFunctionTest`).

---

## 3. +/[axis] adverb distinguish — NOT needed, left as-is

**Feature**: route `+/[axis]` (reduce-plus-along-axis) to a special arm
that distinguishes it from `a /[axis] b` (select-elements).

**Why deferred**:
- Initially feared these would collide in the parser.
- Verified: `+/[0] x` parses as a SINGLE verb token (`+/[0]`) via the
  adverb mechanism in `evaluator.rs:1900` (`adv_explicit_axis`). The
  adverb form keeps the existing adverb path. The new direct-verb
  `a /[axis] b` only fires when the LEFT argument is a normal APL
  value, not a verb.
- No change required.

**Blast radius**: zero (the existing parse path already handles this).

**Status quo**: both `+/[0] 1 2 3 4` (reduce-plus, adverb) and
`2 2 /[0] 2 3 ⍴ ⍳6` (select-elements, direct-verb) work correctly.

**Unblock path**: n/a — no work needed.

**Conformance cases deferred**: 0.

---

## 4. ∧[axis] and ∨[axis] sort-along-axis — 5 cases

**Feature**: `∧[k] x` (sort-up along axis k), `∨[k] x` (sort-down).

**Why deferred**:
- Kotlin's `AndAPLFunction.eval1Arg(a, axis)` calls
  `sortKapArray(a, axis, false, pos)` — i.e. `∧[k] x` is NOT
  boolean-and-along-axis, it is **sort-up along axis k**.
- The port has a `grade_up` helper (used by `⍋`) that returns
  INDICES, not a sorted array. Sort-along-axis needs a new helper
  that returns the array re-ordered along axis k.
- `∧` and `∨` are NOT in the parser allowlist (`0c09c80` extension
  did not include them — they weren't in my initial `Kotlin
  AxisApplied.allowed` list because I only checked the explicit
  "axis-applied" test cases, not the sort tests which reuse `∧`).

**Blast radius**:
- New `sort_axis(value, axis, descending)` helper — about 80-120
  lines (Kotlin's `sortKapArray` is ~60 lines + dispatch).
- New test cases: 5 (3 sort + 1 error + 1 labels).
- Does NOT affect any existing `grade_up` / `⍋` path.

**Status quo**: `∧[0] x` and `∨[0] x` parse-error out as
"reshape dimensions must be integers" because the parser strands
the `[0]` as a separate token (it's not in the allowlist, so the
bracket group is not consumed by AxisApplied and the rest of the
line breaks reshape parsing).

**Unblock path**:
1. Add `∧`, `∨` to parser allowlist (`parser.rs:1078`).
2. Implement `sort_axis` helper following Kotlin
   `array/src/commonMain/kotlin/com/dhsdevelopments/kap/builtins/sort.kt:340-371`.
3. Register in `is_primitive_name` AND the AxisApplied match
   (two-gate rule).

**Conformance cases deferred**: 5 (3 sort-up, 1 sort-down, 1
sort-with-invalid-axis error).

---

## 5. 138/138 perfect-string-match probe — REJECTED

**Feature**: a conformance probe that requires byte-identical output
between port and oracle.

**Why deferred**:
- I attempted this at end-of-session as a "what's the true failure
  count" sanity check.
- It reported 138/138 failures — but the failures were dominated by
  P8 display differences (`()` vs `⟨⟩`) which ROADMAP §11 Option A
  explicitly accepts as not-our-bug.
- The probe also reported "differences" on cases that the user
  confirmed working (`⊂[0] 2 3 2 ⍴ ⍳12` etc.).

**Blast radius**:
- A byte-identical probe is USELESS as a pass/fail signal in the
  P8 era. It would generate false positives for the entire 118
  closed cases and overwhelm the report.
- A proper probe needs to: (a) compare SHAPES first, (b) compare
  VALUES element-wise, (c) compare ERROR MESSAGE TEXT after
  normalising per `ERRORS.md` differences, (d) ignore
  parentheses-vs-angle-bracket display divergence.

**Status quo**: no such probe. The cluster-closure counts in
`PROGRESS-20260904d.md` (138 cases, ~118 closed) are based on
HAND-VVERIFIED OracleProbe comparisons per-verb, not a sweep.

**Unblock path**:
1. Build a shape+value-aware probe (≥ 200 lines of Python).
2. Run against the full 138-case cluster.
3. Manually classify the remaining ~20 as "T1.1 carryover" (⊃,
   ∧/∨) vs "unexpected regression" (would need a fix).
4. Do NOT use this probe to drive T1.1 sign-off without manual
   review — display-divergence noise is too high.

**Conformance cases deferred**: 0 (this is an instrumentation
choice, not a port choice).

---

## 6. T1.3 labels cluster (40 cases) — separate workstream

**Feature**: label-threading fixes for the 40 "labels" cases in
conformance that fall outside T1.1 (extracts, iota-as-reshape-dim,
axis-applied ordering at statement start).

**Why deferred**:
- T1.3 is a SEPARATE Tier 1 cluster per ROADMAP §A.3. The 40 cases
  in `LabelsThreadingTest`, `LabelsExtractionTest`, `IotaAsReshapeDim`,
  etc. were documented as pre-existing parser issues during the T1.1
  labels-threading work (commits `7b07cb6` → `df65df9`, 5 commits).
- They are NOT in the T1.1 bracket-axis cluster. They are labelled
  "labels cluster" in the progress notes and were explicitly
  DEFERRED to a separate T1.3 sweep.

**Blast radius**:
- T1.3 requires parser changes for `↑`, `↓` with label keys, plus
  new value-type fields for "key-axis" labels (Kotlin has a
  `LabelValueWithKey` variant).
- Estimated: 1-2 days of work.
- NOT in scope for T1.1.

**Status quo**: 40 cases fail with various errors (mostly
"type mismatch" or "reshape error"). The 21 `labels[axis]`
cases (subset of T1.1) DO pass.

**Unblock path**: open a new session for T1.3. Read `LabelValue`
and `KeyArrayValue` in
`array/src/commonMain/kotlin/com/dhsdevelopments/kap/values/`.
Plan the value-type migration. Don't reopen T1.1.

**Conformance cases deferred**: 40 (entire T1.3 cluster).

---

## 7. T2.1 reshape spec keywords (`:match`, `:fill`, etc.) — 4 cases

**Feature**: `4 5 ⍴ :match ⍳20` (reshape with options).

**Why deferred**:
- T2.1 is a Tier 2 cluster (reshape spec) per ROADMAP §A.3.
- The 4 cases are documented in `PROGRESS-20260904b.md` as
  T2.1 — already worked on in a separate session, NOT the
  T1.1 session.
- Listed in the carryover only because the user asked for
  "all open work in T1.x".

**Blast radius**: zero (separate session, separate cluster).

**Status quo**: 4 cases fail. T2.1 work tracked elsewhere.

**Unblock path**: per `PROGRESS-20260904b.md`.

**Conformance cases deferred**: 0 (already T2.1-tracked, not
a T1.1 decision).

---

## 8. s:col qualified-name column-lookup — half-day investigation

**Feature**: `(names ⍪ values)[:; col]` (qualified name → column
index → select).

**Why deferred**:
- Requires THREE independent pieces to be in place:
  1. `s:col` symbol-lexical-scope resolution (operator
     namespace)
  2. `:;` axis-spec-of-axis-spec parser quirk
  3. `⊃[axis]` (carryover #1 above)
- Even if I implemented `s:col` today, the test cases still
  fail because they USE `⊃[axis]`.

**Blast radius**: independent of T1.1; out-of-scope.

**Status quo**: tests fail with "axis specifier not supported"
(stemming from ⊃[axis], NOT from `s:col` itself).

**Unblock path**: do `s:col` AFTER `⊃[axis]` is done. Otherwise
the fix is invisible.

**Conformance cases deferred**: ~4 (in `s:col`-using test
names; the actual count is uncertain because the test names
aren't enumerated in the audit).

---

## Summary

| # | Feature | Cases | Why |
|---|---|---|---|
| 1 | ⊃[axis] | 11 | New value type, half-day |
| 2 | +[axis] monadic | ~1 | Reverted: wrong semantics |
| 3 | +/ adverb distinguish | 0 | Already works via adv_explicit_axis |
| 4 | ∧/∨ sort-along-axis | 5 | New helper, not in allowlist |
| 5 | Byte-identical probe | 0 | P8 display noise, useless signal |
| 6 | T1.3 labels cluster | 40 | Separate workstream |
| 7 | T2.1 reshape spec | 4 | Separate workstream |
| 8 | s:col | ~4 | Depends on ⊃[axis] (#1) |

**Total T1.1 carryover**: 17 cases (⊃[axis] 11 + sort 5 + +monadic 1).
**Adjacent clusters**: 48 cases (T1.3 40 + T2.1 4 + s:col ~4).
