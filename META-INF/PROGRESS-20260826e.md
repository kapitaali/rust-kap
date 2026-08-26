# PROGRESS-20260826e — native `int:libInitialised` (P7 stdlib-chain load)

## Summary
Implemented the missing native `int:libInitialised` (Kotlin
`div_functions.kt::LibInitialisedFunction`, `engine.kt:433`). This was the
sole in-scope blocker in `standard-lib.kap` (`1/14 statements failed: error:
unknown function: int:libInitialised`), which blocked the whole stdlib header
from loading clean. After this fix **`use("standard-lib.kap")` no longer emits
a `standard-lib.kap` warning**.

### Grounding (ROADMAP §0 — oracle-first, never assume)
- Oracle `int:libInitialised 0` → `⍬` (null). `typeof int:libInitialised` →
  `Error at: 1:1: No arguments specified for function` (it is a monadic
  native fn). Our vendored stdlib only calls it as `int:libInitialised 0`
  (standard-lib.kap:15).
- Kotlin source: `LibInitialisedFunctionImpl.eval1Arg` calls
  `context.engine.callPendingLibInitialisations()` then returns
  `APLNullValue`. The Kap text-mode build has NO pending initialisers
  (`declare(:initialise …)` is unused in our stdlib), so a faithful no-op
  returning null is correct.

### Changes (two-gate registration per ROADMAP §3.2)
- `evaluator.rs`: added dispatch arm `"int:libInitialised" =>
  Ok(Rc::new(APLValue::Null))` (after `int:formatRational` arm, ~line 1762).
  `APLValue::Null` renders as `⍬` (lib.rs:194) = oracle.
- `evaluator.rs::is_primitive_name` (~2858): added `"int:libInitialised"`
  (eval-time late gate).
- `parser.rs::is_known_fn` (~165): added `libInitialised` to the `ns == "int"`
  allowlist (parser-time admission). `is_primitive_op` deliberately NOT touched
  — it only admits BARE operators (names with `:` return false), and namespaced
  builtins resolve at runtime via `is_known_fn` (two-gate pattern for
  namespaced natives, same as `int:intern`/`math:sin`/`io:print`).

### Oracle-exact verification
- `int:libInitialised 0` → `⍬` (R == O). ✓
- `use("standard-lib.kap")` → `standard-lib.kap 1/14` warning GONE. ✓

## Non-regressions / deliberately-not-fixed
- `math:pi` `4/10` warning in `math.kap` is **oracle-consistent** — oracle
  ALSO errors `Assignment to constant variable: math:pi` (math.kap:5). The port
  became correct by mirroring the oracle; NOT a gap to "fix". Left as-is.
- Bare-arity error text for namespaced primitives (`int:libInitialised` /
  `int:intern` called with NO args) differs from oracle: port →
  `error: undefined symbol: libInitialised`, oracle →
  `No arguments specified for function`. **Pre-existing category gap** —
  `int:intern` (already in port) behaves identically wrong; NOT introduced by
  this change. Separate arity-gate item for namespaced primitives (stdlib never
  triggers it: it always calls `int:libInitialised 0`).
- Remaining `use()` warnings are all ROADMAP §10 out-of-scope: `output3.kap`
  (train-parse — §10.265 says re-test after P1; own mini-phase), `map.kap`
  (own mini-phase §10.264), `fhelp.kap` (null symbol), `http.kap` (networking).

## Gates
- `cargo test -p kap-core --lib` → **96 passed; 0 failed**.
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed; 0 failed**.

## Files changed
- kap-core/src/evaluator.rs (dispatch arm + is_primitive_name)
- kap-core/src/parser.rs (is_known_fn int: allowlist)
