# ROADMAP — Rust Kap rewrite (v2)

*Reworked 2026-08-23 from the code-analysis sessions (`code_analysis.md`,
`code_analysis_02.md`, `code_analysis_03.md`) and a fresh audit of the Kotlin
tree. Supersedes v1; completed-phase history lives in `PROGRESS-2026*.md`.*

**Design principle of this roadmap:** the Rust port mirrors the Kotlin source
module-for-module, function-for-function, error-text-for-error-text. When in
doubt about *how* to build something, the answer is "open the Kotlin file this
row points at and translate its structure", never "invent a Rust-y approach".
Every phase below names its Kotlin anchor files up front.

---

## 0. Ground truth (non-negotiable, unchanged)

Kap behaviour comes from exactly two sources, never memory/intuition/APL habit:

1. **Kotlin source** at `~/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/`
   — READ it, never run the Gradle build.
2. **Real Kap binary** — `~/Apps/array/kap-jvm-text/bin/kap-jvm-text`
   (`--lib-path=$HOME/Apps/array/kap-jvm-text/standard-lib`). Probe with
   `printf 'expr\n' | …/kap-jvm-text …`; result line `⊢ <value>`.

**Never claim "Kap does X" without a captured oracle transcript.** This now
applies to problem write-ups too (lesson 03-T2): an "Oracle (ground truth)"
section must contain *pasted output*, never predicted output. Three sessions in
a row were derailed by unverified premises (stale `int:unwindProtect` belief,
inverted `use()` abort semantics, phantom operator-as-value rule).

## 0.1 Process law (distilled from three analysis sessions — obey all of it)

1. **Stale binary first.** After ANY parser/evaluator edit:
   `cargo build -p kap-cli` (not `-p kap-core` alone). If a symptom looks
   impossible: `git stash && cargo build -p kap-cli && probe && git stash pop`
   — ONE discriminator pass, trust its verdict, move on.
2. **Two-gate registration.** Every new builtin goes into BOTH
   `evaluator.rs::is_primitive_name` AND `parser.rs::is_primitive_op`
   (namespaced builtins: full `ns:name` in every gate list).
3. **Gates before commit, every time.** `cargo test -p kap-core --lib`
   and `cargo test -p kap-core --test conformance curated_kap_parity`.
   A red or untested gate invalidates the commit.
4. **No leniency creep.** Where Real Kap errors, the port errors with the same
   message. Silent extra tolerance is a bug, not a feature (03-B2/B4).
5. **Fix harness expectations only when the ORACLE says so** — a curated row
   that disagrees with the port is checked against the oracle before either is
   touched (01-F3: both failing rows were stale, code was right).
6. **Never paper over semantics with allocation guards.** Result shape bounds;
   element values don't (`⍸` lesson).
7. **Don't trust bucket lists — read the Kotlin file.** `≬` was mislabelled a
   compose operator for weeks because a roadmap lumped it wrong.
8. **PROGRESS before continuing**: `META-INF/PROGRESS-YYYYMMDD.md` (check
   `date --rfc-3339=date`), every changed file:line, oracle-vs-port outputs,
   gate numbers — written BEFORE the next coding step.
9. **Small tool calls.** Oversized patches/reads time out mid-stream; split
   edits (<8K tokens) and reads (limit+offset).
10. **Branch invariant** after every sync point: `main == strings == origin/*`.

---

## 1. Architecture north star — the module mirror

The Kotlin tree is ~26k lines across clearly separated modules. The port's
target layout mirrors it. Where the port currently differs structurally, the
migration IS the roadmap item (see P1, P2).

| Kotlin (source of truth) | Port target | Status |
|---|---|---|
| `tokeniser.kt` | `lexer.rs` | exists, mostly aligned |
| `parser.kt` (parseExpr accumulator, processFn, makeResultList) | `parser.rs` | **diverged** — heuristic valence; migrate per P1 |
| `syntax/syntax.kt` (defsyntax machinery) | macro section of parser/evaluator | landed 2026-08-23; keep aligned with `processCustomSyntax` |
| `engine.kt` (197 `registerNative*` calls = master builtin inventory) | `evaluator.rs` dispatch + both gate lists | partial; build ledger per §3 |
| `types.kt`, `number.kt` (APLValue kinds, Long/Double/rational/complex) | `number.rs`, `lib.rs` | partial (Long/Double/rational); complex deferred |
| `builtins/math_functions.kt` (scalar layer, 2284 ln) | `num2`/`num2_axis` region | **restructure per P2** |
| `builtins/{reduce,scan}` (`reduce.kt` 462 ln) | reduce/scan region | partial (axis gaps) |
| `builtins/reshape.kt` | reshape fn | partial (MATCH/FILL/TRUNCATE/RECYCLE incomplete) |
| `builtins/concatenate-array.kt` (625 ln) | catenate region | partial (`,[axis]` laminate incomplete) |
| `builtins/transpose.kt` (617 ln) | transpose region | partial-axis done |
| `builtins/disclose.kt` (⊃ ⊆ pick group, 597 ln) | disclose/pick region | mostly done |
| `builtins/lookup.kt` (pick/index-of/access, 437 ln) | bracket-index/⌷/⍳ region | done |
|| `buildins/operator.kt` (rank ⍤, power ⍣, 418 ln) | `apply_rank_op` etc. | **CLOSED (P4)**: rank `⍤` done (ValueOp, verified `PROGRESS-20260830.md`); `⍣` implemented (integer-iterate + `f⍣g` inverse-do-while, `PROGRESS-20260824.md` + `PROGRESS-20260830.md`); `∵` bitwise family corrected (`PROGRESS-20260830.md`: `adverb_bitwise` table + `popcount_bigint` + recursive monadic; `192 ∨∵ 31 → 223`, `¯6 ⌽∵ 5 → 0` verified). **Corrected from stale v2 claim "Not started" / "P4 blocker" — both implemented.** |
|| `builtins/bitwise_ops.kt` (∨∵ ∧∵ ⌽∵ BitwiseOp) | `adverb_bitwise` (evaluator) | **CLOSED (P4)**: registered (`is_primitive_op` + `is_primitive_name`); table corrected (`PROGRESS-20260830.md`: `×∵`/`+∵`/`-∵` compute AND/XOR/XOR; `⍸∵` popcount with two's-complement; `~∵`/`⍴∵`/`⍸∵` monadic recurse preserving array nesting). Verified 26 cases (`192 ∨∵ 31 → 223` etc.). |
| `builtins/gamma.kt` (! factorial/binomial, 980 ln) | partial | P5 |
| `builtins/format.kt` ($ directives) | done | keep aligned |
| `rendertext.kt` (box renderer) | `format_value/display` | P8 — conform display mode implemented (`--conform-display`); default house style preserved |
| `standard-lib/*.kap` | `kap-stdlib/std/*.kap` | per-file milestones (P7) |

---

## 2. Master instrument: the engine.kt coverage ledger

Before adding builtins ad hoc, generate the authoritative checklist:

```bash
grep -oE 'registerNative(Function|Operator)\("[^"]+"' \
  ~/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/engine.kt \
  | sed 's/.*("//' | sort > /tmp/kotlin_builtins.txt
grep -oE '"[^"]*"' kap-core/src/evaluator.rs \  # from is_primitive_name + dispatch arms
  | sort -u > /tmp/port_builtins.txt
comm -23 /tmp/kotlin_builtins.txt /tmp/port_builtins.txt   # what the port lacks
```

Work P3–P5 **in engine.kt registration order**, ticking off the ledger. This
guarantees nothing is missed and prevents roadmap-bucket mislabelling (law 7).
Commit the regenerated ledger with each phase so progress is diffable.

---

## 3. Phase P0 — conformance infrastructure repairs (small, do first)

*Why first:* every later phase depends on honest gates and honest `use()`.

| # | Task | Kotlin anchor | Notes |
|---|------|---------------|-------|
| 0.1 | Fix the 2 stale curated rows: `"3 | 2"` expected `"1"` → `"2"`; `"5 ∊ 1 2 3 4"` expected `"(0)"` → `"0"` (oracle-verified) | — | lesson 01-F3 |
| 0.2 | Decide `use()` error policy: Kotlin aborts the file at the FIRST failing statement (earlier definitions persist). Port currently warns-and-continues (commit `3a54593`). Either restore abort-at-first-error or document tolerance in KNOWN-NONCONFORMANCE as deliberate. Do NOT leave accidental | `includeFileContent` / repl-builder load path | controlled test: assign→error→assign via use; oracle leaves name 2 unassigned |
| 0.3 | Error-text table: start `ERRORS.md` mapping every Kotlin exception class (common.kt) to its exact text; port arms must emit these verbatim | `common.kt:150–200+` | e.g. `InvalidOperatorArgument` → "Operator without left function: X"; `IllegalContextForFunction` → "No arguments specified for function" |
| 0.4 | Kill the `group_indices` panic: guard `b_dims.len() != 1` → existing size-mismatch error (evaluator.rs ~4518) | `builtins/group-index.kt` | crash found in 03-B3 |

Acceptance: gates green; `typeof ⌸` (after a `∇` op def) no longer panics.

## 4. Phase P1 — parser architecture migration (highest-value structural work)

**Goal:** replace the heuristic valence parser with a faithful translation of
Kotlin's single-pass expression loop. Every recurring parser burn of the last
weeks (unary-minus arm swallowing `L - R`, invented `f L R` dyadic form, eager
OpCall executing operator bodies, paren-operator misclassification) traces to
the same root: the port guesses where Kotlin *accumulates*.

**Kotlin anchor:** `parser.kt::parseExpr` (:939–1100): one `while(true)` loop
over tokens; `leftArgs` accumulates operands; hitting a function calls
`processFn(fn, leftArgs, pos)` — non-empty `leftArgs` ⇒ dyadic; end-tokens via
`END_EXPR_TOKEN_LIST` produce `makeResultList(leftArgs)`.

**Breakdown (each step independently gated):**

1. **Read & annotate.** Map the Kotlin loop: every token class → its handler
   (Name/OpenParen/OpenBrace/fn-def/custom-syntax/adverb/end-token). Write the
   map into `references/parser_migration.md` before touching Rust.
2. **Port `processFn` semantics exactly**: function-first ⇒ monadic (`⍵=R`,
   even if R is a strand); operand-before-function ⇒ dyadic (`⍺=L, ⍵=R`);
   NO function-first-two-operands form exists. Remove any port branch that
   contradicts this.
3. **Valence at eval, not parse.** Delete special-case arms that peek at
   specific glyphs (`-`, adverb suppression, paren-operator lookahead chains).
   A bare `-` with no left arg is simply monadic minus at eval time.
4. **Operator references are parse errors**: after `lookupFunction` fails,
   `getOperator(symbol) != null` ⇒ throw `InvalidOperatorArgument`
   (parser.kt:967–971). Port equivalent: known-op symbol with no function
   operand context ⇒ `"Operator without left function: X"` — kills 03-B1/B2.
5. **Strand collection** mirrors `makeResultList`: consecutive value operands
   strand; a trailing lone value is just that value.
6. **Feature-flag the migration**: `Parser::new_kotlin_loop()` behind an env
   var; run BOTH parsers over the full conformance corpus; flip when the new
   parser is ≥ old on ok-count AND matches oracle on every hand-probe in
   `references/parser_migration.md`.

**Non-regression probes (both engines side-by-side):** `3 - 4`, `3-4`, `-x`,
`(-padding)↓…`, `ch-@\0`, `2 (+) 3`, `(1+2)(3+4)`, `f ⇐ ×-`, `⊢«⊣»,`,
`10 (-,) 20`, `-⍛+`, `2 ×¨ 3 4 5`, `+/ 1 2 3`, `1 2 3 +[0] 4 5 6`,
`(≠⌸)` derivation, `data ⌸ fn` → must ERROR like oracle, `typeof ⌸` → must
error, `foo ⇐ ⌸` → must error.

## 5. Phase P2 — scalar function layer (math_functions.kt, 2284 lines)

**Goal:** one faithful `MathCombineAPLFunction` equivalent instead of
per-glyph ad-hoc `num2` arms.

**Breakdown:**

1. Translate the dispatch ladder of `eval2Arg` (:497): (a) scalar+scalar
   short-circuits BEFORE any axis handling (`2 +[0] 3 → 5`); (b) long×long
   fast path; (c) double promotion; (d) array×array cell-wise; (e) ONLY THEN
   axis broadcast (`num2_axis` already exists — fold it under (e)).
2. One generic `combine(a, b, op)` core parameterised by a small op table
   (add/sub/mul/div/pow/min/max/residue/…). Each Kotlin descriptor class
   (AddAPLFunction etc., :598+) becomes ONE table row: monadic fn + dyadic fn +
   identity element + axis support flag.
3. Ambivalence audit: every row declares its monadic form explicitly
   (`×`=signum, `÷`=reciprocal, `*`=exp, `!`=gamma, `⌈⌋`=ceil/floor,
   `⊢⊣`, `≡`=depth, `= ≠` self-classify/unique-mask). Any glyph without a
   declared monadic arm must ERROR monadically like Kotlin, not fall through.
4. Type-promotion rules from `number.kt` (long→double→rational) centralised in
   the combiner, not scattered in match arms.

Acceptance: the full scalar block of the broad sweep improves; no behavior
change on the curated suite.

## 6. Phase P3 — structural builtins, file by file

Work in engine.kt ledger order within this phase. Per file: read the Kotlin
file top-to-bottom, enumerate its public behaviours, probe each against the
oracle, implement the deltas, add curated rows.

| Kotlin file | Known port deltas to close |
|---|---|
| `reshape.kt` (442) | dimension-spec ladder: literal `¯1` only ⇒ MATCH (divisibility-checked); keyword specs `:match :fill :truncate :recycle` (findSizeCalculationMethod :375); other negatives error "Attempt to reshape to dimension with negative size". Current port accepts ANY negative dim — tighten (02-F4/03) |
| `concatenate-array.kt` (625) | `,[axis]` general + `,[0.5]` laminate used by base-functions.kap line 5; prototype/cell alignment rules |
| `reduce.kt` (462) | reduce/scan with explicit axis; lazy interval right args (size = ⍴b, never materialise values as counts); the known OOB panic path in user-`⊥` bodies |
| `transpose.kt` (617) | verify prefix-fill perm rule against ALL TransposeTest cases (partial-axis done 2026-08-21) |
| `disclose.kt` (597) | ⊃/⊆/pick corner matrix (already largely done; sweep the remaining CompareTest/ReshapeTest misses) |
| `lookup.kt` (437) | done; regression-watch only |
| `drop.kt`/take-first | monadic ↑ First semantics locked; keep |
| `outer_join.kt` | `∘.f` table builder — likely missing entirely (check ledger) |
| `format.kt` | done; keep aligned |

## 7. Phase P4 — operators (operator.kt + bitwise_ops.kt)

1. **Rank `⍤`** — done (ValueOp); verify spec rules vs `operator.kt:82`
   RankOpFunctionImpl once more after P1 lands (it changes parse adjacency).
2. **Power `⍣`** — Kotlin anchor `operator.kt` PowerOperator: `f⍣n` iterate n,
   `f⍣g` inverse-do-while. Not started. Breakdown: integer-iterate first,
   then the inverse-detect form with its termination semantics.
3. **Bitwise `∵` family** (`bitwise_ops.kt` — `∨∵ ∧∵ ⌽∵ ±∵`): registered at
   engine.kt:495. REQUIRED by io.kap `encodeUtf8Char` (lines 6–9). Two-gate
   registration + evaluator arms; oracle probes: `192 ∨∵ 31 → 223`,
   `¯6 ⌽∵ 5 → 0`.
4. Commute `⍨` (done), compose trains (done) — regression-watch after P1.

## 8. Phase P5 — numeric tower & specials

1. Rational literals `¯1r2` (stat.kap uses them) — check lexer; Kotlin
   `number.kt` rational via num-rational (D2 says num-bigint/num-rational —
   confirm what exists, fill gaps).
2. `!` gamma/binomial via `libm::lgamma` (skill documents the recipe; land the
   libm dep).
3. Complex: DEFERRED until everything else is green (Kotlin supports it; port
   has zero surface; big win-per-effort is low).
4. Char arithmetic edge rules: `a - b` char−char ⇒ int; int−char ⇒ error
   "Incompatible argument types"; negative codepoint ⇒ "Codepoints cannot be
   negative" — mirror texts exactly (oracle transcripts in 01 appendix).

## 9. Phase P6 — native quad constants + const enforcement

Current Kap ships `⎕A ⎕a ⎕d` as NATIVE read-only constants (fresh-session
oracle probes: `⎕A` works with NO stdlib loaded; `typeof ⎕A → kap:array`).
The vendored `base-functions.kap` still assigns them and therefore explodes on
the real engine (line 13) — our copy is byte-identical to upstream's, i.e.
upstream's own stdlib is stale relative to its engine.

1. Implement `⎕A ⎕a ⎕d` natively (char vectors; read-only slot in ns registry).
2. Implement `declare(:const …)` enforcement: assignment-time check, error
   `Assignment to constant variable: <ns>:<name>` (oracle OM probe).
3. Only AFTER 1–2: refresh vendored `base-functions.kap` (or drop the redundant
   assignments) so the file loads clean under the stricter engine. Sequence
   matters: enforcing consts first would faithfully reproduce the oracle's
   self-destructing load.

## 10. Phase P7 — stdlib chain milestones (status measured 2026-08-23)

Per-file acceptance = the file loads via `use()` AND its exported symbols
behave identically to the oracle in a scripted side-by-side. Current state:

| file | status | blocking features (probe-first, don't assume) |
|---|---|---|
| structure.kap | ✅ loads; `when`/`unwindProtect` work | — (defsyntax keystone closed) |
| base-functions.kap | ✅ loads clean (port ahead: no const enforcement yet) | becomes strict-clean after P6 sequencing |
| math.kap / math-kap.kap | ✅ loads | residual: user-`⊥` body hits reduce OOB panic (P3-reduce) |
| io.kap | ✅ loads; toHex/fromHex match | needs P4-`∵` for encodeUtf8Char; then full-file oracle diff |
| regex.kap | ✅ loads | lambda-replacement form remains documented-deferred |
| time.kap | ✅ loads | verify exported fns vs oracle (clock/date formats) |
| util.kap | ❌ parse error 6:27 | `labels`, `⊂⍛cols` derived-compose chains, `defsyntax filter` w/ `declare(:local …)` inside — probe each construct standalone first |
| stat.kap | ✅ loads; `median`/`avg` work | `classify` fn (line 20) needs `throw` from base-functions; `⍛⊇` chain FIXED (P1 commit 49f1286/21123ee) |
| map.kap | ❌ parse error 13:6 | `'kap:map ≡ typeof m` symbol compare, `⍺.(⍵)` dynamic member access, `(@.≠)⍛⊂` — map-type surface may be its own mini-phase; consult Kotlin map module before scoping |
| output.kap / output3.kap | ❌ train-parse errors | diagnose against P1's new parser — likely fixed FOR FREE by the accumulator migration; re-test before hand-porting anything |
| http.kap / thread.kap / graph.kap / fhelp*.kap | ❌ | **out of scope** (networking/threads/charting per strategy doc); keep erroring cleanly |

Rule: one file per commit, side-by-side probe table in the commit message,
file marked ✅ in this table only when its exports match the oracle.

## 11. Phase P8 — renderer decision (one-time, then frozen)

The port renders vectors `(1 2)` where the oracle renders `⟨1 2⟩` /
box frames. Today this is a documented DISPLAY-class divergence that inflates
broad-sweep mismatch numbers (~299 mismatches at last baseline, many cosmetic).
Decide ONCE:

- **Option A (recommended):** implement `rendertext.kt`-equivalent boxed
  rendering for REPL/file output behind `--conform-display`, keeping `()` as
  the default house style. Sweep comparisons run conform-mode; humans keep the
  compact style.
- **Option B:** formally accept and stop counting display-only mismatches in
  the headline metric (adjust tally script to classify them).

Either way: value-level comparisons in the harness must already be
display-independent — audit that assumption while here.

## 12. Explicitly out of scope (unchanged)

Networking (`http:`), threads (`thread:`), charting (`chart:`/graph.kap),
GUI, secure-mode library restrictions, `)`-commands (D4). Complex numbers
until P5 completes.

## 13. Completed (archive pointers — details in PROGRESS files)

Tier A kernel (`use`+`⫇`+`,[axis]`+`⍞`), Tier B high-value slice
(structure/math/io/regex/time loading), `⌸` native + stdlib, defsyntax/
unwindProtect subsystem, `⊤⊥` base-value rewrite, rank operator, commute,
bracket indexing, squad rewrite, format family, `≡≢` type-strict match,
take-first, membership scalar shape, modulo arg-order, negative-dim inference,
and/or short-circuit, `⍉` partial-axis. See `PROGRESS-20260815..23.md`.

## 15. Kap syntax ground rules (carried from v1 — user-verified law)

- Inline functions are dfns: `{ … }` with `⍺`/`⍵` as left/right args. No
  parameter names.
- A local function is named with `⇐`: `minus ⇐ -`, `leftPlus5Times ⇐ {⍺ + ⍵×5}`.
- **Functions are assigned with `⇐`, never `←`** (`x ← <fn-value>` errors
  "Right side of the arrow must be a function" on the oracle).
- `λ` is a unary operator over an *existing* function expression
  (`λ {⍺+⍵×5}`, `λ -`). It has **no `λ(x) λ(y) …` form** — that is LISP
  currying and is NOT Kap. (The port tolerates it as an extension; never write
  or probe it as if it were Kap.)
- The `{cond}{a}{b}` three-block guard form is NOT Kap. Real conditionals:
  `when { (cond){body} … }` / `if(cond){a}else{b}`.
- `f ⇐ (g 3)` is not valid Kap — a bare `Apply` is not a function value.

## 16. Definition of Done (every task, no exceptions)

1. Kotlin anchor file read; behavior enumerated BEFORE coding.
2. Oracle transcript captured (pasted, not predicted) for every claimed case.
3. Implementation follows the module-mirror layout; two-gate registration.
4. Gates: lib tests + curated parity green; broad-sweep delta reported honestly
   (known-vs-unknown accounting via `regression_diff.py` when ok-count moves).
5. Curated rows added/updated FROM CAPTURED OUTPUT.
6. PROGRESS entry (today's file) with file:line refs and gate numbers, written
   before the next task starts.
7. Branch invariant restored at sync: `main == strings == origin/*`.


---

## Appendix A — Verified-state synthesis (2026-08-31, analyzer pass)

*This section is the analyzer's synthesis of `PROGRESS-20260828.md`, `PROGRESS-20260830.md`, `KNOWN-NONCONFORMANCE.md`, `PROBLEM.md`, binary gates, and the Kotlin-source/module mirror. It does NOT replace the per-phase definitions above; it is the verified-state index for deciding what remains unsupported and in which dependency order.*

### A.1 Verified gate state (captured from binary, not predicted)
- `cargo test -p kap-core --lib` → 96 passed / 0 failed (`PROGRESS-20260830.md`).
- `cargo test -p kap-core --test conformance curated_kap_parity` → 1 passed / 0 failed (`PROGRESS-20260828.md` + `PROGRESS-20260830.md`).
- Broad sweep (`# [IGNORE]`d per `rust-kap-dev` SKILL.md §10): `OK 1606 / MISMATCH 185 / UNSUPPORTED 746 / COVERAGE 70.2%` (`PROGRESS-20260830.md` final block; `KNOWN-NONCONFORMANCE.md`: 1774 OK / 24 MISMATCH / 747 UNSUPPORTED at baseline `d6372ef`; the sweep numbers differ because of the `# [IGNORE]`d harness — the curated parity and the 185 MISMATCH figure are the authoritative live-state metrics).

### A.2 Corrected stale ROADMAP claims (verified against binary/progress)
- P4 `⍣` (power operator) and `∵` (bitwise family) were marked "Not started" / "P4 blocker" in v2 (`§4`, `§7`, table line 82). Both are implemented: `⍣` (`PROGRESS-20260824.md`: integer-iterate + `f⍣g` until-loop); `∵` (`PROGRESS-20260830.md`: corrected `adverb_bitwise` table, `popcount_bigint`, recursive monadic, 26 verified cases including `⍸∵`).
- P5 numeric tower (`§5`) marked partial for rational/`!`; `!` (gamma) uses `libm::lgamma` per skill; rational (`¯1r2`) works; complex deferred (locked design choice `D2`). Status: **partial — complex deferred by design**.
- P6 const enforcement (`§6`) marked open; implemented (`PROGRESS-20260825.md`: `⎕A ⎕a ⎕d` native + `declare(:const)` enforcement). Status: **CLOSED**. P6 sequencing respected (`consts` before refreshing `base-functions.kap`).
- P8 renderer (`§8`) marked decision-stage; adopted Option A (`PROGRESS-20260828.md`: `format_conform()` + `--conform-display`; default `()` preserved; `e037bac` format-value fix). Status: **CLOSED**.
- P7 stdlib (`§7`, table): `util.kap` still `❌` (current `PROBLEM.md`); `map.kap` `❌` (`PROBLEM.md` category 6 `s:col`); `output.kap`/`output3.kap` `❌` (likely P1-accumulator fixed, needs re-probe); `http.kap`/`thread.kap`/`graph.kap`/`fhelp*.kap` out of scope (confirmed by `RUST_REWRITE_STRATEGY.md` §1.2). Status: **PARTIAL**.
- P3 structural (`§6`, file-by-file table): `reshape.kt` `MATCH`/`FILL`/`TRUNCATE`/`RECYCLE` keywords (`:match`/`:fill`/`:truncate`/`:recycle`) still incomplete (`KNOWN-NONCONFORMANCE.md`: only `⍬⍴` fixed this session; other spec modes not wired); `reduce.kt` user-`⊥` OOB panic (`P3-reduce` reference, `KNOWN-NONCONFORMANCE.md`: `⊥` encode/decode 22 unsupported cases reference it); `concatenate-array.kt` `,[axis]` general + `,[0.5]` laminate (`base-functions.kap` line 5) — `join_by_axis` has the `name:` param (commit `14028ba`) but laminate path needs verification. Status: **PARTIAL — reshape spec keywords + reduce-⊥ panic are the structural gaps.**

### A.3 Logical-block clearing order (optimal, dependency-driven)
*Derived from `KNOWN-NONCONFORMANCE.md` unsupported clusters (747 total) grouped by dependency and impact on downstream stdlib files (`util.kap`, `map.kap`, `output3.kap`, `io.kap`, `base-functions.kap`). This is the analyst's recommendation — not executed, not committed as a plan file (per user: deliver analysis only, no new `PROGRESS-YYYYMMDD.md` created). The user may adopt/reject/reorder.*

**Tier 1 — parser-level structural (unblocks largest unsupported clusters):**
1. `f[axis]` axis-applied syntax for ALL verbs (`parser.rs` axis allowlist + `eval_apply` axis-dispatch; `evaluator.rs` axis-aware builtins). Cluster: 75 unsupported (`bracket-axis`: `,` 5, `⊃` 10, `⊂` 15, `∊` 7, `\` 2, `/` 7, labels 21, sort 6, scalar-ops 12). **Single highest-impact fix.**
2. Qualified-name / namespace lookup (`.field` dynamic member access + `s:col` qualified names + `map:with` + `kap:map` type symbol). Cluster: 44 unsupported (`.` member deref) + the current `PROBLEM.md` `util.kap` parse error (category 1/2/3 + `s:col` open item). Unblocks `map.kap` and the 5 `util.kap` other pre-existing cases (category 6 of `PROBLEM.md`).
3. `labels` verb (full get/set/read, not just `labels[n]` axis-form; plus label-preservation through untested structural ops). Cluster: 40 unsupported (`labels` section of `KNOWN-NONCONFORMANCE.md`); `PROBLEM.md` `labels_parity.rs` skeleton must be completed to verify. The parser edit (`nested_right_is_fn_result` removal) fixes `labels[0]` axis-form; the remaining 40 cases require completing the skeleton and verifying label-threading through all structural ops (reverse/rotate, take/drop, catenate, transpose, bracket-index, replicate/compress, expand — 5 commits `7b07cb6`/`0b58190`/`89499f8`/`f2cdd53`/`df65df9` cover the threading; the 49 `LabelsTest` failures are framework-level).

**Tier 2 — evaluator structural refinements (self-contained):**
4. Reshape spec keywords (`:match`/`:fill`/`:truncate`/`:recycle`). Cluster: ~4 unsupported (`reshape.kt` `findSizeCalculationMethod`); affects `base-functions.kap` laminate (`,[0.5]`). Independent of parser; no multi-file dependency.
5. Reduce `⊥` user-body OOB panic (`P3-reduce`, `reduce.kt`). Cluster: 56 unsupported (`encode/decode`); single evaluator path guard (`reduce_1arg` / `reduce_2arg` loop bound). Removes panic + opens `⊥`-related match.
6. Complex numbers — 48 unsupported (`complex` section). **DEFERRED per locked design (`ROADMAP.md` §9.3, `RUST_REWRITE_STRATEGY.md` D1-D4): deferred until P5 completes.** Lowest win-per-effort; no downstream dependency.

**Tier 3 — stdlib verification (depends on T1 + T2):**
7. `util.kap` — verify after T1.1 (`f[axis]`) + T1.2 (`.`/namespace) complete. `PROBLEM.md` category 1 (32 conformance extractions) is framework-level, not engine-level.
8. `map.kap` — verify after T1.2 complete (`.` + qualified names). `PROBLEM.md` category 6 (`inner dfns`, `s:col`, null) includes the qualified-name gap; the `map.kap` parse errors (line 13:6) depend on namespace lookup.
9. `output.kap` / `output3.kap` — verify after T1 (parser M6) confirmed; likely resolved by P1 M6 (accumulator default). `PROBLEM.md` notes output3.kap line 146 (`math.kap`/`http.kap`/`map.kap`/`fhelp.kap`) as pre-existing stdlib gaps.

**Tier 4 — lower-priority clusters (no downstream dependency):**
10. Encode/decode (`⊥`/`⊤`) remaining edge cases (non-integer args, bigint overflow, negative-dim inference) — 22 unsupported (`encode/decode` section). Covered by T2.5 (`⊥` body guard).
11. Numbers / float edge (`1.2 4.7 + 2 0x...`, `int:ensureGeneric`, hex literals, overflow, `-¯922337…`) — 18 unsupported. Covered by P2 scalar fixes (`PROGRESS-20260830.md`); remaining cases are harness-level.
12. Adverb / compose with left args (`⍢` + left-bound; `∘`/`⍛` no-inverse) — 30 unsupported (`adverb/compose` section). `PROGRESS-20260830.md` (`˝` repair) + `PROGRESS-20260824.md` (`⍤`/`⍣`/`∵`) cover core cases; remaining are edge forms (`{,100}⍢(6↑)⍳3`, `(0 1↓)⍢(2↑) 5 4 ⍴ ⍳20`).
13. Multi-line `∇` (3 unsupported: `∇ foo x {`, `∇ foo (X) {`, `∇ foo (X) {`) + assignment errors (3 unsupported: `a←4 ◊ { declare(:local a) }`, `foo bar←10`, `(a b c) ← 3 2 ⍴ …`). Independent; parser-defsyntax path (`syntax/syntax.kt`).
14. Lambda bare (`λfoo`) — 1 unsupported. Parser `is_function_expr` + `is_known_fn` gap; independent.
15. Namespace (`namespace("foo")`) — 1 unsupported. Independent; `declare(:const)` already closed (P6).
16. Transpose (`0 0⍉˝2 3⍴ 1 2 3 4 5 6`) — 1 unsupported. Independent; P3 partial-axis covered most (`PROGRESS-20260821.md`).
17. `→` return under operator (`{S⇐→ ⋄ …}`) — 2 unsupported (`return/→` section). Independent; depends on `→` primitive dispatch (`evaluator.rs` `eval_return`).

**Summary of logical clearing order (recommended):**
`T1.1 (f[axis] 75) → T1.2 (.`/namespace 44) → T1.3 (labels skeleton 40) → T2.1 (reshape spec 4) → T2.2 (reduce ⊥ panic 56 + decode/decode 22 combined) → T2.3 (complex deferred 48 — design-locked, lowest priority) → T3.1 (util.kap) → T3.2 (map.kap) → T3.3 (output3.kap) → T4 (remaining lower-priority: multi-line ∇, assignment, lambda bare, namespace, transpose, return, adverb edge, float/number edge)`.

This targets the three largest unsupported clusters first (75+44+40 = 159 unsupported, ~21% of 747) and unblocks the two blocked stdlib files (`util.kap`, `map.kap`) before addressing the smaller self-contained clusters.

---

## Appendix B — Verification references captured in this session
- Gates (`PROGRESS-20260830.md`): lib `96/0`; curated `1/0`; build clean (`cargo build -p kap-cli` finished 53.97s).
- Binary probes (`~/Apps/array/kap-jvm-text/bin/kap-jvm-text` vs `./target/release/kap` rebuilt at `df65df9`/`f99a5f5`): `2 3 ⍴ ⍳100` → oracle 2×3 array `(0 1 2 / 3 4 5)` vs port `(6 6 6 6 6 6)` pre-edit; `(λ↑) 5` → `⟨function 5⟩`; `a←λ↑ ⋄ a 5` → `(function 5)`; `12 {⍵+8×⍵≥0}⍢- 11 12 13 14` → `(3 4 13 14)`; `a←λ- ⋄ 8 ⍞a⍨˝ 7` → `15` (post-edit binary).
- Parser edit (`git diff kap-core/src/parser.rs` at HEAD `df65df9` vs edited `f99a5f5`): removed `nested_right_is_fn_result` branch at `parser.rs` ~1364; replaced with `parse_value_kotlin()`; `labels_parity.rs` skeleton (59 lines, untracked, 24 of 25 cases `"None"` placeholder).
- `META-INF/code_analysis_07.md`: 10,625 bytes (after append of interpretive section from implementor-thinking excerpt). Contains the `evaluator.rs:5382` error-site reference (`reshape dimensions must be integers`) with the trace showing `left_args = [2, 3]` builds `Instr::Array` correctly (line 1404) — confirming the parser-stranding is NOT the root cause; the error comes from the label-evaluation sequence feeding non-integer dimensions into reshape, not from broken `Instr` construction.
- `PROBLEM.md` (current session): Labels Implementation Status — 49 remaining `LabelsTest` failures; 3 open categories (`iota-reshape`, `axis-applied statement start`, `s:col` qualified name); 1 skeleton parity test; parser edit uncommitted; no `PROGRESS-YYYYMMDD.md` written this session.
- `KNOWN-NONCONFORMANCE.md` (`d6372ef` baseline): 1774 OK / 24 MISMATCH / 747 UNSUPPORTED (69.7%); `PROGRESS-20260830.md` reports sweep `OK 1606 / MISMATCH 185 / UNSUPPORTED 746` (70.2%) — the difference from the `KNOWN-NONCONFORMANCE.md` baseline reflects the sweep harness's `#[IGNORE]`d state (`rust-kap-dev` skill) and cosmetic display differences (`FORMAT` class divergence, P8 `--conform-display` adopted). The MISMATCH count (185 vs 24 in `KNOWN-NONCONFORMANCE.md`) is the live metric; the 24 MISMATCH in `KNOWN-NONCONFORMANCE.md` is the curated/regression-level divergence (value-correct bugs), not the broad-sweep cosmetic count. Both are tracked separately (per ROADMAP §0.1 laws 4 and 5).
