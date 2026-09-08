# HANDOFF PROMPT — rust-kap (2026-09-08, 94.0% milestone, pushed)

**Project:** `~/Apps/array/rust-kap`, branch `feature/wheres-extra` = `08f1b76`, pushed to `origin/feature/wheres-extra` (in sync). Rust rewrite of the Kap array language; goal was 94% conformance vs the Kotlin oracle — **MET**.

**TRUTH SOURCE (do not violate):**
- Oracle: `~/Apps/array/kap-jvm-text/bin/kap-jvm-text --no-standard-lib --no-lineeditor` (a.k.a. `$ORACLE`). Kotlin source (READ only): `~/Apps/array/array`.
- Port: `./target/debug/kap --no-standard-lib /dev/stdin` (a.k.a. `$PORT`).
- Stale-binary rule: after ANY parser/evaluator edit, `cargo build -p kap-cli` (NOT `-p kap-core` alone). Symptom check: `git stash && cargo build -p kap-cli && probe && git stash pop`.
- Gates (must stay green): `cargo test -p kap-core --lib` (111/0), `cargo test -p kap-core --test conformance curated_kap_parity` (1/0).
- Invariant: **0 MISMATCH** — never commit with a mismatch; land it or leave UNSUPPORTED.
- Sweep: `bash run_conformance_tests.sh` → 2482 OK / 0 mismatch / 159 unsupported = **94.0%**.
- Env quirk: bare `python3` resolves to a broken conda shim — use `/usr/bin/python3`.
- User style: terse, immediate action; edits via `patch` tool, avoid python-script edits.

---

## CURRENT STATE (pushed, clean tree)

- **HEAD `08f1b76`** `fix(scan/factor/numerator/innerjoin/union/lexer/pick/laminate/ravel): +26 OK, 94.0%, 0 MISMATCH` — 3 files, +405/-114.
- NOTE: `main`/`strings` NOT yet re-synced to `08f1b76` (invariant `main == strings == origin/*` via `git branch -f strings main && git push origin main strings` was deferred; only `feature/wheres-extra` was pushed this time).
- Session arc 89.8% → 94.0%: `490aa98` (under compose/reverse-compose/bare-pick/commute), `4a71d76` (Str rank-1, `|` hypot, cell-wise `⌊⍢|`, null-take), `08f1b76` (this batch, below).

## What 08f1b76 did (all oracle-verified, Kotlin refs in comments)

1. **Scan scalar/empty** (evaluator.rs ~13624): `+\\ 10 → 10`, `+\\ 0⍴0 → ⍬` (Kotlin `ScanFunctionImpl.eval1Arg`, reduce.kt:414 — rank-0/empty returns `a`).
2. **math:factor** (~17060): element-wise over arrays, BigInt trial division, integer-Rational accepted, 1→[1]/0→⍬ (mpmaths `defaultFactorLong`), `bigint_to_kap` reduction, `make_simple_or_nested` output.
3. **math:numerator** (~3430): element-wise over arrays (`MathCombineAPLFunction.combine1Arg`).
4. **Inner join rewrite** (~4990): Kotlin `InnerJoinResult` semantics — fn1 on whole axis-VECTORS then fn0-reduce, B-stride `multipliers[0]`, `scalarOrOneElementVector` stretch (outer_join.kt:230-256). Fixed 4 panics/errors. Also corrected a WRONG curated expectation: `(2 2⍴1 2 3 4)+∙×…` is `((7 10)(15 22))` per oracle, not `((5 11)(11 25))` (conformance.rs:865).
5. **Union/intersection** (~18579/18690): enclosed rank-0 B discloses to compound cell (unique.kt:20-40), `setop_rows_from_elems` ragged-vs-flat handling, `⍬`-right (`[0]` dims) → A unchanged for ∪.
6. **Lexer** (lex_helpers.rs): decimal-rational `2.5r → 5/2` (tokeniser.kt `([0-9]+)(\.([0-9]*))?r$` trailing-r form via `decimal_str_to_rational`), `¯0x/¯0b` sign (`^(¯?)0x…$`).
7. **Disclose pick** (`reveal` ~12080): `indexFromPosition` semantics incl. negatives (disclose.kt:474), `⊃`-prefixed bounds errors (kind:"fails" cases count OK only on error).
8. **Laminate**: `isScalar()` is `rank == 0`, so enclosed `⊂"foo"` resizes (concatenate-array.kt:133).
9. **Ravel-under**: `wa=ravel(a); updated=base(wa)` same-dims required; reshape to `a.dims` (concatenate-array.kt:606).
10. **Empty-pick under**: `⍬ {1}⍢⊇ 2 3 4 → (2 3 4)` but `⍬ {10 11}⍢⊇…` ERRORS (PickTest pair; `replaceForUnder` shape check, lookup.kt:222).

## Remaining unsupported (159) — triaged

- **Out of scope ~70** (needs JVM libs/files/stdlib the harness doesn't load — skip): SQL 21, JavaTypes 13, Json 9, IOAPL 9, Xml 8, Csv 5, JvmMethodCalls 5, Calcite 4, Lock 4, time: 7, html/arrow/encoder, stdlib-only (`⌹`, `io:`, `math:pi`, `o3:format`, `s:col`, `⌸`+stdlib, `${…}` extractor artifacts).
- **Fixable parser/semantics clusters** (next work if pushing past 94%): `+[1]/` axis-on-fn vs `+/[1]` reject; strand assignment `b((b←2)+10)→(2 12)`; `;`-lists; `namespace("foo")` call-form; `λ((n-)˝)` escape (λ-paren path rejects Derived); closures `({⍵×3}⍢(z↑))¨` parse at 1:22; `√⍨˝` commute-inverse; `2 {…} 5` z←⍺ under-take (oracle itself errors overflow — verify first).
- **Do-not-touch**: EncodeTest `⊤` big-int (oracle without stdlib also errors — implementing risks Mismatch); `math:sin/⍬` (oracle errors no-identity, harness-limited); `⊃ 200000⍴…` (expects OOM exception type).
- Dump drill-down: `CONFORM_DUMP=1 CONFORM_DUMP_PATH=/tmp/dumpN.txt cargo test --jobs 1 --test conformance -- --nocapture`; case mine via `/usr/bin/python3` over `conformance/kotlin_tests.jsonl`.

## Key functions (evaluator.rs)

`apply_under_op` (~14300), `under_pick_selector` (~15305), `pick_flat_positions` (~15339), `resolve_under_wrapper`, `apply_inner_product` (~4932), `frame_cells`, `build_nested`/`nest_rows`/`vec_to_value`, `union` (~18543), `intersection` (~18693), `setop_rows`/`setop_rows_from_elems`/`disclose_cells`, `reveal` (~11907), `reveal_axis`, `math_factor` (~17060), `adverb_scan` (~13610), `adverb_reduce`, `value_to_instr`, `is_value`. Lexer: `lex_number`/`parse_kap_number`/`decimal_str_to_rational` (lex_helpers.rs).

## Suggested next goal (not started)

Past 94% = parser work (`+[1]/`, `λ((n-)˝)`, strand-assign, `;`-lists) — each needs Kotlin parser.kt ground truth + oracle probe before coding. Or declare 94% terminal and sync `main`/`strings`.
