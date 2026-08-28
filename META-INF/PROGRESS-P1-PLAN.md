# PROGRESS — P1 Parser Migration Plan (2026-08-28)

## Current state

The Kotlin accumulator parser is the DEFAULT path (commit d65cb39,
`KAP_KOTLIN_PARSER=0` opts out). The port's parser is structurally close to
Kotlin but has feature gaps that cause genuine conformance mismatches —
NOT display issues (the conformance JSONL uses Kotlin's internal `()`
notation, so mismatches are real bugs).

## P1 remaining steps (estimated 6 independent fixes)

### Step 1: Fix `⟦⟧` function-call list to build `Instr::List`

**File:** `kap-core/src/parser.rs` `parse_function_call_list` (~line 1064)

Currently builds `Instr::Array` (space-stranded) but Kotlin builds
`APLList` (`Instr::List`). This breaks `fn⟦a;b;c⟧` semantics — the list
must be a single argument, not spread.

**Oracle:** `+/⟦1 2 3 4⟧` → `⊢ 10` (currently returns `(1 2 3 4)`)

**Fix:** Change line 1101 from `Instr::Array { elements: elems }` to
`Instr::List { elements: elems }`. The evaluator already handles
`Instr::List` correctly — it's only the parser producing the wrong node.

**Tests affected:** `+/⟦1 2 3 4⟧`, `g⟦1;2;3⟧`, `∇ foo (a;b;c)` calls

### Step 2: Fix `∇` definition to return `⍬`

**File:** `kap-core/src/evaluator.rs` ~line 927 (`Instr::UserFnDef`)

Currently returns the function value (`<function>`). Kotlin returns
`APLNullValue` (`⍬`) for function definitions.

**Oracle:** `∇ foo (a;b;c) { a+b+c }` → `⊢ ⍬` (currently returns `<function>`)

**Fix:** After `env.define(...)`, return `Ok(Rc::new(APLValue::Null))`
instead of `Ok(v)`.

**Tests affected:** All `∇ foo …` definitions in FunctionCallParenTest

### Step 3: Fix `⊤` encode with negative B

**File:** `kap-core/src/evaluator.rs` `encode` (~line 11379)

Currently uses raw modulo which produces negative digits for negative B.
Kotlin's `vectorEncode` uses two's-complement-style encoding.

**Oracle:** `(10⍴10) ⊤ ¯10` → `⊢ ⟨9 9 9 9 9 9 9 9 9 0⟩`

**Fix:** For each negative B element, compute `radix - ((-v) % radix)` for
the lowest digit, then carry `((-v) / radix)` with sign. Reference:
Kotlin `vectorEncode` in math-kap.kap.

**Tests affected:** `decodeNegative`, `decodeNegativeBigInt`, `decodeScalarLeftArg4`

### Step 4: Fix `\` expand algorithm

**File:** `kap-core/src/evaluator.rs` (search for `"\\\\"` dispatch)

Currently returns `(3 null 4)` instead of `(3 0 4 5)` for `1 0 1 1 \ 3 4 5`.
Negative repeats should insert `|n|` zeros; zero repeats drop the element.

**Oracle:** `1 0 1 1 \ 3 4 5` → `⊢ (3 0 4 5)`; `1 2 ¯2 \ 10 11` → `⊢ (10 11 11 0 0)`

**Fix:** Re-implement expand per Kotlin `ExpandFunction` — iterate counts,
for positive count repeat the element, for negative count insert `|count|` fills,
for zero count skip.

**Tests affected:** `simpleExpand1D`, `extendedNumRepeats`, `extendedNegativeRepeat`

### Step 5: Fix `∩` rank check

**File:** `kap-core/src/evaluator.rs` `intersection` (~line 10225)

Currently returns `(1 2)` for `(2 2⍴⍳4) ∩ 1 2`. Oracle errors with
"∩: All but the first axis needs to have the same dimensions. Ranks: A=[2, 2], B=[2]".

**Oracle:** `(2 2⍴⍳4) ∩ 1 2` → Error

**Fix:** Add a rank-compatibility check before computing members: if both
args have rank > 1 and their trailing axes don't match, error.

**Tests affected:** `invalidDimensionLeftArg`, `invalidDimensionRightArg`

### Step 6: Fix indexed assignment error on non-variable target

**File:** `kap-core/src/parser.rs` `parse_value_kotlin` `LeftArrow` arm (~line 541)

Currently accepts `(1 2 3)[2] ← 3` silently. Oracle errors with
"Indexed assignment can only be used when the target is a variable".

**Oracle:** `(1 2 3)[2] ← 3` → Error at: 1:1: Indexed assignment can only be used when the target is a variable

**Fix:** After popping the target from `left_args`, if it's an `Instr::Index`,
check whether the array operand is a simple `Symbol`. If not (e.g. a literal
array, a function call result), emit the indexed-assignment error.

**Tests affected:** `indexedAssignmentToExplicitArrayShouldFail`, `indexedAssignmentToStringShouldFail`

## Verification

Each step: `cargo build -p kap-cli` + oracle probe + `cargo test -p kap-core --lib`
and `cargo test -p kap-core --test conformance curated_kap_parity`.

After all steps: full conformance sweep, update PROGRESS-20260828.md with
final ok/mismatch counts, commit, push, enforce branch invariant.

## Target outcome

From 1500 ok / 267 mismatch → target ~1560 ok / ~207 mismatch (60+ fixes
from the above steps).
