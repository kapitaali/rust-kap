# ROADMAP — Rust Kap rewrite

Consolidated from the dated `PROGRESS-2026*.md` session logs. Tracks the
open Phase 6 breadth work and deferred items. Branch invariant (enforced
after every commit): `main == strings == origin/*`.

## Current phase: stdlib-kernel (Phase 4) + Phase 6 breadth

Phase 6 breadth is now **effectively complete** — all listed structural/array builtins
(`⊆`/`⊇`, `∘`/`⍛`, `≬`, `→`, bracket-index, `⌷`, `≡`/`≢`, `⊃`, the `⍕` format family)
are DONE and match the Kotlin oracle. The remaining genuinely-open work is the **stdlib
kernel** (`use()` file-loading + the Kap-source stdlib), which unblocks `⌸`/`⌺`/`⎕*` and
the `s:`/`io:` helpers. This is the next planned stage; strategy below.

## Stdlib-kernel strategy (Phase 4 of `RUST_REWRITE_STRATEGY.md` §5–§6)

`use("file.kap")` is a **lexical `IncludeToken`** in Kotlin (`parser.kt::processInclude` →
`includeFileContent`): it resolves the path (`engine.resolveLibraryFile` / `resolvePathName`),
builds a fresh `APLParser` over the file, and runs `engine.withSavedNamespace { innerParser
.parseValueToplevel() }` — a **parse-time file include evaluated into the current namespace**.
The port already has the in-memory `namespace` / `import` / `declare` directives wired in
`evaluator.rs`; it is **missing only `use` itself** (plus two glyphs the stdlib exercises).

### Tier A — minimal kernel: load `base-functions.kap` (unblocks `⌸`, `⎕p`, `⎕pl`, `⎕A`, `⎕a`, `⎕d`)

`standard-lib/base-functions.kap` is only **18 lines** and defines `⌸` as a pure-Kap user
function (`∇ (keys) (fn ⌸) (values) { … ⍞fn¨ keyindex⫇values }`) plus the `⎕` quad symbols.
Its *entire* missing-port dependency surface (everything else it uses — `⍋`/`≠`/`/`/`¨`,
`io:print`/`io:println` — is **already present** in the port):

| Missing port feature | Kotlin source | Notes |
|---|---|---|
| `use(...)` file-include directive | `parser.kt::processInclude` / `includeFileContent` | New lexer `IncludeToken` + parser arm + `engine.resolveLibraryFile`/`resolvePathName` (map a lib path to `standard-lib/` on disk). Evaluate the file as a top-level `Instr` in the current namespace. |
| `⍞` apply-reference operator | `tokeniser.kt::ApplyToken` | **Unary** operator over a *function-valued symbol* (oracle: `⍞+` → "Variable not assigned: kap:+" because `+` was never bound to a var). `⍞fn` applies the function named by `fn`. Port has none. |
| `⫇` GroupFunction | `engine.kt:351` `registerNativeFunction("⫇", GroupFunction())` (disclose.kt) | Native pick-with-axis (`A⫇B` selects cells of `B` by index vector `A` along the major axis). Port has none. |
| `,[axis]` catenate-with-axis | `catenate` in `evaluator.rs` (currently axis-less) | `base-functions.kap` uses `,[0.5]`. Needs axis support added to the existing `,`. |

**Acceptance for Tier A:** `use("standard-lib/base-functions.kap")` (or a vendored copy) loads
with no errors, and afterwards `⌸` / `⎕A` / `⎕p` behave like the oracle (probe each vs
`kap-jvm-text`). Add curated rows for `⌸` / `⎕A←@A…@Z`.

### Tier B — `standard-lib.kap` chain (the full `use` web)

`standard-lib.kap` does `use("structure.kap") … use("fhelp.kap")` — 13 includes pulling in
`math`/`io`/`regex`/`util`/`map`/`time`/`stat`/`http`/`thread`/`output3`/`graph`/`fhelp`.
Many of those `.kap` files call builtins the port may still lack (e.g. `math:` namespace,
`chart:`, `http:`, `thread:`). **Strategy: load the chain incrementally, one file at a time,
and implement/repair only the builtins each file actually exercises** — not the whole Kotlin
surface at once. The `math.kap`/`io.kap`/`util.kap`/`structure.kap` subset is the highest-value
first slice; `http.kap`/`thread.kap`/`graph.kap`/`fhelp.kap` can stay deferred (they need
networking/threading/charting builtins that are explicit out-of-scope per `RUST_REWRITE_STRATEGY.md` §1.2).

### Tier C — wire `kap-cli` to load the vendored stdlib at startup

The `kap-stdlib` crate **already exists** (workspace member) and already vendors the Kap
source: `kap-stdlib/std/{standard-lib,base-functions,structure,math,io,util,regex,time,
stat,map,http,thread,output3,graph,fhelp}.kap` (+ `kap-stdlib/test/test.kap` and hundreds of
extracted `*.kap` conformance cases). So Tier C is **not** "vendor the files" — it is: (a) make
`use()` resolve paths against `kap-stdlib/std/` (or a configured lib dir), and (b) have `kap-cli`
call `use("standard-lib.kap")` at startup (mirroring `LinuxReplBuilder.loadStartupFiles`). Each
loaded file is a regression test: if it parses and its functions run, that surface is compatible.
The `test/` dir doubles as the D5 Kap-native harness (run `test.kap` + `test/*Test.kap`, parse the
`TESTS total=N pass=M fail=K` summary).

### Mechanics notes (faithful port)

- `use` must resolve relative to a registered library directory (the Kotlin `resolveLibraryFile`
  maps `"base-functions.kap"` → the stdlib dir; `secureMode` restricts to registered paths — the
  port can skip secure-mode for now). Absolute paths pass through.
- `use` evaluates in the **current namespace** (`withSavedNamespace` save/restore), so a file's
  `namespace("kap")` switches in, defines symbols, and the outer namespace is restored.
- `⍞` needs a parser distinction: a *symbol* operand (function reference) vs an *apply* — the
  port's `is_primitive_op`/`is_primitive_name` gates must let `⍞` take a bare symbol as its operand.

### Rollout order (recommended)

1. **Tier A** (4 features) → `⌸`/`⎕*` work. Highest leverage, smallest surface. **Do this first.**
2. **Tier B** structure/math/io/util subset, one file per commit, gated by side-by-side probe.
3. **Tier C** vendoring + startup load.

## Open scope targets (Phase 6 breadth gaps)

Pick one to scope per session:

- `⊆` / `⊇` — partition / shape  **(DONE)**
- `∘` / `⍛` (compose / reverse-compose trains)  **(DONE)** — `≬` is NOT compose; it is
  `toList` (see below). `∘`/`⍛` already wired (lexer→parser→ast→evaluator trains).
- `≬` / `toList` (+ inverse `fromList`)  **(DONE, 2026-08-21)** — see `KNOWN-NONCONFORMANCE.md`.
- `→` — branch / guard  **(DONE)** — see `KNOWN-NONCONFORMANCE.md` (`→` committed earlier this branch)
- bracket indexing `x[sel]` (`Instr::Index` → `index_select`)  **(DONE)** — see `KNOWN-NONCONFORMANCE.md`
- key / major-cell operators: `⌺` / `⌸`, `⍋⍒`-with-axis
  - `⍋⍒`-with-axis is **N/A**: Kotlin `GradeFunction` extends `NoAxisAPLFunction`,
    so axis specifiers are explicitly unsupported (oracle errors "Function does not
    support axis specifier"). Nothing to implement — the port correctly rejects it.
  - `⌺`/`⌸` are **NOT native** (stdlib `use()`-loaded `kap:keys`/`kap:stencil`); they are
    reachable once the **stdlib-kernel (Tier A)** is built — see the "Stdlib-kernel strategy"
    section below. `base-functions.kap` (18 lines) defines `⌸` as a pure-Kap user fn; the
    only port gaps are `use`, `⍞`, `⫇`, `,[axis]`.
- format family  **(DONE, 2026-08-21)** — see `KNOWN-NONCONFORMANCE.md` (`⍕` monadic
  flatten + dyadic `$s`/`$h`/`$$` directives all match the oracle).
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
- **`use()` file-loading / `.kap` stdlib kernel** — now scoped as a planned stage, not
  open-ended deferral. See the **"Stdlib-kernel strategy"** section above: Tier A (load
  `base-functions.kap`) needs only `use` + `⍞` + `⫇` + `,[axis]`; Tiers B/C follow.

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
- **`⍕` format (monadic flatten + dyadic directives) — FIXED (2026-08-21)** —
  monadic `⍕` now uses `formatted(PLAIN)` (recursive flatten, no separators/parens;
  `⍕ 1 2 3 → "123"`), via new `lib.rs::format_plain`. The dyadic `$s`/`$h`/`$$`
  directive compiler (Kotlin `format.kt`) was already implemented and matches the
  oracle. 8 new monadic curated parity rows added.
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
