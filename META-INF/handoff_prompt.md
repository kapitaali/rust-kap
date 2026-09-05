# HANDOFF PROMPT — rust-kap (2026-09-05, post T1.2-3b + T1.3 onlyBrackets)

**Project:** `~/Apps/array/rust-kap` (branch `feature/wheres-extra` = `main` = `strings` = `6ef4ea6`, pushed to origin). Rust rewrite of the Kap array language. Goal: extend conformance against the Kotlin oracle `~/Apps/array/kap-jvm-text/bin/kap-jvm-text`.

**TRUTH SOURCE (do not violate):**
- Oracle binary: `~/Apps/array/kap-jvm-text/bin/kap-jvm-text` (a.k.a. `$ORACLE`).
- Port binary: `~/Apps/array/rust-kap/target/debug/kap` (a.k.a. `$PORT`).
- Kotlin source (READ only): `~/Apps/array/array`.
- **Stale-binary rule (critical):** after ANY parser/evaluator edit, `cargo build -p kap-cli` (NOT `-p kap-core` alone). If symptoms look impossible, `git stash && cargo build -p kap-cli && probe && git stash pop` (skill `rust-kap-dev` calls this the stale-binary discriminator).
- Gates: `cargo test -p kap-core` → **161 passed / 0 failed across 14 binaries**. `cargo test -p kap-core --test conformance` for the conformance sweep (slow, ~22s).
- User directive: **document to `META-INF/PROGRESS-<date>.md` before continuing** (file:line + oracle-vs-port result per finding). `META-INF/ROADMAP.md` §0.1 laws 4 ("no leniency creep") and 8 (PROGRESS file per day) are binding.
- Two-gate rule (parser + evaluator must agree): `parser.rs::is_primitive_op` and `evaluator.rs::is_primitive_name`.

---

## CURRENT STATE (pushed, clean tree)

- **HEAD `6ef4ea6` on `feature/wheres-extra` / `main` / `strings` / `origin/*` (all 7 refs equal).**
- 3 new commits this session (`c1a85f6` → `6ef4ea6`):
  - `c1a85f6` — `docs(meta): NON-IMPLEMENTED ledger #2/#4 marked CLOSED (Parts 7-8)`. Stale-audit fix: `META-INF/NON-IMPLEMENTED.md` (created at `507e98e`) was never updated; entries #2 (+[axis] monadic) and #4 (∧/∨ sort-along-axis) are now marked CLOSED.
  - `b209edc` — `fix(parser): top-level [...] is index deref, not list literal (T1.3, oracle-verified)`. One-line guard in `parse_value_kotlin` (parser.rs:316) that errors with `"Index dereference without argument"` if the first token is `OpenBracket`. Closes the `onlyBrackets` case.
  - `6ef4ea6` — `docs(meta): PROGRESS-20260904 Part 10 — T1.3 onlyBrackets closed at b209edc`.
- **Tests: 161/0 across 14 binaries** (was 13/155/0; added `kap-core/tests/index_deref_only.rs` with 6 new tests).
- **Conformance: 1931 ok / 38 mismatch / 576 unsupported / 75.9% coverage** (was 1929/40/576; +2 ok, -2 mismatch).
- The `onlyBrackets` case (and the implicit `[]`, `a ← []`, `a ← [1; 2; 3]` cases that share the root cause) moved from mismatch to ok.

## T1.1 f[axis] cluster — FULLY CLOSED except ⊃[axis] (deferred)

| Verb | Status | Commit / Where |
|---|---|---|
| `⊂[axis]` | ✓ closed | `00d23fa` (Part 7) |
| `∊[N]` | ✓ closed | `00d23fa` (Part 7) |
| `∧[axis]` / `∨[axis]` (sort-along-axis) | ✓ closed | `7684633` (Part 7), 13 tests in `kap-core/tests/sort_axis.rs` |
| `+[axis]` monadic | ✓ closed | `ccc38ac` (Part 8), 4 tests in `kap-core/tests/monadic_axis_arith.rs` |
| `,[axis]` laminate (`,[0.5]`) | ✓ closed | `Part 6` |
| `⌽[axis]` / `⊖[axis]` (rotate) | ✓ closed | Part 7 |
| `↑[axis]` / `↓[axis]` (take/drop) | ✓ closed | Part 7 |
| `labels[axis]` / `hasLabels[axis]` | ✓ closed | 21/21 in T1.1 labels-threading (`72c4879`) |
| `⊃[axis]` (disclose) | ❌ 11 cases DEFERRED | see `META-INF/NON-IMPLEMENTED.md` #1 — needs `DisclosedArrayValue` + `TransposedAPLValue`, 200-400 lines, half-day |

**T1.1 carryover: 11 cases (⊃[axis] only).**

## T1.2 MemberDeref cluster — FULLY CLOSED

- T1.2-1 (6 missing-error cases): done (Part 8)
- T1.2-2 (5 symbol-as-index): done (Part 9 confirmed)
- T1.2-3a (`memberDereferenceFromExpression`): done
- T1.2-3b (`dereferenceMapWithConstantString`): **closed at `87d0744` (Part 9)**
- T1.2-3c (`indexDereferenceInvalidDimensions*`): done

Fix for T1.2-3b at `evaluator.rs:1147-1175`: name-form path of `Instr::MemberDeref` coerces bare-name (`namespace=None`) to `Str(name)` and qualified-name (`namespace=Some(_)`) to `Symbol{name, namespace}`. Parens form (`m.('foo)`) unchanged. Regression test: `member_deref_bare_name_matches_string_key` in `kap-core/tests/member_deref_parity.rs` (now 7 tests).

## T1.3 labels cluster — 1 of 40 cases closed this session

The `onlyBrackets` case (which was in the wider T1.3 carryover) is now closed. The remaining 39 T1.3 cases (label threading through `↑`/`↓` with label keys, axis-applied ordering at statement start, etc.) are still deferred per ROADMAP §0.1 law 4 — they require parser changes for label-keyed take/drop plus new value-type fields for key-axis labels.

## T2.1 / T2.2 / T2.3 — still deferred

- **T2.1 reshape spec keywords (4 cases)** — `:match` / `:fill` etc. Separate session, separate cluster. (T2.1 work tracked elsewhere per `PROGRESS-20260904b.md`.)
- **T2.2 reduce `⊥` body OOB panic (78 cases)** — 56 + 22 cases. Architecturally different, deep-dive. The enclose pass-through fix (T2.2 close-out) is in; the body-panic work is NOT.
- **T2.3 complex numbers (48 cases)** — design-locked, deferred.

## ACTIVE TASK — next session recommendations

The 38 remaining mismatch cases are the candidates. Per the survey in Part 10, the active mismatch list (sample 30, full 38) is dominated by:

- **Bigint / Rational / Complex mismatches** (`rationalMixedWithDouble0/1`, `checkBigInt0`, `rangeWithComplexLeft/RightArg`, `math:isPrime`) — algorithmic, multi-day. Defer.
- **Null propagation** (`subCharAndNull`, `minMaxWithNilShouldFail`) — small evaluator fixes; check `null` argument cases in `⍸`/`⌊`/`-`. **Tractable: 1-2 cases per session.**
- **Member-of specialised array** (`memberOfWithSpecialisedArrayLong/Double`) — `(⊂1 2) ∊ 10 11 12` expected `"0"`, got `(0)`. The P8 display difference (`"0"` vs `(0)`) — see if ROADMAP §11 Option A applies. **Tractable if display-only.**
- **Assignment/destructuring** (`assignmentToList`, `destructuringAssignmentWrongDimensions`) — `foo bar←10` should error, port accepts; `(a b c d e f) ← 3 2 ⍴ ...` should error on dim mismatch. Parser-level.
- **`testWithScalarEnclosed`** — `≠ ⊂"abc"` expected `(1)`, got `(1 1 1)`. `≠` on an enclosed scalar should be a 1-element vector. Small evaluator fix.
- **`testScope`** — `declare(:local a)` inside a function — scope-tracking bug. Larger refactor.
- **Left-bind chain** (`leftBindChain0/1/3`) — `1+10+` partial-application. Parser-level.
- **Operator with lambda** — `∇ foo x { ... }` lambda-rebinding. Parser-level.

**Tractable picks for next session** (in order of smallness):
1. `memberOfWithSpecialisedArrayLong/Double` — likely P8 display-only (2 cases; verify in `~/Apps/array/array/src/commonTest/kotlin/com/dhsdevelopments/kap/MemberOfTest.kt` first).
2. `testWithScalarEnclosed` — `≠` on enclosed scalar returns 1-element vector. 1 evaluator line + 1 test.
3. `assignmentToList` + `destructuringAssignmentWrongDimensions` — both about left-hand-side structural checks in assignment. 1 parser change + 2 tests.
4. `null⌊null` (`minMaxWithNilShouldFail`) — add null-arg check to `⌊`/`⌈`. 1 evaluator line + 1 test.

## FILES / PATHS / COMMANDS (concrete)

- **Repo:** `~/Apps/array/rust-kap`
- **Branch invariant command:**
  ```bash
  cd ~/Apps/array/rust-kap
  git checkout main && git merge --ff-only feature/wheres-extra
  git checkout strings && git merge --ff-only main
  git checkout feature/wheres-extra
  git push origin main strings feature/wheres-extra
  git rev-parse HEAD main strings feature/wheres-extra origin/main origin/strings origin/feature/wheres-extra
  # all 7 should print the same SHA
  ```
- **Stale-binary fix:**
  ```bash
  touch kap-core/src/parser.rs kap-core/src/evaluator.rs && cargo build -p kap-cli
  ```
- **Conformance sweep:**
  ```bash
  cd ~/Apps/array/rust-kap
  cargo test -p kap-core --test conformance --release 2>&1 | tee /tmp/conformance.out
  ```
  (The conformance binary writes `conformance_summary.txt`.)
- **Focused T1.1 probe:** `/tmp/t1_1_focused.py` (committed copy at `tools/t1_1_focused.py`).
- **Test file inventory** (under `kap-core/tests/`):
  - `conformance.rs` — main conformance sweep
  - `encode_decode_parity.rs`, `inspect_decode.rs`, `show_decode.rs` — display
  - `labels_parity.rs` — T1.1 labels threading
  - `member_deref_parity.rs` — T1.2 (now 7 tests)
  - `monadic_axis_arith.rs` — T1.1 +[axis] monadic (4 tests)
  - `null_propagation.rs`, `print_null.rs` — null handling
  - `reshape_spec_keywords.rs` — T2.1 carryover
  - `sort_axis.rs` — T1.1 ∧/∨ sort (13 tests)
  - `index_deref_only.rs` — T1.3 onlyBrackets (6 tests, NEW this session)

- **PROGRESS files:** `META-INF/PROGRESS-20260904.md` (Parts 1-10), `META-INF/PROGRESS-20260904b.md` (T2.1), `META-INF/PROGRESS-20260904c.md` (T2.2 root-cause), `META-INF/PROGRESS-20260904d.md` (T1.1 audit).
- **Audit doc:** `META-INF/NON-IMPLEMENTED.md` — updated this session (entries #2 and #4 marked CLOSED; entry #1 ⊃[axis] is the only remaining T1.1 carryover).

## PORT ANCHOR FILES

- `kap-core/src/evaluator.rs:316-340` — `parse_value_kotlin` (the one-line `[` guard added this session).
- `kap-core/src/evaluator.rs:1147-1175` — `Instr::MemberDeref` name-form path (T1.2-3b fix).
- `kap-core/src/evaluator.rs:1531-1840` — `AxisApplied` dispatch (covers ∧/∨, ⊂, ∊, /, ⌿, +, -, ×, ÷, *, ↑, ↓, ⌽, ⊖, ,, ⍪, labels, hasLabels).
- `kap-core/src/evaluator.rs:13890-13970` — `grade` / `grade_up` / `grade_down` (used by `⍋`/`⍒`, NOT by `∧`/`∨`).
- `kap-core/src/evaluator.rs:13977-14058` — `sort_array` (the helper used by `∧[axis]`/`∨[axis]`).
- `kap-core/src/parser.rs:1088` — axis-applied allowlist (20 verbs: `+ - × ÷ * , ⍪ ⌽ ⊖ ↑ ↓ labels hasLabels ⊂ ⊃ ∊ / \ ⌿ ∧ ∨`).
- `kap-core/src/parser.rs:5359-5379` — `parse_primary` `OpenBracket` arm (parses `[…]` as list literal, BUT only reached AFTER a value — top-level `[` is now caught at `parse_value_kotlin:316`).
- `kap-core/src/parser.rs:5451-5541` — `parse_index_suffix` (handles `value[axis]` correctly).

## KOTLIN ANCHOR FILES (READ ONLY)

- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/parser.kt:888-903` — `processIndex` / `processLeftArgAdjustment` (the oracle semantics for top-level `[`).
- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/lookup.kt:78-90` — `MemberDereferenceNameArgumentInstruction` (T1.2-3b).
- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/builtins/sort.kt:192-219` — `sortKapArray` (T1.1 sort-along-axis).
- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/builtins/binary-functions.kt:62-162` — `AndAPLFunction` / `OrAPLFunction` (monadic = sort, dyadic = bitwise).
- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/builtins/math_functions.kt:428` — `MathCombineAPLFunction.eval1Arg` (silently drops axis for monadic `+`/`-`/`×`/`÷`/`*`).
- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/builtins/disclose.kt:411-510` — `DiscloseAPLFunction` / `processAxis` (T1.1 ⊃[axis] carryover — needs porting).
- `array/src/commonMain/kotlin/com/dhsdevelopments/kap/values/` — `DisclosedArrayValue`, `TransposedAPLValue` (need to be ported for ⊃[axis]).

## CONSTRAINTS / HARD RULES (re-stated)

1. **Ground truth:** Kotlin source (READ only) + oracle binary. NEVER claim "Kap does X" without oracle transcript.
2. **Stale binary:** After ANY parser/evaluator edit: `cargo build -p kap-cli` (NOT just `-p kap-core` alone). If still stale: `touch kap-core/src/*.rs && cargo build -p kap-cli`.
3. **Branch invariant:** keep `main == strings == origin/*` after commits: `git branch -f strings main && git push origin main strings`.
4. **Process:** PROGRESS file `META-INF/PROGRESS-YYYYMMDD.md` per ROADMAP §0.1 law 8. Update `META-INF/NON-IMPLEMENTED.md` if a deferred item is closed.
5. **No leniency creep** (ROADMAP §0.1 law 4): when a port task surfaces a feature whose proper implementation would touch code outside its scope, STOP and document why in `NON-IMPLEMENTED.md` — do not expand the diff.
6. **Two-gate rule:** parser's `is_primitive_op` AND evaluator's `is_primitive_name` must both admit a verb before it can be used (e.g. `∧`/`∨` were added to both at `7684633`).
7. **Display-only diffs are not bugs** (ROADMAP §11 Option A): port uses `()`/`[]` where oracle uses `⟨⟩`/`┌→──┐` for multi-dim arrays. Don't try to "fix" this.

## PICK FOR NEXT SESSION (suggested)

**`testWithScalarEnclosed`** is the smallest, most self-contained mismatch. `≠ ⊂"abc"` should return a 1-element vector `(1)`, port returns `(1 1 1)`. Likely a 1-2 line fix in the `≠` (unique-mask) evaluator arm where the `Enclosed` value isn't being treated as a 0-or-1 (one-or-zero scalar) before the unique-mask op. Verify against oracle first, then fix and add a regression test.

If you want a different pick from the list above, follow the same pattern: oracle-verify the case, find the fix site via grep, write the regression test FIRST (RED), apply the fix (GREEN), commit, fast-forward branches, push.
