# HANDOFF PROMPT — rust-kap (2026-09-04, post T1.1 step 1–3 + T1.2)

**Project:** `~/Apps/array/rust-kap` (branch `feature/wheres-extra` = `main` = `strings` = `6e53c89`, pushed to origin). Rust rewrite of the Kap array language. Goal: extend conformance against the Kotlin oracle `~/Apps/array/kap-jvm-text/bin/kap-jvm-text`.

**TRUTH SOURCE (do not violate):**
- Oracle binary: `~/Apps/array/kap-jvm-text/bin/kap-jvm-text` (a.k.a. `$ORACLE`).
- Port binary: `~/Apps/array/rust-kap/target/debug/kap` (a.k.a. `$PORT`).
- Kotlin source (READ only): `~/Apps/array/array`.
- **Stale-binary rule (critical):** after ANY parser/evaluator edit, `cargo build -p kap-cli` (NOT `-p kap-core` alone). If symptoms look impossible, `git stash && cargo build -p kap-cli && probe && git stash pop` (skill `rust-kap-dev` calls this the stale-binary discriminator).
- Gates: `cargo test -p kap-core` → 137 passed / 0 failed across 11 binaries. `cargo test -p kap-core --test conformance` for the conformance sweep (slow, ~22s).
- User directive: **document to `META-INF/PROGRESS-<date>.md` before continuing** (file:line + oracle-vs-port result per finding).

---

## CURRENT STATE (pushed, clean tree)

- **HEAD `6e53c89` on `feature/wheres-extra` / `main` / `strings` (all equal).**
- 12 unpushed-then-now-pushed commits since `f99a5f5`:
  - `7b07cb6…df65df9` (5 commits) — labels threading through catenate/transpose/bracket-index/replicate/expand.
  - `bd7cc33` + `fdce3fe` + `c87828b` — T1.2 MemberDereference cluster (60/60 closed).
  - `0c09c80` + `00d23fa` + `27821c4` — T1.1 step 1–3: parser allowlist 12→18 verbs; `⊂[axis]` + `∊[N]` arms; `/[axis]` + `⌿[axis]` direct-verb arms.
  - `23b217a` — T1.2-2 fix: symbol-as-index routes through `extract_column_by_label`; value/name form split (closes 5 conformance cases incl. R2 `{a.(⍵)}¨ (⍳10)-10`).
  - `6e53c89` — doc/test bookkeeping: PROBLEM.md rewrite, ROADMAP.md verified-state Appendix A, `code_analysis_07.md` synthesis, 7 new parity test files, 2 expected-value corrections in `kotlin_tests.jsonl`.
- **Tests: 137/0 across 11 binaries.** Verified with `cargo test -p kap-core`.

## ACTIVE TASK — T1.1 bracket-axis cluster (~36% closed per d1 audit, 89 cases)

**Closed (steps 1–3, in 0c09c80 / 00d23fa / 27821c4):**
- Parser: `[axis]` allowlist extended 12→18 verbs (`⊂ ⊃ ∊ / \ ⌿`).
- `⊂[axis]` (Kotlin `AxisEnclosedValue`, disclose.kt:9-46) — column-major axis 0, row-major axis 1+.
- `∊[N]` (Kotlin `MemberFunction`, member.kt:91-115) — `enlist` + `enlist_recurse` helpers; monadic `∊` dispatch at evaluator.rs:3183.
- `/[axis]` + `⌿[axis]` (Kotlin `SelectElementsLastAxis/FirstAxisFunctionImpl`, lookup.kt:340-371) — `select_elements_axis` helper, replication loop over strides. Direct-verb only; adverb form `+/[axis]` already works via `adv_explicit_axis` at evaluator.rs:1900.

**Open in T1.1 (from d1 audit, no fresh data — re-audit recommended):**
1. **`⊃[axis]` (8 cases)** — needs `DisclosedArrayValue` + transpose perm; not a one-shot fix. `disclose_axis` helper was prototyped and removed (wrong shape: rank-1 flat vs oracle's rank+1).
2. **`+[axis] x y` B-is-rank-1 path** in `num2_axis` (Kotlin `computeTransformation` "Other side is rank-1" case) — likely 4-7 cases. The `+[axis]` monadic arm was prototyped and reverted (wrong semantics — Kotlin uses `ResizedArrayImpls.makeResizedArray` not enclose-axis).
3. **Other bracket-axis verbs** not yet attempted (audit said 89 cases total, ~57 open after step 3 — exact residual TBD).

**Verified oracle probes (current port):**
- `⊂[0] 2 3 2 ⍴ ⍳12` → `((0 6) (2 8) (4 10) (1 7) (3 9) (5 11))` ✓
- `⊂[1] 2 3 2 ⍴ ⍳12` → `((0 2 4) (6 8 10) (1 3 5) (7 9 11))` ✓
- `⊂[1] 1 2 3 4` → "Axis 1 is not valid. Expected: 1" ✓
- `∊ 1 2 3` → `(1 2 3)`, `∊ (1 2)(3 4)` → `(1 2 3 4)`, `∊ ((1 2)(3 4))((5 6))` → `(1 2 3 4 5 6)` ✓
- `∊[N]` for N=0..3 and N=3000: oracle-matching ✓
- `∊[¯1] ...` → "Negative enlist limit: -1" ✓
- `2 2 /[0] 2 3 ⍴ ⍳6` → shape `(4 3)`, values ✓
- `2 1 1 2 ⌿[2] 2 3 4 ⍴ ⍳24` → shape `(2 3 6)`, values ✓
- `9 9 /[2] 2 3 ⍴ ⍳6` → "Axis 2 is not valid. Expected: 2" ✓
- `R2: a←(⍕¨100+⍳10) ⋄ {a.(⍵)}¨ (⍳10)-10` → `("100" "101" … "109")` ✓ (oracle `⊢ ⟨"100" … "109"⟩` — same content, just `⟨⟩` vs `()` display)
- T1-T5 arrayDereference: oracle-matching content (port emits `error: …` vs oracle's `Error at: L:C: …` — cosmetic only)

## CARRYOVERS / DEFERRED
- **T1.3 labels cluster (40 open cases)** — parse layer unblocked per d1 summary; real fix is separate parser issues (extracts, iota-as-reshape-dim, axis-applied at statement start). Tracked separately.
- **T2.1 reshape spec keywords** (`:match`/`:fill`/`:truncate`/`:recycle`, 4 cases) — closed at `PROGRESS-20260903b.md`.
- **T2.2 reduce `⊥` panic (78 cases)** — architecturally different (body-panic deep-dive), deferred.
- **`s:col` qualified-name column-lookup** — operator+dfn+namespace-lexical-scope tangle, half-day investigation.

## DECISIONS & RATIONALE
- **Two-gate registration for builtins** (hard rule): new verb goes in BOTH `evaluator.rs::is_primitive_name` AND `is_primitive_op`. Miss one → silent dispatch failure.
- **AxisApplied match arm order**: specific verbs (`⊂`, `∊`, `/`, `⌿`) MUST precede the `_ =>` catchall. Re-ordering without testing breaks prior verb semantics.
- **Stale-binary-first** (hard rule): when a probe result contradicts a recent edit, `git stash && cargo build -p kap-cli && probe && git stash pop` BEFORE debugging. This is the #1 silent killer and the most common cause of impossible-looking diffs.
- **Kotlin ground-truth discipline**: every new arm carries a `Kotlin <file>:<line>` citation pointing to the source of truth. No `from-scratch` reasoning — the Kotlin `builtins/*.kt` files are the spec.
- **Oracle ground-truth in any problem write-up MUST contain pasted output** (never predicted). `OK/MISMATCH/UNSUPPORTED` totals must be from `cargo test`, not estimated.

## FILE PATHS
- `/home/theb/Apps/array/rust-kap/kap-core/src/evaluator.rs` — new arms in AxisApplied match (`⊂`, `∊`, `/`, `⌿`), helpers `enclose_axis`, `enlist`, `enlist_recurse`, `select_elements_axis`; `∊` monadic dispatch at line 3183; `eval_member_deref` symbol-as-index routing at ~1393; `array_member_deref` at ~1236; value/name form split.
- `/home/theb/Apps/array/rust-kap/kap-core/src/parser.rs` — AxisApplied parsing, verb allowlist (12→18 at commit `0c09c80`).
- `/home/theb/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/disclose.kt:9-46` — Kotlin ground truth for `⊂[axis]`.
- `/home/theb/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/member.kt:91-115` — Kotlin ground truth for `∊[N]`.
- `/home/theb/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/lookup.kt:340-371` — Kotlin ground truth for `/[axis]` and `⌿[axis]`.
- `/home/theb/Apps/array/rust-kap/conformance/kotlin_tests.jsonl` — 2545 cases; T1.1 axis-applied subset ~89 per d1 audit.
- `/home/theb/Apps/array/rust-kap/META-INF/ROADMAP.md` §A.3 (T1.1 spec) + Appendix A (verified-state synthesis 2026-08-31).
- `/home/theb/Apps/array/rust-kap/META-INF/PROBLEM.md` — current-state Labels Implementation status (post-`df65df9`).

## COMMANDS (copy-paste ready)
```bash
export ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
export PORT=~/Apps/array/rust-kap/target/debug/kap

# build
cd ~/Apps/array/rust-kap && cargo build -p kap-cli

# test gate
cd ~/Apps/array/rust-kap && cargo test -p kap-core

# conformance sweep (slow)
cd ~/Apps/array/rust-kap && cargo test -p kap-core --test conformance

# oracle probe (one-liner)
printf '%s\n' "EXPR" | $ORACLE 2>&1 | grep -aE '⊢ ' | head -1

# port probe (one-liner)
printf '%s\n' "EXPR" | $PORT 2>&1 | tr -d '\0' | head -3 | sed -n 's/.*>>> //p' | head -1

# stale-binary discriminator
cd ~/Apps/array/rust-kap && git stash && cargo build -p kap-cli && probe && git stash pop
```

## ACTIVE BLOCKERS / NEXT-MOVE OPTIONS
1. **`⊃[axis]` (8 cases)** — biggest single remaining T1.1 cluster; design-heavy (needs `DisclosedArrayValue` + transpose perm).
2. **`+[axis]` B-is-rank-1 path** in `num2_axis` (~4-7 cases) — most-impactful remaining bracket-axis.
3. **T1.1 fresh conformance sweep** — d1 estimate "~32/89 closed, ~36%" is 1-day stale; run real sweep to get current numbers before committing to a sub-cluster.
4. **T1.3 labels cluster (40 open)** — separate parser issues, parse layer unblocked per d1.
5. **T2.2 reduce `⊥` panic (78 cases)** — architecturally different deep-dive.
6. **`s:col` carryover** — operator+dfn+namespace-lexical-scope investigation.

## RECOMMENDED FIRST MOVE
Run a fresh T1.1 conformance sweep (`cargo test -p kap-core --test conformance 2>&1 | grep T1.1` or filter the JSONL) to get a real "open/closed" count. Then pick from options 1 or 2 based on actual residual — don't trust the d1 audit number blindly.

## SKILLS TO RELOAD BEFORE CONTINUING
- `rust-kap-dev` — full STALE-BINARY / `next_is_paren_operator` / parser-edit discriminator notes.
