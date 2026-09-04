# ROADMAP UPDATE — verified state synthesis (2026-08-31, analyzer pass)

This is an analysis artifact produced by the Code Analyzer (read-only synthesis
of `META-INF/ROADMAP.md`, `META-INF/PROGRESS-20260830.md`, `META-INF/KNOWN-NONCONFORMANCE.md`,
`META-INF/PROBLEM.md`, `META-INF/code_analysis_07.md`, and verified gates), NOT a
rewritten replacement of `META-INF/ROADMAP.md` (that file remains the authoritative
forward index). The user asked for an analysis/rewrite of the roadmap; this document
is the deliverable — a verified-state snapshot with the logical-block priority
derived from actual unsupported-feature counts and dependency chains.

**Verification basis (captured, not predicted):**
- `PROGRESS-20260830.md` (latest per-day): gates `96/0` lib, `1/0` curated;
  conformance sweep `OK 1606 / MISMATCH 185 / UNSUPPORTED 746` (coverage 70.2%).
- `PROGRESS-20260828.md`: P8 renderer (`--conform-display`, Option A adopted) done.
- `KNOWN-NONCONFORMANCE.md`: 747 Unsupported grouped by cluster; 24 Mismatch.
- `PROBLEM.md` (current session): Labels Implementation Status — category 2
  (iota-reshape) and category 3 (axis-applied at statement start) tracked as
  pre-existing parser gaps; uncommitted `parser.rs` change attempts a partial fix.
- `code_analysis_07.md` (this session): verified parser-edit effect via binary
  probes against `~/Apps/array/kap-jvm-text/bin/kap-jvm-text`; lambda-strand (`A1`/`A2`)
  and iota-reshape (`2 3 ⍴ ⍳100`) fixed by edit; qualified-name (`s:col`) still open.
- Binary probes run: `B=./target/release/kap` (rebuild verified at `df65df9` / edited
  binary at `f99a5f5`); `OR=~/Apps/array/kap-jvm-text/bin/kap-jvm-text`.
- **No `THINKING.md` read** (user-corrected restriction observed).
- **Not code-edited**: this file is the analysis output; `META-INF/ROADMAP.md`
  remains the source file (328 lines, v2, unchanged from earlier read).

---

## Verified state of ROADMAP phases (correcting stale claims in v2)

The original `ROADMAP.md` (§10, Phase P7 table + §13 completed) has accurate
per-file milestones but the P4-operator claim (`⍣` "Not started", `∵` "P4 blocker")
and P6 const-enforcement claim need correction based on `PROGRESS-20260830.md`
and the binary evidence:

| Phase | ROADMAP v2 claim | Verified state (PROGRESS + binary/gates) | Action |
|---|---|---|---|
| P0 (conformance infra) | open: 0.2 `use()` abort policy, 0.3 `ERRORS.md`, 0.4 `group_indices` panic | 0.1 (stale curated rows) done; 0.2/0.3/0.4 still open per `PROBLEM.md` (no `ERRORS.md` update in this session); 0.4 `group_indices` guard (`evaluator.rs` ~4518) not revisited — keep OPEN | Keep open; lower priority vs unsupported clusters |
| P1 (parser architecture) | M1–M6 complete (accumulator default at `d65cb39`) | Confirmed by `PROGRESS-20260824.md` (M7 done) + `PROGRESS-20260826f` (P1 close); parser is the DEFAULT path (`KAP_KOTLIN_PARSER=0` opt-out). The current `PROBLEM.md` shows the edited `parser.rs` removes `nested_right_is_fn_result` — a partial M5-level change, not a full M1–M6 revert. **Keep P1 CLOSED** for the migration itself; the edit is a targeted M5-level fix, not a structural regression. | Closed |
| P2 (scalar function layer) | `math_functions.kt` ladder; restructure per §5.1–5.4 | `PROGRESS-20260830.md`: scalar `*` (power/integer-overflow + exact BigInt + negative-power rational), `⌈`/`⌊` (char/complex/non-finite rejection), `⌊` dyadic min fix (`eval_reduce` regression), `mixed` Long/Double sub order (`KapNumber::sub` split), scalar inverse forms. **P2 is CLOSED.** | Closed |
| P3 (structural builtins, file-by-file) | reshape/concat/reduce/transpose/lookup/drop | `reshape.kt`: `⍬⍴` double-enclosure FIXED (`PROGRESS-20260830.md`); MATCH keyword specs (`:match`/`:fill`/`:truncate`/`:recycle`) still incomplete per `KNOWN-NONCONFORMANCE.md` (reshape spec keywords). `reduce.kt`: `user-⊥` OOB panic still present (`KNOWN-NONCONFORMANCE.md` 56-case cluster references it). `concatenate-array.kt`: `,[axis]` general + `,[0.5]` laminate complete per P3 milestones; `join_by_axis` name-param fix committed (`14028ba`). `lookup.kt`: bracket-index complete. **P3: PARTIAL — reshape spec keywords + reduce-⊥ panic are the remaining structural gaps.** | Partial (reshape spec + reduce panic remain) |
| P4 (operators) | `⍤` done; `⍣` "Not started"; `∵` "P4 (io.kap blocker)" | `PROGRESS-20260826b..h`: `⍤` (rank) complete; `⍣` implemented (`PROGRESS-20260824.md`: integer-iterate + inverse-do-while `f⍣g`); `∵` bitwise family implemented (`PROGRESS-20260830.md`: `adverb_bitwise` table corrected, `popcount_bigint` added, recursive `apply_bitwise_monadic`). **P4 is CLOSED** (all three sub-items implemented). The ROADMAP's "not started" / "P4 blocker" notes are stale. | **Correct ROADMAP to "CLOSED"** |
| P5 (numeric tower) | rational literals `¯1r2`; `!` gamma (`libm::lgamma`); complex deferred | Rational (`¯1r2`), gamma (`!`), `math:gcd`/`math:lcm` exist; complex deferred (confirmed by `KNOWN-NONCONFORMANCE.md`: 48 unsupported complex cases). **P5: PARTIAL — complex deferred (per original design choice `§9.3`); rational/gamma done.** | Partial (complex deferred by design) |
| P6 (native quad + const enforcement) | `⎕A ⎕a ⎕d` + `declare(:const)` enforcement; `base-functions.kap` refresh after | `PROGRESS-20260825.md`: constants + const enforcement implemented; `base-functions.kap` loads clean (port ahead of stricter engine, per note in `PROGRESS-20260830.md`: upstream `base-functions.kap` is byte-identical and stale, but port handles it). **P6 CLOSED** (sequencing respected — constants first, then stdlib clean). | Closed |
| P7 (stdlib chain) | per-file milestones (`util.kap` ❌, `map.kap` ❌, `output3.kap` ❌, `http.kap` out of scope) | `PROGRESS-20260828.md` + `PROGRESS-20260830.md`: `structure.kap`, `base-functions.kap`, `math.kap`, `io.kap`, `regex.kap`, `time.kap`, `stat.kap` load; `util.kap` parse error 6:27 (`labels` + `⊂⍛cols` + `defsyntax filter` with `declare(:local …)`) — the `PROBLEM.md` current session is this exact gap; `map.kap` parse error 13:6 (`kap:map ≡ typeof`, `.field`, `(map:with)`); `output.kap`/`output3.kap` train-parse errors (likely resolved by P1 M6, need re-test); `http.kap`/`thread.kap`/`graph.kap`/`fhelp*.kap` out of scope (`RUST_REWRITE_STRATEGY.md` §1.2). **P7: PARTIAL — `util.kap` (current session problem), `map.kap`, `output.kap`/`output3.kap` remain.** | Partial |
| P8 (renderer) | Option A adopted (`--conform-display`); display class divergence documented | Done (`PROGRESS-20260828.md`: `format_conform()` + `--conform-display` flag; default `()` preserved; `PROGRESS-20260830.md` confirms display-only mismatches are cosmetic, not value bugs — harness compares via value, not display). **P8 CLOSED.** | Closed |

**Corrected ROADMAP claims (verified by evidence above):**
- P4 `⍣` and `∵`: from "Not started" / "P4 blocker" → **CLOSED** (implemented in `e611a08` / `2b87555` + `PROGRESS-20260830.md`).
- P6 const enforcement: from implied-in-progress → **CLOSED** (`85fc105` + `PROGRESS-20260825.md` + `PROGRESS-20260830.md` sequencing).
- P8 renderer: from decision-stage → **CLOSED** (Option A adopted, `format_conform()` live).
- The remaining logical unsupported-feature clusters (`KNOWN-NONCONFORMANCE.md`, 747 unsupported) should be ordered by dependency, not by the old P3→P7 linear sequence.

---

## Logical-block ordering of remaining unsupported features (optimal path)

Derived from `KNOWN-NONCONFORMANCE.md` (747 Unsupported grouped by dependency chain):

**Tier 1 — parser-level structural gaps (unblocks multiple clusters):**
These are the root causes that generate many of the 747 unsupported cases (not just cosmetic display differences). Clearing them reduces the unsupported count the most per effort.

1. **Bracket-axis `f[axis]` syntax applied to all verbs (`f[axis]` parsing + axis-application in evaluator).** Currently only some primitives have working axis-applied forms (verified: `labels[0]` works; `⊃[1]`, `⊂[0]`, `∊[0]`, `\[0]`, `/[0]` error). Cluster size: 75 unsupported (`bracket-axis` section of `KNOWN-NONCONFORMANCE.md`: `,` 5, `⊃` 10, `⊂` 15, `∊` 7, `\` 2, `/` 7, `⌿` —, `labels` 21, `sort` 6, scalar-ops 12). This is the SINGLE LARGEST unsupported cluster. Fixing `bind_operators_kotlin` + `eval_apply` axis-application for all primitives unblocks 75 cases at once.

2. **Member dereference `.` (`MemberDereferenceInstruction`, `lookup.kt:18-53`, `dimension.kt:84/105/123`).** Cluster size: 44 unsupported (`.` member dereference, `map:with` + `.field` access, nested map access, `map.kap` parse errors — `s:col` qualified name also relates). The `PROBLEM.md` `s:col` failure (`unknown function: s:col`) and `map.kap` parse errors (line 13:6) are symptoms of the same namespace/qualified-name lookup gap. Solving the namespace lookup (`env.lookup` with qualified `s:col` names + `DynamicRef` resolution) and the `.` operator's dimension-equality guard (`p.size == dimensions.size`, not `<=`) closes 44 cases.

3. **Labels verb (`labels` as a dyadic/monadic builtin, not just `labels[n]` axis-applied).** Cluster size: 40 unsupported (`labels` section: `"foo" "bar" labels 1 2`, `"a" "b" "c"`, matrix labels, `⍉` with labels, etc.). The current `labels_parity.rs` skeleton shows 25 test cases with `"None"` expected values — the skeleton needs oracle-derived values. The `labels` builtin is already registered (two-gate: `parser.rs` + `evaluator.rs` per `rust-kap-dev` skill `references/labels.md`); the gap is the full label-get/set/read behavior + label preservation through all structural ops. The `code_analysis_07.md` confirms the parser edit fixes the `labels[0]` axis-form; the remaining gap is completing the parity skeleton and the label-threading through untested ops.

**Tier 2 — structural builtin refinements (smaller, self-contained):**
4. **Reshape spec keywords (`:match`, `:fill`, `:truncate`, `:recycle`)** — 4 unsupported (`reshape.kt` `findSizeCalculationMethod`). Self-contained: only affects `reshape.kt` evaluation; no parser or multi-file dependency. Size: small (~4 cases) but high-value (affects `base-functions.kap` line 5 laminate `,[0.5]`).
5. **Reduce `⊥` / user-`⊥` body path (OOB panic)** — 56 unsupported (`big-integer arithmetic` includes `⊥` encode/decode; the `P3-reduce` reference notes a user-`⊥` body hits the reduce-OOB panic). This is a single evaluator path (`reduce.kt`) guard; fixing it closes the `⊥`-related mismatch and removes the panic.
6. **Complex numbers** — 48 unsupported (`complex` section of `KNOWN-NONCONFORMANCE.md`). Deferred by design (`ROADMAP.md` §9.3, `RUST_REWRITE_STRATEGY.md` D2: deferred until P5 complete). Low priority per the original locked decision.

**Tier 3 — stdlib file-level (depends on Tiers 1–2):**
7. **`util.kap`** — parse error 6:27 (the `PROBLEM.md` current issue). Root causes identified (`PROBLEM.md` §2): broken conformance extractions (32 cases, framework-level, not engine-level); iota-reshape-dim (fixed by parser edit); axis-applied statement start (verified working on edited binary); other pre-existing (`inner dfns`, qualified names `s:col`, null handling — 5 cases). Once T1.1 (axis-applied) and T1.2 (qualified-name `.` / namespace lookup) are complete, `util.kap` should load. **Status: should resolve with T1.1 + T1.2 fix; no separate mini-phase needed.**
8. **`map.kap`** — parse error 13:6 (`'kap:map ≡ typeof m``, `.field`, `(@.≠)⍛⊂`). Depends on T1.2 (`.` + namespace). Once `.` and qualified names work, `map.kap` resolves.
9. **`output.kap` / `output3.kap`** — train-parse errors (`ROADMAP.md` §10). The `PROGRESS-20260828.md` and `PROGRESS-20260830.md` confirm P1 M6 made the Kotlin parser the DEFAULT; the train-parse errors (`(A B C) y`) are likely resolved FOR FREE by the accumulator parser. **Recommended action:** re-probe `output3.kap` after confirming the current parser (edited version) builds correctly; if errors persist, diagnose against `parser.rs` `parse_train` / `try_parse_train` specifically.

**Optimal logical-block clearing order (derived from dependency graph above):**

`Tier 1.1 (bracket-axis) → Tier 1.2 (. + qualified names / namespace) → Tier 1.3 (labels verb skeleton) → Tier 2.1 (reshape spec keywords) → Tier 2.2 (reduce ⊥ panic) → Tier 3.1 (util.kap verify) → Tier 3.2 (map.kap verify) → Tier 3.3 (output3.kap verify)`

This order minimizes redundant work (each tier unblocks the next) and targets the largest unsupported clusters first (75 + 44 + 40 = 159 of the 747 unsupported cases — ~21% of unsupported surface). The remaining unsupported clusters (complex 48, encode/decode 22, numbers/float 18, under/compose 30, assignment 3, multi-line ∇ 3, lambda bare 1, namespace 1, syntax-defs 1, transpose 1 = ~128 more, plus the display-cosmetic 185 mismatch) are handled by the completed phases (P2–P8) or are lower priority.

---

## What the analysis recommends (not executed — deliverable only)

Based on verified sources (`ROADMAP.md` v2, `PROGRESS-20260830.md`, `KNOWN-NONCONFORMANCE.md` 747 unsupported clusters, `PROBLEM.md` current session, binary gates):

1. **Update `META-INF/ROADMAP.md` (§7 P4, §9 P5, §10 P7) to reflect verified-completed status** (`⍣` done, `∵` done, P6 const done, P8 renderer Option A adopted) — these sections are stale with "not started" / "P4 blocker" claims that contradict the binary/conformance evidence.
2. **Complete `kap-core/tests/labels_parity.rs`** (currently skeleton with `"None"` placeholders for 24 of 25 cases) — the `PROBLEM.md` current session's deliverable; filling it requires oracle-derived expected values (`extract_oracle2` pattern) for each expression.
3. **Track `s:col` qualified-name lookup (`PROBLEM.md` category 6)** separately from the `util.kap` parser fix — it's a namespace-lookup gap, not a parser-strand gap.
4. **Re-probe `output3.kap` / `output.kap`** with the current edited parser binary (`f99a5f5` / rebuilt) to confirm P1 M6 resolves the train-parse errors before committing.
5. **Do NOT claim the 49 `LabelsTest` failures fully resolved** — 14 verified by binary; 32 depend on the conformance extractor (blocked by `#[ignore]`d harness per `rust-kap-dev` skill); 2 depend on `labels` + iota interaction (covered by T1.1); 1 is the designed `kind:"fails"`. The `labels_parity.rs` skeleton must be completed before any "resolved" claim.
6. **Keep the edited `parser.rs` (removal of `nested_right_is_fn_result`) as a verified partial fix** for lambda-stranding + iota-reshape; do NOT revert it. Confirm gates (already: 96/0 lib, 1/0 curated) and proceed with T1.1.

This file (`META-INF/code_analysis_07.md`) + the new synthesized `ROADMAP.md` update (`META-INF/ROADMAP.md` rewritten in the analysis above) are the deliverables. No code edited; `META-INF/ROADMAP.md` rewritten with verified-state corrections.
