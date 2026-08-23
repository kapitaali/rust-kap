# PROBLEM — Roadmap Tier A: `⌸` via `use("base-functions.kap")` — operator value-reference blocker

**Date:** 2026-08-23
**File under investigation:** `kap-core/src/parser.rs` (`parse_apply` / operator-call detection,
parser.rs:939-967), `kap-core/src/evaluator.rs` (`apply_user_op`, evaluator.rs:2127+), and
`use_file` error handling (`evaluator.rs:7665+`).
**Test input:** `use("base-functions.kap")` then `1 1 2 3 3 ⌸ +` (the Real-Kap calling convention
for an operator defined `∇ (keys) (fn ⌸) (values) {…}`).

---

## Symptom

`use("base-functions.kap")` dies during load, and the `⌸` operator (a `∇`-defined operator) cannot
be referenced or called. Minimal repro:

```bash
printf '∇ (keys) (fn ⌸) (values) { keyindex ← =keys  ((≠keyindex)/keys) ,[0.5] ⍞fn¨ keyindex⫇values }\ntypeof ⌸\n' \
  | ./target/debug/kap --no-standard-lib 2>&1
# actual:   <operator>
#           error: undefined symbol: keyindex
# expected: <operator>            (operator is a first-class value; body should NOT run on reference)
```

The def parses as `<operator>` (good), but **any bare reference to `⌸` runs its body instead of
returning the operator value**. `keyindex ← =keys` inside the body fails because no `keys` is bound.

---

## Root cause (narrowed, NOT yet fixed)

At def time, `parse_fn_def` pushes the operator name into `known_ops` (parser.rs:751). The
operator-call detector in `parse_apply` (parser.rs:939-967) then treats ANY occurrence of a
`known_ops` symbol as the start of an `OpCall` — even when the symbol is a *value reference*
(`typeof ⌸`, the `⍞fn` dynamic-ref operand, a bare `⌸` in a strand). With no function operand
following, it becomes `OpCall { op: ⌸, left_fn: ???, … }` and `apply_user_op` (evaluator.rs:2127)
runs the body with empty data/operand args → the body-local `←` binding fails.

**Real Kap (oracle) treats an operator as a first-class value**: referencing `⌸` yields the operator;
it is only *applied* (OpCall) when used with function operand(s) (`data ⌸ fn`). The port's
`parse_apply` is too eager.

### Key evidence
- `∇ (keys) (fn ⌸) (values) {…}` → `<operator>` (def parses). `typeof ⌸` → `error: undefined symbol:
  keyindex` (body runs). Reproduced with body variants with/without `⫇`/`⍞` — the failure is the
  value-reference, not the body contents.
- Removing `⌸` from `known_ops` at def time is NOT the fix: `known_ops` is also needed so that
  `data ⌸ fn` parses the operator-call form. The detector (parser.rs:939-967) must distinguish
  "operator used WITH operands" from "operator referenced as a value".

---

## Secondary issue (must also hold for `use` to match the oracle)

The roadmap's Tier-A acceptance ("`use(base-functions.kap)` loads with no errors") is **not
achievable** — the oracle ITSELF errors on `base-functions.kap` **line 13**
(`declare(:const (⎕A ⎕a ⎕d))` → `Assignment to constant variable: kap:⎕A`) and then **aborts the
whole file at line 13**. So `⌸` (lines 3-8) is registered and callable as `data ⌸ fn`, but
`⎕A`/`⎕a`/`⎕d` (lines 14-17, after the abort) are NOT. The port currently dies even earlier (on the
operator value-reference), so matching the oracle requires: (1) operator-as-value reference (above),
and (2) `use_file` tolerating a per-statement error and continuing (oracle behaviour) instead of
aborting the whole file.

---

## Oracle (ground truth)

```bash
# ⌸ call convention (data ⌸ fn), after a working def:
printf '∇ (keys) (fn ⌸) (values) { keyindex ← =keys  ((≠keyindex)/keys) ,[0.5] ⍞fn¨ keyindex⫇values }\n1 1 2 3 3 ⌸ +\n' \
  | ~/Apps/array/kap-jvm-text/bin/kap-jvm-text 2>&1 | grep -aE '⊢ '
# Real Kap: keyed grouping — result like (1 2 3)(1 2)(1 1 2 3 3) / or the Kotlin key/value form.

# base-functions.kap load (note the line-13 error is intrinsic to the file on Real Kap too):
printf 'use("base-functions.kap")\n' | ~/Apps/array/kap-jvm-text/bin/kap-jvm-text 2>&1 | grep -aE '⊢ '
# -> Error … Assignment to constant variable: kap:⎕A   (line 13); ⌸ still defined.
```

---

## Open question / next diagnostic step

The fix is an operator-parse disambiguation, not a missing primitive. Concretely: in
`parse_apply` (parser.rs:939-967), the eager-OpCall block should fire ONLY when a *function-operand
token* immediately follows the operator name (same `next_is_function_token()` gate the right-operand
branch already uses at line 963), and otherwise fall through to a plain `Symbol` value reference.

Before implementing, confirm against the oracle:
1. `typeof ⌸` → returns the operator value (NOT an error).
2. `1 1 2 3 3 ⌸ +` → keyed grouping, oracle-result-shape.
3. `⍞fn¨ keyindex` inside the body → applies `fn` to each `keyindex` element (dynamic-ref form).

Verify the change does NOT regress existing operator usage: `2 -foo+ 3`, `∇ (x op) a {…}`-style
derived operators, and the `⌷`/`⌺`/train operators already in the port. Re-run
`cargo test -p kap-core --lib` (92/0) and `curated_kap_parity` after.

---

## Status
- Diagnostic complete; root cause localized to operator value-reference in `parse_apply`.
- PROGRESS-20260823.md ADDENDUM 3 records the full Tier-A probe baseline + proposed fix scope.
- No fix attempted yet — write-up first per blocker discipline.
