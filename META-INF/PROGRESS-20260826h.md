# PROGRESS 2026-08-26 (h) — axis-applied take/drop `↑[k]` / `↓[k]`

## [2026-08-26] `↑[axis]` / `↓[axis]`: a silently-wrong-value parser gap

Continuation of the roadmap. Chasing the `output3.kap:118:37` parse error left by
`2426014` uncovered a **worse** bug sitting behind it.

### The reported error was the lesser problem

`output3.kap:118` is
`((⌈arrayMaxWidth[0]÷⍺)↑[¯1+≢⍴⍵])«,»((-⌈arrayMaxWidth[1]÷⍺)↑[¯1+≢⍴⍵]) ⍵`.
Isolating it produced a parse error — but reducing further showed something worse:

| expr | port (before) | oracle |
|---|---|---|
| `2↑[0] ⍳6` | `(1)` | `⟨0 1⟩` |
| `2↓[0] ⍳6` | `(1)` | `⟨2 3 4 5⟩` |
| `2↑[9] ⍳6` | `(1)` | `Error: ↑: Axis 9 is not valid. Expected: 1` |
| `2↑[0] "abcdef"` | `((0) "abcdef")` | `⟨@a @b⟩` |

**No error at all — just a wrong value**, and even a nonsense axis (`[9]`) returned
`(1)`. The `"abcdef"` row is the diagnostic tell: the axis `(0)` was STRANDED beside
the argument, so `↑` was never applied. This is the parser-strands-instead-of-applies
signature from the skill's debugging heuristic.

### Root cause

`parser.rs:884` gates the `f[axis]` → `AxisApplied` wrap behind an allowlist:
`"+" | "-" | "×" | "÷" | "*" | "," | "⍪" | "⌽" | "⊖"`. `↑`/`↓` were absent, so
`↑[0]` never became an `AxisApplied` and the `[0]` was parsed as an ordinary
bracket-index primary that then stranded.

Kotlin ground truth: `TakeAPLFunctionImpl` / `DropAPLFunctionImpl` extend plain
`APLFunction`, **not** `NoAxisAPLFunction` (drop.kt:8/:307), and their `eval2Arg`
has an explicit `axis != null` branch (drop.kt:335-347):
- the left argument must be a **single integer** (`:339-343`), else
  `"When given an explicit axis, the left argument must be a single integer"`;
- `ensureValidAxis(axisInt, bDimensions)` (`:345`);
- the selection is `IntArray(rank) { i -> if (i == axisInt) argInteger else 0 }` (`:346`);
- monadic + axis ⇒ `AxisNotSupported` (`:309`).

### Fixes

- **`kap-core/src/parser.rs` (~:884)** — added `↑`/`↓` to the `axis_ok` allowlist,
  with the Kotlin justification and the stranding symptom recorded in a comment.
- **`kap-core/src/evaluator.rs` (~:1209)** — new `"↑" | "↓"` arm in the
  `AxisApplied` dispatch: single-integer left-arg check, `ensureValidAxis` bounds
  check, and the per-axis selection vector. All three error texts reproduce
  Kotlin's wording exactly.
- **`kap-core/src/evaluator.rs` — new `take_or_drop_opt`** — `take_or_drop` now
  delegates to it. **This was the subtle part:** Kotlin's selection array uses `0`
  for "leave this axis alone", but the port's `slice_axis` reads `Some(0)` as *take
  zero elements* and `None` as *whole axis*. Passing `0` for the unselected axes
  emptied every rank-≥2 take (`2↑[1] 2 3⍴⍳6` → `()`). Unselected axes now pass
  `None` for take; for drop, `Some(0)` already means "drop nothing", so both
  encodings agree and it is kept.

### Verification (port vs oracle)

| expr | port | oracle | ok |
|---|---|---|---|
| `2↑[0] ⍳6` | `(0 1)` | `⟨0 1⟩` | ✓ |
| `2↓[0] ⍳6` | `(2 3 4 5)` | `⟨2 3 4 5⟩` | ✓ |
| `(-2)↑[0] ⍳6` | `(4 5)` | `⟨4 5⟩` | ✓ |
| `2↑[1] 2 3⍴⍳6` | values `(0 1 3 4)`, `⍴`=`(2 2)` | 2×2 `0 1 / 3 4` | ✓ |
| `2↑[0] 2 3⍴⍳6` | values `(0 1 2 3 4 5)`, `⍴`=`(2 3)` | 2×3 | ✓ |
| `1↓[1] 2 3⍴⍳6` | values `(1 2 4 5)`, `⍴`=`(2 2)` | 2×2 `1 2 / 4 5` | ✓ |
| `1↓[0] 2 3⍴⍳6` | values `(3 4 5)`, `⍴`=`(1 3)` | 1×3 | ✓ |
| `2↑[9] ⍳6` | `↑: Axis 9 is not valid. Expected: 1` | same body | ✓ |
| `↑[0] ⍳6` | `↑: Function does not support axis specifier` | same body | ✓ |
| `2 3↑[0] ⍳6` | `↑: When given an explicit axis, the left argument must be a single integer` | same body | ✓ |

No regressions in the plain forms: `2↑ ⍳6`, `2↓ ⍳6`, `2 2↑ 3 3⍴⍳9`,
`1 1↓ 3 3⍴⍳9`, `↑ 1 2 3`, `↓ 1 2 3` all still oracle-exact.

### One divergence investigated and cleared as PRE-EXISTING

`⍴ 2↑[1] 2 3⍴⍳6` → `error: reshape dimensions must be integers` (oracle `⟨2 2⟩`).
This is **not** a shape bug — via a variable the shape is correct (`(2 2)`). It is a
`⍴`-precedence quirk: `⍴` binds as dyadic reshape taking the `2` as its left
argument. **Baseline discriminator run** (`git stash push` of BOTH coupled files —
parser.rs AND evaluator.rs — then rebuild): the pre-change binary produces the
identical error, while `⍴ ⌽[0] 2 3⍴⍳6` works in both. So it predates this work and
is unrelated to the axis allowlist. Recorded, not fixed.

(Note: stashing both files together is deliberate — a single-file stash produced a
non-compiling "baseline" earlier today, see PROGRESS-20260826f process notes.)

### Regression rows added — `kap-core/tests/conformance.rs` (~:692)

7 value rows (rank-1 take/drop, negative count, and the four rank-2 shape checks
via a variable) plus the 3 error texts as comments (the harness cannot assert error
text). All pass.

### Gates (re-run, all GREEN)

- `cargo test -p kap-core --lib` → **96 passed; 0 failed**
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed; 0 failed**
- `cargo build -p kap-cli` clean (1 pre-existing warning).

### stdlib load state

| file | before | after |
|---|---|---|
| `output3.kap` | 89/122 `parse error at 118:37` | 90/123 `parse error at 118:62` |
| `util.kap` / `stat.kap` / `structure.kap` / `io.kap` / `standard-lib.kap` | CLEAN | CLEAN |
| `map.kap` | 18/24 parse error 13:6 | unchanged |
| `math.kap` | 1/9 `math:pi` | unchanged (oracle-consistent) |

**Honest reading of that row:** the axis specifier at col 37 now parses (the error
moved to col 62, the `«,»` fork), and the statement count went 122→123 because more
of the file now parses far enough to be counted. The failure count moving 89→90 is
NOT a regression — it is one more statement being *reached*. Line 118 as a whole
still fails.

### Remaining / next

- **`output3.kap:118:62`** — the `«,»` fork whose members are axis-applied takes:
  `(…↑[…])«,»(…↑[…]) ⍵`. Isolated: `(2↑[0])«,»((-2)↑[0]) ⍳6` → port
  `parse error at 1:8: expected a function in train`, oracle `⟨0 1 4 5⟩`. Note the
  paren-wrapped `(2↑)` form ALSO fails (`(2↑)«,»((-2)↑) ⍳6` → `unexpected token in
  primary`), so this is about a left-bound take being accepted as a fork member,
  not about the axis. Next item.
- Carried over: dyadic structural under; pick-based under (lookup.kt:232);
  `typeof "hi"` → `kap:string` vs oracle `kap:array`; `map.kap` 13:6.
