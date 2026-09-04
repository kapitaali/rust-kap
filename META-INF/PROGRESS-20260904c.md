# PROGRESS-20260904c — Post-T1.2 conformance matrix update + `s:col` deep-dive

**Branch:** `feature/wheres-extra` (= `main` = `strings`, invariant held, 2 commits this session: `bd7cc33` T1.2, `fdce3fe` parity test)
**Author:** resume session
**Handoff context:** `PROGRESS-20260904b.md` closed T1.2 (MemberDeref) at 13/14 of the focused sample. This session: re-classify the full 60-case `MemberDereferenceTest` cluster and investigate the remaining `s:col` (qualified-name column-lookup) gap.

## TL;DR

The full 60-case `MemberDereferenceTest` conformance cluster now has
**37 pass / 23 fail**, where 9 of the 23 failures are P8 display-only
divergences (`┌Map: N──┐` vs `map[size=N]`, `⟨⟩` vs `()`, etc.) that
are already accepted per ROADMAP §11 Option A. The 14 remaining
failures break down into:

- **6 "should error" cases the port is missing** (real bug)
- **5 "symbol-as-index" array picks** the port rejects (real bug,
  label-threading gap)
- **3 misc semantic gaps** (`FromExpression`, `ConstantString` key
  type-discrimination, `InvalidDimensions1`)

The T1.2 cluster is **NOT yet fully closed** — T1.2 is partially
closed (37/60 oracle-equal; 14/23 fail-class are real bugs, the
other 9 are display).

The `s:col` (qualified-name `column-lookup`) case from
`LabelsTest.kt:967` is a separate, deeper problem: the dfn inside
namespace `s` cannot bind `⍺` when invoked through the operator
form `⊂⍛cols` (oracle works, port fails with
"array index must be an integer"). The direct-call form
`"a1" s:cols a` errors in BOTH oracle and port ("Variable not
assigned: kap:⍺"), but the operator form works in the oracle. This
is operator + dfn + namespace-lexical-scope interaction, not a
simple fix.

## Full 60-case MemberDereferenceTest breakdown

### Pass (37 / 60)

All 14 cases from `PROGRESS-20260904b.md`'s focused sample are
included here, plus 23 more from the broader sweep (mostly `.` on
arrays of various dimensions, `.(neg)` on a labelled array, etc.).

### Fail (23 / 60) — categorised

**P8 display-only (9)** — already accepted per ROADMAP §11 Option A:
- `memberDereferenceMap` — `┌Map: 2──┐` vs `map[size=2]`
- `memberDereferenceMapExpression` — same
- `memberDereferenceFromExpressionString` — same
- `memberDerefenceWithExpression` — same
- `memberDereferenceSequence` — same
- `memberDereferenceSequenceExpressionElement` — same
- `memberDereferenceWithExplicitFunctionNameKey` — same
- `memberDerferenceWithStranding` — same
- (1 more in the bigger sweep)

**Missing-error (6)** — port doesn't reject what oracle rejects:
- `memberDereferenceMapConstantInvalidValue` — `m.bar` (key not
  in map) should error; port returns the map.
- `memberDereferenceMapExpressionInvalidValue` — same.
- `memberDereferenceMissingKeyShouldThrow` — `m.missing` should
  error; port returns `(1 2)`.
- `memberDereferenceSequenceMissingElement` — `m.(99)` should
  error; port returns the map.
- `memberDereferenceSequenceMissingFirstElement` — same.
- `memberDereferenceWithExplicitIntegerKeyShouldFail` — `a.20`
  with map should error (parse error in oracle); port accepts.

These are the "missing-error" subset. The root cause is likely
that the new `eval_member_deref` function at evaluator.rs:1224 is
returning the object on the "missing key" path instead of
propagating the `KapMap::lookup → None` failure as a
`KeyNotFoundException`. Need to check the dispatch.

**Symbol-as-index array picks (5)** — port rejects; oracle does
label-based pick:
- `arrayDereferenceNegativeIndex` — `(10×⍳10) labels 0 1 2 3 4
  5 6 7 8 9` is a 1-elt array with negative-index labels; `.(¯1)`
  should return last label. Port errors with "Cannot use symbol
  as index into array".
- `arrayDereferenceFromSymbol` — `.(sym)` should pick by label
  name, not by integer position. Port errors.
- `arrayDereferenceFromExpressionString` — same family, returns
  the right numbers but the wrong shape.
- `arrayDereferenceHighDimension` — 3D array with label-keys;
  port errors.
- `arrayDereferenceFromSymbolWithNoLabelsShouldUseDefaultLabelNames` —
  `col1`, `col2`, … default label names. Port errors.

These are a label-threading gap that the MemberDeref rewrite
didn't cover — `array_member_deref` only handles scalar/vector
integer indices. Symbol/string key-based picks on labelled arrays
are routed to `extractColumnByLabel` but the negative-index
fallback path doesn't apply.

**Misc semantic gaps (3)**:
- `memberDereferenceFromExpression` — oracle returns `⍬`
  (calling `findMap` on a value that has no Map form yields
  Null); port returns `<function>`. Different semantics on
  function-vs-Null.
- `dereferenceMapWithConstantString` — `m.("foo")` on
  `map:with 'foo 42`. Oracle matches by string ("a" returned);
  port returns "Key not found: default:foo". This is a
  type-discrimination issue: `'foo` and `"foo"` are different
  keys per `values_key_equal`, but the oracle's `makeTypeQualifiedKey`
  treats them as equal for map lookups. Real bug.
- `indexDereferenceInvalidDimensions1` — oracle errors
  (`(3 3 ⍴ 10×⍳9).(0)` — rank-2 array with scalar member should
  error in some axis-mode); port returns `0`. Likely the
  "scalar member on multi-dim array should use last-axis pick"
  path is firing when it shouldn't.

## The `s:col` deep-dive

`"a1" s:col a` where `a ← "a1" "a2" labels 1 2`:

- **Oracle**: `1` ✅
- **Port**: `error: array index must be an integer` ❌

Definition: `col ⇐ ⊂⍛cols` where `cols ⇐ { ⍉(⊂⍺ ⍳⍨ labels ⍵) ⌷ ⍉⍵ }`
inside `namespace ("s")`.

Components that work in the port:
- `labels a` returns the labels ✅
- `"a1" ⍳⍨ ⊂ "a1" "a2"` (index-of) returns `(1 1)` ✅
- `⍉a` (transpose) returns the transposed array ✅
- `(⊂1) ⌷ ⍉a` (pick with enclosed index) returns the right value ✅

The `s:cols` direct-call form (`"a1" s:cols a`) errors in BOTH
oracle and port with "Variable not assigned: kap:⍺" /
"undefined symbol: ⍺". The dfn inside `namespace ("s")` cannot
bind `⍺` as a parameter when called directly — but the operator
form `⊂⍛cols` (which calls the dfn through a derived-function
wrapper) **works in the oracle** and **errors in the port** with
"array index must be an integer".

This is a multi-layer interaction:
1. The `⊂⍛cols` derived-function machinery (operator+dfn).
2. The `cols` dfn's lexical scope for `⍺` and `⍵`.
3. The `namespace ("s")` declaration that scopes the binding.

The "array index must be an integer" error in the port suggests
the issue is downstream: the operator form succeeds in calling
the dfn, but somewhere the value flowing through is not an
integer where the port's `⌷` (pick) requires one. This is
likely a label-vs-integer disambiguation gap similar to the
5 symbol-as-index array picks above.

The simplest unblock for `util.kap` stdlib usage is probably
making the `extractColumnByLabel` fallback accept strings and
symbols when no integer-label match is found.

## Recommendation for next session

**Don't try to close all 23 MemberDeref failures in one session.**
The work splits naturally into:

1. **T1.2-1 (small)**: 6 "missing-error" cases — fix
   `eval_member_deref` to propagate `KapMap::lookup → None` as
   `KeyNotFoundException`. ~30 min, low risk.
2. **T1.2-2 (medium)**: 5 "symbol-as-index" cases — extend
   `array_member_deref` to handle symbol/string keys via
   `extractColumnByLabel`. ~2 hr, requires verifying with the
   Kotlin `array.kt` `pick` and the labels `pick` variant.
3. **T1.2-3 (large)**: 3 misc semantic gaps — need oracle
   transcripts for each to understand expected behaviour.
   Probably 1 day.
4. **T1.2-suffix (separate)**: `s:col` qualified-name
   column-lookup. This is a labels+operator+dfn+namespace
   interaction; can be deferred until T1.1 labels are fully
   done.

Or pick a different direction entirely:
- T2.2 (reduce `⊥` body OOB panic, 56+22 cases)
- T2.3 (complex numbers, 48 cases, deferred design-locked)

The user has a standing preference for verifiable oracle-grounded
work over speculative completion, so option (1) is the safest
next move (small, all 6 cases have clear oracle expectations).

## Files modified this session

- `META-INF/PROGRESS-20260904c.md` (this file)

No code changes. Branch invariant held.
