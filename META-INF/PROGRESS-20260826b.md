# PROGRESS-20260826b — dual-nature `/ ⌿ \ ⍀` (value-left select/expand)

## Trigger
Continue roadmap. `util.kap` (ROADMAP §10 P7) fails to load at line 14:
`((toBoolean ⍞fn)¨ arg) / arg` — the `/` here is the VALUE-LEFT replicate,
but the port rejected value-left `/` with `Operator without left function: /`.

## Root cause (oracle-first, Kotlin source of truth)
`array/.../engine.kt` registers `/ ⌿ \ ⍀` as **BOTH** a native *function* AND a
native *operator*:
- `registerNativeFunction("/", SelectElementsLastAxisFunction())`  (engine.kt:332)
- `registerNativeFunction("⌿", SelectElementsFirstAxisFunction())` (engine.kt:333)
- `registerNativeFunction("\\", ExpandLastAxisFunction())`         (engine.kt:334)
- `registerNativeFunction("⍀", ExpandFirstAxisFunction())`         (engine.kt:335)
- AND `registerNativeOperator("/", ReduceOpLastAxis())` etc.      (engine.kt:486+)

So each glyph is dual-nature: value-left → function (replicate/compress/expand);
function-left → operator (reduce/scan). The port only knew the operator half, so
value-left hit the accumulator's "Operator without left function" guard
(parser.rs:612). `×`/`÷` are NOT dual (pure functions) — confirmed by oracle.

## Fix
1. **parser.rs** — dual-nature exception at the main-loop operator guard (parser.rs:612):
   `/ ⌿ \ ⍀` with a value left operand now route through `finish_fn_call` (function
   form) instead of erroring. `known_ops` is left intact so `+/` etc. still bind as
   reduce/scan via `bind_operators_kotlin`. Also added `⌿ \ ⍀` to `is_primitive_op`
   (parser.rs:2782) so the primitive path recognizes them.
2. **evaluator.rs** — generalized the old `/` `replicate` into four value-left forms:
   - dispatch arm (evaluator.rs:2165) routes `/ ⌿` → `select_elements` (last/first
     axis), `\ ⍀` → `expand` (last/first axis); monadic → oracle-exact error
     "Function cannot be called with one argument".
   - `select_elements` (axis-aware replicate/compress): scalar A broadcasts to all
     B cells; validates A length == B size on selected axis; handles rank-0..n B.
   - `expand` (axis-aware expand/insert-zeros): positive A[i] copies B cell i A[i]
     times, 0 inserts one zero, negative inserts |A[i]| zeros; validates selected
     count == B size on axis.
   - helpers `collect_ints`, `coord_of`, `flat_of`, `repeat_along_axis`,
     `expand_along_axis` (all `Self::`-scoped associated fns in `impl Engine`).
   - `replicate` kept as `select_elements(..., last_axis=true)` (legacy name).

## Oracle-exact verification (RUST == ORACLE, value bodies; only `Error at: L:C:`
prefix differs — pre-existing P8 gap, left alone)

| Expr | Oracle | Port |
|---|---|---|
| `1 0 1 / 1 2 3` | `⟨1 3⟩` | `(1 3)` ✓ |
| `1 0 1 ⌿ 1 2 3` | `⟨1 3⟩` | `(1 3)` ✓ |
| `3 / 7` (scalar-left) | `⟨7 7 7⟩` | `(7 7 7)` ✓ |
| `1 0 1 2 / 10 20 30 40` | `⟨10 30 40 40⟩` | `(10 30 40 40)` ✓ |
| `⍴ (1 0) / (2 2⍴⍳4)` | `⟨2 1⟩` | `(2 1)` ✓ |
| `⍴ (1 0) ⌿ (2 2⍴⍳4)` | `⟨1 2⟩` | `(1 2)` ✓ |
| `⍴ 1 1 0 \ (3 2⍴⍳6)` | `⟨3 3⟩` | `(3 3)` ✓ |
| `1 1 0 \ 1 2 3` (bad dims) | expand err | expand err ✓ |
| `(1 1 0) ⍀ (3 2⍴⍳6)` (bad dims) | expand err | expand err ✓ |
| `1 0 1 ⌿ (2 2⍴⍳4)` (A size mismatch) | dim err | dim err ✓ |
| `+/ 1 2 3` (reduce, unaffected) | `6` | `6` ✓ |
| `+\ 1 2 3` (scan, unaffected) | `⟨1 3 6⟩` | `(1 3 6)` ✓ |
| `+/[0] (2 2⍴⍳4)` (axis reduce) | `⟨2 4⟩` | `(2 4)` ✓ |
| `/ 1 2 3` (monadic) | "Function cannot be called with one argument" | same body ✓ |

## util.kap load
Before: `6/17` statements failed (parse error at 14:18).
After: `1/12` statements failed — the remaining one is `≠ requires numbers`
(ROADMAP §?, a SEPARATE pre-existing eval bug in util.kap, NOT the line-14 parse
error). The `((toBoolean ⍞fn)¨ arg) / arg` line now parses and the `/` replicate
evaluates (verified via the matrix above).

## Gates
- `cargo test -p kap-core --lib` → **96 passed, 0 failed**.
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed, 0 failed**.
- `cargo build -p kap-cli` → green.

## Out of scope (unchanged this session)
http.kap / thread.kap / graph.kap / fhelp*.kap still error (ROADMAP §10).
`≠ requires numbers` in util.kap is a separate blocker (noted, not fixed).
