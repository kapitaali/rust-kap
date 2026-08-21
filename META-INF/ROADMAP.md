# ROADMAP — Rust Kap rewrite

Consolidated from the dated `PROGRESS-2026*.md` session logs. Tracks the
open Phase 6 breadth work and deferred items. Branch invariant (enforced
after every commit): `main == strings == origin/*`.

## Current phase: Phase 6 breadth

Filling in remaining structural / array builtins to raise conformance
coverage against the Kotlin corpus (2,535 extracted cases). Baseline
coverage at last measure: **~43%** (1091 ok / 278 mismatch / 1186 unsupported).

## Open scope targets (Phase 6 breadth gaps)

Pick one to scope per session:

- `⊆` / `⊇` — partition / shape  **(DONE)**
- `∘` / `≬` — operators
- `→` — branch / guard
- key / major-cell operators: `⌺` / `⌸`, `⍋⍒`-with-axis
- format family
- **`⌷` (squad / index selection) — CRITICAL, currently mis-dispatched to
  `disclose`** (see `KNOWN-NONCONFORMANCE.md`). `2 ⌷ 1 2 3 4` returns the whole
  array instead of `3`. Largest mismatch/unsupported driver. Needs a real
  index-select distinct from `⊃`.
- **`≡` / `≢` (match) — CRITICAL, wrong semantics** (see
  `KNOWN-NONCONFORMANCE.md`). Implemented as `deep_equal→1/0` with no type
  strictness; `10≡10.0`→`1` (oracle `0`). Needs Kap's match (type/depth → depth
  or 0).
- **`⊃` (reveal / disclose + nested pick) — FIXED** (2026-08-21). Monadic discloses
  (identity for simple arrays, drops outer axis for `⊂`-nested); dyadic is pick-with-
  dimension-checks with Kap's exact error text. Verified vs Kotlin oracle + source.

## Worst-covered corpus files (where coverage gains live)

| File | ok / total |
|------|------------|
| `CompareTest.kt` | 62 / 81 |
| `LabelsTest.kt` | 62 / 63 |
| `ReshapeTest.kt` | 58 / 100 |
| `NumbersTest.kt` | 44 / 85 |
| `InverseFnTest.kt` | 41 / 47 |
| `ReduceTest.kt` | 38 / 62 |

## Deferred (explicitly out of scope for now)

- **`regex:replace` with a lambda replacement function** — e.g.
  `"x([A-Z])" regex:replace (…;λ{…})`. Only the `(subject; replacement)`
  *string* form is supported.
- **Dyadic interval `⍸`** (`a ⍸ b`) and inverse `⍸˝` (needs `˝` adverb) —
  returns a clean "not implemented" error so the harness counts it Unsupported.
- **`use()` file-loading / `.kap` stdlib kernel** (`standard-lib.kap` +
  `base-functions.kap`) — deferred.

## Suggested hardening

- Add a curated conformance row exercising a large-vector reduce
  (e.g. `+⌿ 100000 ⍴⍳2 → 50000`) to lock in the O(n) hang-fix behavior.

## Already closed (current branch)

- **`⊆` (partitioned enclose) implemented** — `evaluator.rs::partitioned_enclose`
  mirrors Kotlin `PartitionedEncloseFunction` (disclose.kt). Monadic = "nest"
  (scalar passes through; else the array is enclosed whole). Dyadic `A ⊆ B`
  partitions `B` along the last axis using `A`'s integer indicators (a `>0`
  at `i>0` opens a new partition; an indicator `>1` repeats). Verified against
  the `kap-jvm-text` oracle: `1 0 1 ⊆ 1 2 3 → ((1 2) (3))`,
  `1 0 1 0 1 ⊆ 10 20 30 40 50 → ((10 20) (30 40) (50))`, etc. Display uses the
  port's `()` convention (not the oracle's `⟨⟩`). Registered in BOTH
  `evaluator.rs::is_primitive_name` and `parser.rs::is_primitive_op`. Added 6
  curated parity rows to `conformance.rs`. `⊇` (PickAPLFunction) — see below.
- **`⊇` (pick) implemented** — `evaluator.rs::pick_apl` mirrors Kotlin
  `PickAPLFunction` / `PickResultValue` (lookup.kt). Result shape = shape of
  `A` (left); each element of `A` is an *index coordinate* into `B` (right): a
  scalar index for rank-1 `B` (with `¯n` negative support), a coordinate vector
  for higher-rank `B`. Errors match the oracle ("Index out of bounds",
  "rank mismatch"). Scalar results render as `(x)` (the port's cell convention,
  consistent with `⊂`/`⊆`), not the oracle's bare scalar. Verified:
  `0⊇1 2 3 4 5 → (1)`, `2⊇… → (3)`, `¯1⊇… → (5)`, `1 0 2⊇10 20 30 40 → (20 10 30)`.
  Registered in BOTH lists. Added 4 curated parity rows.
- **Adverbs `¨` / `/` / `\\` now bind to named user functions** — `dbl¨ 1 2 3`,
  `dbl/ 1 2 3`, `dbl\ …` work (commit `828913a`). Previously `unknown function: ¨`.
- Dyadic `⍳` index-of + string `cmp` (commit `ee7df32`-era).
- `regex:*` namespace (match/find/findall/replace/split/compile).
- `⍸` (where) empty/Null cases + dyadic-out-of-scope error.
- O(total²) hang-fix in reduce/scan (per-case timeout so the broad sweep
  runs green by default).

## Kap syntax ground rules (must hold for all future work)

- Inline functions are dfns: `{ … }` with `⍺`/`⍵` as left/right args. No
  parameter names.
- A local function is named with `⇐`: `minus ⇐ -`, `leftPlus5Times ⇐ {⍺ + ⍵×5}`.
- `λ` is a unary operator over an *existing* function expression
  (`λ {⍺+⍵×5}`, `λ -`). It has **no `λ(x) λ(y) …` form** — that is LISP
  currying and is NOT Kap. Never write, probe, or reason about it.
- `f ⇐ (g 3)` is **not valid Kap** — Real Kap errors "Right side of the
  arrow must be a function". A bare `Apply` is not a function value.
