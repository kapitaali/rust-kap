# PLAN — final ~10% of conformance (implementor playbook)

Goal: 89.3% → ~98%+ on the **in-scope** corpus without regressions.
Status at write time: `2367 ok / 13 mismatch / 270 unsupported / 2650 total`
(`conformance_summary.txt`), lib tests **91 passed / 5 failed** (HEAD `b0a000d`).
Corpus: 2592 `commonTest` (in scope) + 58 platform-specific (`jvmTest`/`jsTest`/etc.,
out of scope per strategy §1.2). Ground truth for every claim: Kotlin source +
`kap-jvm-text` oracle (ROADMAP §0). Method per task: systematic-debugging +
TDD skills (root cause first, one variable at a time, Rule of Three).

## 0. Non-negotiable protocol (every task, no exceptions)

1. **Lib green before conformance work.** The 5 red lib tests are P0 (§1). No
   conformance commit lands while `cargo test -p kap-core --lib` is red.
2. **Baseline dump + diff per task.** Before touching code:
   `rm -f /tmp/conform_dump.txt && CONFORM_DUMP=1 cargo test --jobs 1 --test
   conformance -- --nocapture` → `cp /tmp/conform_dump.txt /tmp/base.txt`.
   After the fix, re-run → `/tmp/mine.txt`, then diff `OK→!OK` (regressions) vs
   `!OK→OK` (wins). Note: `references/regression_diff.py` (cited in skill) does
   **not** exist in this tree — use `comm`/`grep` on the two dumps directly.
3. **One cluster per commit.** Never bundle two defect classes; every commit
   message carries its oracle transcript + dump delta (`+N OK / -M MISMATCH`).
4. **Verify-first on every `expected`.** `expected=None` rows prove nothing when
   they flip to OK (extraction gap, not oracle truth) — confirm against the
   oracle binary before claiming the win. Same for suspicious `expected` values
   (§2, reverse tests).
5. **Two-gate rule** (`is_primitive_op` + `is_primitive_name`), **stale-binary
   rule** (rebuild `kap-cli`, `git stash` discriminator when a symptom looks
   impossible), **no leniency creep** (where oracle errors, port errors identically).

## 1. P0 — fix the 5 red lib tests (single signature, do first)

All five fail identically — scalar cells render boxed:

| test | got | want |
|---|---|---|
| `eval_iota` | `((0) (1) (2) (3) (4))` | `(0 1 2 3 4)` |
| `eval_reverse` | `((4) (3) (2) (1) (0))` | `(4 3 2 1 0)` |
| `eval_take_drop` | `((0) (1) (2))` | `(0 1 2)` |
| `eval_parenthesised_groups` | `()` | `(10 11 12)` |
| `eval_format_directives` | `("(0) (1)" "(2) (3)")` | `("0 1" "2 3")` |

Suspect (unverified — confirm, don't assume): HEAD `b0a000d` "scalar-cell shape
(rank<=1 → 0-dim cell)". Discriminator: `git stash` → run the 5 tests → green
proves the commit; unstash → red confirms scope. Then trace one case
(`⍳5` → `((0)…)` means each element built as a 1-cell array, likely in
`make_simple_or_nested` / the iota cell path) and fix at the source. The first
four share the shape; `eval_parenthesised_groups` (`()` vs `(10 11 12)`) may be
a second defect hiding behind the same commit — isolate it separately if it
survives the shape fix. Gate: 96/0 before §2.

## 2. Mismatches first (13 — every one is a proven-wrong answer)

Mismatches outrank unsupported: the engine claims an answer and it's wrong.
Order by isolation (single-builtin → multi-builtin interaction):

1. **`roundSpecialisedArrayLong`** — `math:round` on `9223372036854775806` returns
   `…807`, on `¯9223372036854775807` returns `…808`. Off-by-one at the i64
   boundary; look at the round path's float conversion (`as_double` loses the
   low bit — `number.rs`). Single function, oracle-verify the two edge values.
2. **`encodeMultipleValues1`** — `2 3 4 ⊥ <300-el array>` wrong. Multi-radix
   decode; compare against `decode.kt` cell order. One builtin.
3. **`multiDimensionalIntersectionWithRightEnclosed1`** — `(⍳3 3) ∩ ⊂(1 0) (1 1)`
   returns `()`; `expected=None` so oracle-verify what `∩` should give first.
4. **`oneArgumentIdenticalWithPlusReduceEach` (×2, incl. `comp` variant)** —
   `≡ +/¨ ⍳4 5 6` gives `2`, want `1`. `≡`-vs-reduce/each shape interaction;
   fix the pair together (same root cause or two adjacent ones — diff will tell).
5. **`testUnderFromList`** — `(1+)⍢fromList (10;20;30)` gives `(11 21 31)`,
   want `11`. Under-with-`fromList` applies per-element instead of once; look at
   `apply_under_op` list handling.
6. **`failWith2DArrayArgument`** — `⍳ 2 2⍴1 2 3 4` evaluates instead of failing.
   Missing rank guard on monadic `⍳` (cf. how `⍉` guards landed). No-leniency
   case: add the error, don't widen acceptance.
7. **`operatorWithLambdaFunctionRightArg{With,Without}Paren`** — `(-bar ⍞f0) 10`
   returns `<function>` instead of `220`/`290`. `⍞`-bound lambda as operator
   operand never applies. Parser (`OpCall` formation) + `apply_user_op` jointly;
   use `astprobe` first to see which side drops the application.
8. **`axisAssignedFunctionWithDifferentEnv3` / `assignmentToConstInFunction`** —
   both `expected=None`. Oracle-verify before touching: the first involves
   `+[y]` axis capture across envs, the second `declare(:const)` inside a fn.
   If the oracle errors, these become no-leniency guard tasks; if it returns the
   port's value, they're extraction gaps — close as verified, not as fixes.
9. **`reverseHorizontalTest` / `reverseVerticalTest`** — `expected=Some("1")`
   but the Kotlin test almost certainly asserts something downstream (a real
   `⌽` test would expect the reversed array, not `1`). Highest suspicion of
   bad `expected` synthesis in this list: oracle-verify the exact expr, and if
   the oracle returns what the port returns, fix the extractor row, not the engine.

## 3. Unsupported clusters (in-scope only, biggest first)

Out of scope, do not touch: `contrib/sql` (22, strategy §1.2 `contrib/*` **and**
`jvmTest`), `JavaTypesTest` (13, `jvmTest`), `XmlParserTest` (8, `jvmTest`).
That removes 43 of the 270 without work. Triage-before-code: `JsonTest` (9),
`IOAPLTest` (9, incl. 5× `io:readCsv` on `test-data/*.txt` — those files do not
exist in this repo, so confirm absence → harness-environment gap, not engine
gap), `KapDatesTest` (7, platform time module — probe oracle, likely thin
wrappers). Then, in order:

1. **ConcatenateTest (6)** — `,[1]`, `,[0]`, `,[0.5]`×2, `,[2]`+`,[1.5]`
   (`concatenateWithAdditionalDimension`). One code path: `catenate_axis` +
   laminate. Highest density: 6 cases, one function. Watch the label-threading
   commits (`7b07cb6`) — axis forms must preserve labels per
   `ConcatenateAPLFunctionFirstAxisImpl.resolveLabels()`.
2. **StructuralUnderTest (11)** — after §2.5 lands, take the remaining 11 as one
   `apply_under_op` pass. Read `operator.kt` structural-under top-to-bottom
   first; several will share the overlay/inverse dispatch already built.
3. **SyntaxTest (12)** — parser-only cluster; each case is small. Batch by
   sub-area after `astprobe` triage, one commit per sub-area, never one mega-
   commit (parser edits have the widest blast radius — cf. `b0a000d` fallout).
4. **PickTest (9)** — `lookup.kt` pick/index-of/access. Adjacent to the closed
   bracket-index work; same file region, low regression risk.
5. **ComposeTest (6), ReduceTest (8)** — trains/reduce edge forms. ReduceTest's
   8 likely share the nested-reduce path flagged in `b0a000d` ("nested-
   cube/reduce interaction fix" still open) — treat that commit message as the
   starting hypothesis and verify with the dump diff.
6. **AssignmentTest (7), ScopeTest (7), NamespaceTest (7)** — scoping cluster;
   ARCHITECTURE.md §7 lookup order is the map. `declare(:local)`/`home_ns`
   semantics per case; oracle-verify each (several `expected=None`).

## 4. Stop conditions & honest accounting

- Stop a task at 3 failed fix attempts (Rule of Three) → re-analyze, don't pile
  on; parser-blast-radius tasks stop at 2.
- When the headline ok-count moves, report known-vs-unknown: rows with
  non-empty `expected` are genuine; `expected=None` flips are coverage, not
  proven wins, until oracle-verified.
- Done = lib 96/0 + `curated_kap_parity` green + dump diff with zero `OK→!OK`
  rows + PROGRESS entry (today's file, file:line + oracle transcript + gate
  numbers) before the next task. Target: mismatches → 0 (minus any confirmed
  extractor bugs), in-scope unsupported → long tail only.
