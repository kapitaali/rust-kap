# PROGRESS 2026-08-26 (i) — expression-valued left-bind + `⌊`/`⌈` rational reduction

## [2026-08-26] Two evaluator/number fixes; one wrong hypothesis reverted

Continuation of the roadmap, chasing `output3.kap:118:62` (the `«,»` fork).

### Bug 1 — left-bind whose bound VALUE is an expression, not a literal

Isolating output3's line 118 fork narrowed to a much broader defect:

| expr | port (before) | oracle |
|---|---|---|
| `((-2)↑) ⍳6` | `error: only symbol/lambda functions supported yet` | `⟨4 5⟩` |
| `((1+1)↑) ⍳6` | same error | `⟨0 1⟩` |
| `((-2)+) 5` | same error | `3` |
| `((⌈3÷2)↑) ⍳6` | same error | `⟨0 1⟩` |
| `(¯2↑) ⍳6` | `(4 5)` ✓ | `⟨4 5⟩` |

Note `¯2` (high-minus literal) worked while `(-2)` (a parenthesised negation
*expression*) did not — that asymmetry was the diagnostic tell.

**Root cause (found by TRACING, after three wrong guesses — see process notes):**
the parser was already building the correct
`Train[Apply{fn:-, right:2}, Symbol{↑}]`. The defect was in the **evaluator**:
`is_value` (evaluator.rs) accepted only `Literal | Array | Empty`, so
`apply_train`'s left-bind arm (`funcs.len() == 2 && Self::is_value(&funcs[0])`)
did not match, execution fell through to the ATOP case, and the value member was
then treated as a function → the "only symbol/lambda functions supported yet"
fallthrough in `eval_apply`.

Kotlin ground truth: `makeLeftBindFunction` (parser.kt:486) binds the whole
accumulated `leftArgs` list — **any value instruction**, not just literals.

**Fix (one site, `kap-core/src/evaluator.rs` `is_value`):** widened to include
`Apply`, `Index`, `BooleanOp`, `Value`. `Symbol` is deliberately EXCLUDED — a bare
symbol in a train is normally a function reference (`(f g)`), and admitting it
would break plain 2-trains. Consequence: `(n↑)` with a variable `n` still fails
(`unknown function: n`); that is a separate parser-side gap, left open rather than
papered over.

### Bug 2 (found BY the curated gate) — `⌊`/`⌈` must reduce an exact rational

Adding `("((⌈3÷2)↑) ⍳6", "(0 1)")` as a regression row turned the gate RED with
`error: ↑/↓ counts must be integers`. Probing:

| expr | port (before) | oracle |
|---|---|---|
| `typeof ⌈3÷2` | `kap:rational` | `kap:integer` |
| `typeof ⌊3÷2` | `kap:rational` | `kap:integer` |

Kotlin uses `makeAPLNumberWithReduction()` in the **rational** branch of both
`⌊` (math_functions.kt:1278) and `⌈` (:1341), which reduces a denominator-1
rational to an integer. The port's Double branches already normalised to `Long`,
but the Rational branches returned a bare `Rational`.

**Fix (`kap-core/src/number.rs`):** new `KapNumber::reduce_rational` (denominator 1
⇒ `Long`, or `BigInt` when it does not fit; else `Rational`), applied in both
`ceil()` and `floor()`.

Verified rationals that must STAY rational are untouched: `typeof 3r2` →
`kap:rational`, `3÷2` → `3/2`, `typeof 3÷2` → `kap:rational`. Negatives correct:
`⌊¯3r2` → `¯2`, `⌈¯3r2` → `¯1` (both oracle-exact).

### Verification

All oracle-exact after: `((-2)↑) ⍳6` → `(4 5)`, `((1+1)↑) ⍳6` → `(0 1)`,
`((-2)+) 5` → `3`, `((⌈3÷2)↑) ⍳6` → `(0 1)`, `typeof ⌈3÷2` / `typeof ⌊3÷2` →
`kap:integer`, `⌈3÷2` → `2`, `⌊3÷2` → `1`.

Regression sweep, all unchanged and oracle-exact: `(2↑) ⍳6`, `(10+) 1`,
`3 (+ « × » -) 4`, `(⌽⍢⌽) 1 2 3 4`, `10 (-,) 20`, `¯2 3 4 (×∘-) 1000`,
`10 (-⍛+) 100`, `(1 2+≢) 5`, `(1+2)(3+4)`, `(⌷1 2 3)`, `f ⇐ ×- ⋄ f 3`,
`1 2 3 +∙× 1 2 3`, `⌈3.7`, `⌊3.7`, `3 ⌈ 5`, `3 ⌊ 5`, `⌈/ 3 9 2 7`, `⌊/ 3 9 2 7`,
`⌈ 1 2 3`.

### Regression rows added — `kap-core/tests/conformance.rs` (~:712)

`((-2)↑) ⍳6`, `((1+1)↑) ⍳6`, `((-2)+) 5`, `((⌈3÷2)↑) ⍳6`.

### Gates (re-run, all GREEN)

- `cargo test -p kap-core --lib` → **96 passed; 0 failed**
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed; 0 failed**
- `cargo build -p kap-cli` clean.

### stdlib load state — UNCHANGED

`output3.kap` still 90/123 at `118:62`; util/stat/structure/io/standard-lib CLEAN;
map 18/24 @13:6; math 1/9 (`math:pi`, oracle-consistent). **These two fixes do NOT
move output3's line 118** — the `«,»` fork with left-bound members is still
blocked. They are independent correctness wins found while investigating it, not
progress on that line; recorded plainly rather than framed as advancing output3.

### Remaining / next

- **`output3.kap:118:62`** — still the `«,»` fork. Isolated:
  `(2↑)«,»((-2)↑) ⍳6` → port `error: No arguments specified for function`, oracle
  `⟨0 1 4 5⟩`; and `(2↑[0])«,»((-2)↑[0]) ⍳6` → port
  `error: unsupported axis operator: ↑`. So there are (at least) two further gaps:
  a left-bound function as a **fork tine**, and the axis form inside a fork.
  Simple forks are fine (`(2↑)«,»⌽ ⍳6`, `⌌«,»(2↑) ⍳6`, `3 (+ « × » -) 4` all
  oracle-exact), so it is specifically the combination.
- `(n↑)` — variable as a left-bind value (`unknown function: n`), parser-side.
- Carried over: dyadic structural under; pick-based under (lookup.kt:232);
  `typeof "hi"` → `kap:string` vs oracle `kap:array`; `map.kap` 13:6.

### Process notes (important — I got this wrong three times first)

I made **three speculative parser edits** on the hypothesis that the paren
classifier was at fault (`next_is_paren_operator`'s nested-group handling,
`fold_train_members`'s value set, `parse_paren_vfn_chain`'s `first_is_value`).
Each looked plausible and each FAILED to fix the case. Only after adding a
temporary `KAP_TRACE_PAREN` `eprintln!` did the truth appear: **the trace never
fired at all**, proving `parse_function_atom`'s `OpenParen` arm was not even on
the live path (the Kotlin accumulator at parser.rs:457 owns it), and a second
trace showed the parser's output instr was ALREADY correct — so the bug could
only be in the evaluator.

All three speculative parser edits were **reverted** (`git checkout -- parser.rs`)
and the final change is one widened `matches!` in the evaluator. Lessons, now also
in the skill:
- **Trace before editing when the failure is "wrong/absent behaviour" rather than a
  compile error.** Dump the built `Instr` and confirm which function actually runs;
  a plausible-looking gate that is never reached will absorb unlimited effort.
- **A debug probe that does not fire is itself the finding** — it disproves the
  assumed code path (this is the stale-binary heuristic's sibling; here the binary
  WAS fresh, confirmed by forcing `touch kap-core/src/*.rs && cargo build`).
- **`cargo build` reporting "Finished in 0.0Ns" after an edit means it did NOT
  relink.** This bit me twice this session; `touch` the edited file first, then
  re-probe, before concluding a fix does not work.
- **A curated regression row can surface an unrelated real bug** — Bug 2 was found
  only because Bug 1's row exercised `⌈3÷2`. Worth adding rows that compose
  features rather than testing each in isolation.
- No debug instrumentation remains in the tree (the `KAP_TRACE_PAREN` probes were
  removed with the parser revert; verified by `git diff --stat`).
