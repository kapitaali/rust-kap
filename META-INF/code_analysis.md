# CODE ANALYSIS — rust-kap (2026-08-22)

**Role:** analysis only. Nothing in this document has been edited; every claim below
was verified empirically against the current tree (`feature/wheres-extra`, HEAD
`a2bffde` + 4 modified source files) and side-by-side against the Real Kap oracle
(`~/Apps/array/kap-jvm-text/bin/kap-jvm-text`).

**Sources analyzed:**
- `META-INF/handoff_prompt.md`, `PROGRESS-20260821.md` (+uncommitted tail),
  `PROGRESS-20260822.md`, `ROADMAP.md`, `KNOWN-NONCONFORMANCE.md`
- `last_session.json` (crashed 2026-08-21 session, HTTP 504 after retries;
  139 messages) and `last_session_continued.json` (session `20260822_150318_b3b14d`,
  "Rust-kap14", 226 messages, user-interrupted)
- Current diffs of `kap-core/src/{ast,parser,evaluator,number}.rs`
- Kotlin ground truth: `array/src/commonMain/.../tokeniser.kt`, `parser.kt`,
  `builtins/reshape.kt`, `engine.kt`

---

## 1. Bottom line

The uncommitted tree contains **one P0 parser regression** that breaks dyadic
subtraction everywhere (gates RED: lib 83 passed / **9 failed** vs 92/0 baseline,
curated parity **0/1** vs 1/0), plus **5 leftover debug prints**, **2 stale
conformance rows**, one **reshape validation divergence**, and a **missing
operator (`∵`) + missing `⌽∵`-style axis support** that will block io.kap
immediately after the P0 is fixed.

Everything else in the uncommitted batch (`⊤`/`⊥` base-value rewrite, rank
operator `f⍤k`, `and`/`or`, modulo arg-order, `∊` scalar shape, negative-dim
reshape *inference*) was probed against the oracle and is **semantically
correct** — keep it, fix around it.

---

## 2. Findings

### F1 — P0 REGRESSION: unary-minus arm in `parse_function_atom` swallows the right operand of dyadic `-`

**Location:** `kap-core/src/parser.rs:1919–1927` (the `parse_function_atom`
`Symbol` arm).

```rust
Token::Literal(LiteralValue::Symbol { name, namespace }) => {
    ...
    if name == "-" && namespace.is_none() {
        self.advance();
        let operand = self.parse_function_atom()?;   // <-- consumes `4` in `3 - 4`
        return Ok(Instr::Apply { fn_expr: Symbol("-"), left: None, right: operand });
    }
```

**Mechanism:** For any `L - R`, the `L f R` block (`parse_apply`, ~1144–1196)
parses `-` as a function atom via `parse_function_expr`. This arm fires inside
that call, consumes `R` itself, and returns monadic negate. The block then calls
`self.parse_apply()` at line 1190 with only the trailing `Newline` left →
`parse_primary` catch-all (parser.rs:2171) → `parse error … unexpected token in primary`.

**Live proof (this session):**

| expr | port (current tree) | oracle |
|------|--------------------|--------|
| `3 - 4` | parse error at 1:6 | `¯1` |
| `3-4`   | parse error at 1:4 | `¯1` |
| `x←3 ⋄ x-1` | parse error | `2` |
| `@a-@\0` (io.kap line 4 form) | parse error at 1:5 | `97` |
| `65 - @a` | parse error | `Error: Incompatible argument types…` (must still error, but as a runtime type error) |

This kills **all 9 failing lib tests** (`eval_sub_neg`, `eval_modulo`,
`eval_char_difference`, `eval_char_minus_int_vector`, all four `eval_train_*`)
because each contains a `-` expression, and breaks `use("io.kap")` at line 4
(`code ← ch-@\0`) → every `io:` symbol ends up "undefined".

**Kotlin ground truth:** there is **no unary-minus handling anywhere** in the
Kotlin parser/tokeniser. `-` is an ordinary symbol whose valence resolves at eval
time (monadic negate when no left operand accumulates). The lexer does fold
`-<digit>` into a negative literal, but `-x` is just `Symbol("-")` then
`Symbol("x")`.

**Recommended fix (Option B/C from the handoff, validated here):**
1. **Delete** the `parse_function_atom` arm at parser.rs:1919–1927 entirely.
2. **Keep** the `parse_apply` entry arm (parser.rs:805–816) — it only fires when
   `-` is the first token of a value expression, which matches the oracle
   (`- 5` → `-5`, `(- 5)` → `-5`, `x←3 ⋄ -x` → `-3`, all oracle-confirmed).
3. If `(-x)` inside parens still fails after removal, handle it at the
   OpenParen/function-expr boundary only (Option C) — never inside
   `parse_function_atom`, which is exactly the path the `L f R` block uses.
4. Re-gate: `cargo test -p kap-core --lib` must return to 92/0 and curated to 1/0
   before anything else is touched. (Note: two curated rows are stale — see F3 —
   so the honest target after fixing F1+F3 is lib 92/0, curated 1/0.)

### F2 — Leftover debug prints (5) polluting stdout and masking probe results

`kap-core/src/parser.rs`: lines **807**, **818**, **1268**, **1270**, **2173**
(all `eprintln!("DBG …")`). The handoff already flagged them as must-remove;
they are still present and fire on nearly every REPL line (they made several
probes this session look like failures until filtered). Remove all five; grep
gate: `grep -c 'eprintln!("DBG' kap-core/src/*.rs` must be 0.

### F3 — Two stale curated conformance rows (harness expectations predate correct code)

`kap-core/tests/conformance.rs` curated rows:

| row | expected | got | oracle |
|-----|----------|-----|--------|
| `"3 \| 2"` | `"1"` | `"2"` | `⊢ 2` — expectation is wrong (`3\|2` = residue of 2 mod 3 = **2**) |
| `"5 ∊ 1 2 3 4"` | `"(0)"` | `"0"` | `⊢ 0` — the new scalar-shape membership is CORRECT; the expected `(0)` predates it |

These are not code bugs — update the rows to `"2"` and `"0"`. They were written
before the `modulo()` arg-swap fix (number.rs) and the `∊` scalar-left fix
(evaluator.rs ~5782), both of which are oracle-verified correct.

### F4 — Negative reshape accepts ANY negative dimension; Kotlin allows only literal `-1`

`evaluator.rs` reshape hunk (~5820): treats every negative dim as "infer".
Kotlin (`reshape.kt:267,275–290,375–380`): **only `-1`** triggers MATCH
inference (divisibility-checked); any other negative errors
`Attempt to reshape to dimension with negative size: <n>`.

| expr | port | oracle |
|------|------|--------|
| `2 ¯1 ⍴ ⍳20` | `(2 10)` ✓ | 2×10 matrix ✓ (feature is real, keep it) |
| `¯3 ⍴ ⍳10` | `(0 1 2 … 9)` ✗ | **Error** ✗ |

Fix: accept `d == -1` only (error if more than one), reject other negatives
with Kap's message text. Also note Kotlin supports keyword specs
(`:match :fill :truncate :recycle`) the port doesn't know about — fine to defer.

### F5 — Missing operator `∵` (BitwiseOp) blocks io.kap even after F1 is fixed

io.kap lines 6–9 use `∨∵ ∧∵ ¯6 ⌽∵ code` (bitwise or/and + rotate-with-axis).
Kotlin registers it at `engine.kt:495 registerNativeOperator("∵", BitwiseOp())`;
oracle: `192 ∨∵ 31 → 223`, `¯6 ⌽∵ 5 → 0`. Port: `undefined symbol: ∵`.
This is the next blocker for `encodeUtf8Char` after F1 lands (the file parses
past line 4 but then dies on `∨∵`). Needs: lexer recognition, BOTH registration
lists (`is_primitive_op` / `is_primitive_name`), and a BitwiseOp evaluator arm —
per the standard two-gates rule in the skill.

### F6 — `⊥⍤1` rank-operator display shape divergence (minor, DISPLAY-class)

`256 (⊥⍤1) 104 105 0` → port `6842624`, oracle `6842624` ✓ (value correct).
But vector results render `(15 15)` where the oracle shows `⟨15 15⟩` /
box-drawing frames — the known house-style glyph divergence documented in
KNOWN-NONCONFORMANCE.md. Not a defect; do NOT "fix".

### F7 — `parse_two_disclose_groups_strand` failure is collateral, not independent

Fails as `foo ⇐ (×-)` → "expected a function in train". The `×-` train member
parse runs through the same poisoned `parse_function_atom` path as F1; re-test
after F1 before treating it as its own bug.

---

## 3. Uncommitted work verified CORRECT (keep; do not revert)

Probed side-by-side this session, port == oracle:

| expr | result (both engines agree on value) |
|------|--------------------------------------|
| `256 ⊥ 104 105 0` | `6842624` |
| `2 ⊥ 1 2 3` | `11` |
| `64 ⊤ 66051` | `(16 8 3)` |
| `16 ⊤ 255` | `(15 15)` |
| `256 (⊥⍤1) 104 105 0` | `6842624` (rank op) |
| `⍴ ¯1 3 ⍴ ⍳12` | `(4 3)` (−1 inference) |
| `1 or 2` / `0 or 2` / `2 and 3` / `0 and 3` | `1` / `2` / `3` / `0` (short-circuit, raw-operand semantics) |
| `3\|5` / `5 \| ¯3` / `3 \| 2` | `2` / `2` / `2` (modulo swap correct) |
| `5 ∊ 1 2 3` | `0` (scalar shape correct) |
| `"ABC" unicode:toLower` | `"abc"` |

Also correct per Kotlin source review: the `AxisApplied` parser/evaluator wiring
(`+[0]` etc.), `ValueOp` AST node, `apply_rank_op` spec rules (scalar→both
sides, 2-elem→[L,R], 3-elem→middle, negative clamped).

---

## 4. Agent process findings (from the two session transcripts)

These are the meta-mistakes worth institutionalizing; the skill already encodes
most of them but the sessions kept violating them:

1. **Stale-binary thrash dominated both sessions.** In `last_session_continued.json`,
   the same symptom (`3 - 4` parse error) was alternately declared "real bug",
   "stale-binary phantom", "real again" across messages [23], [64], [108],
   [116], [131], [154]–[162]. Each flip cost a rebuild cycle. The skill's rule
   (stash → clean rebuild → probe, ONE discriminator pass, then trust the result)
   was applied only at message [191] and immediately produced the truth.
   **Suggestion:** make the stash+clean+probe triple a mandatory single step
   before ANY diagnosis of a "flaky" parse symptom, and record its verdict once.

2. **Debug prints were added and removed repeatedly instead of using a scratch
   harness.** At least 9 distinct `DBG` eprintln sites were created and deleted
   across the session; 5 survive in the tree today (F2). A temporary probe test
   (the array-language-impl skill's catch_unwind table pattern) would have shown
   the whole parse path without touching the source.

3. **The handoff under-specified the fix but correctly identified the culprit.**
   `handoff_prompt.md` root-caused the `parse_function_atom` arm precisely
   (including why `+` works and `-` doesn't) and listed three candidate fixes,
   yet the next session spent ~100 messages re-deriving the same root cause
   before agreeing with the handoff. **Suggestion:** future handoffs should name
   the recommended option explicitly ("apply Option B") rather than presenting
   open candidates.

4. **Session death by oversized streaming calls.** Both transcripts end in
   transport failure: the first on a 504 after `max_retries_exhausted` during a
   large tool call; the second on repeated stream timeouts from large
   read/terminal payloads (messages [36], [38]) and finally a user stop.
   The project convention (<8K-token calls, split patches) exists — it was
   applied too late.

5. **Gates were not re-run before the crash.** PROGRESS-20260821 says "MUST
   re-run before commit"; PROGRESS-20260822 says "Gates NOT yet re-run". This
   analysis confirms they would have failed (9 lib + curated). The discipline
   gap let a broken tree accumulate ~900 diff lines across 4 files.

6. **One good outcome to keep:** the second session's final message [214]–[220]
   correctly located the `L f R` swallow mechanism before being stopped. Its
   reasoning matches this analysis; only execution remained.

---

## 5. Recommended action order for the editor

1. **F1** delete parser.rs:1919–1927 (keep the :805 entry arm) → rebuild
   `cargo build -p kap-cli` (NOT just kap-core) → probe `3 - 4`, `3-4`,
   `ch-@\0`, `(-5)`, `x←3 ⋄ -x` vs oracle.
2. **F2** remove the 5 DBG prints; gate `grep -c 'eprintln!("DBG'` == 0.
3. **F3** update the two stale curated rows (`"3 | 2"` → `"2"`,
   `"5 ∊ 1 2 3 4"` → `"0"`).
4. Run full gates: `cargo test -p kap-core --lib` (target 92/0),
   `cargo test -p kap-core --test conformance curated_kap_parity` (target 1/0).
5. **F7** re-test `foo ⇐ (×-)`; only escalate if still failing.
6. Commit the batch (it is otherwise sound), update PROGRESS-20260822 with the
   gate numbers, then proceed to **F4** (tighten reshape to literal `-1` +
   Kap error text) and **F5** (`∵` BitwiseOp, both registration lists) to
   finish the io.kap chain: `encodeUtf8Char` needs `∨∵ ∧∵ ⌽∵`; `toHex`/
   `fromHex` should then match the oracle end-to-end (`io:toHex 255` → `"FF"`).

---

## Appendix — Oracle probe table used in this analysis

| expr | oracle | port (current tree) |
|------|--------|---------------------|
| `3 - 4` / `3-4` | `¯1` / `¯1` | parse error / parse error |
| `x←3 ⋄ x-1` | `2` | parse error |
| `- 5` / `(- 5)` | `-5` / `-5` | (entry arm OK) |
| `@a-@\0` | `97` | parse error |
| `1-@a` | Error: incompatible types | parse error |
| `192 ∨∵ 31` | `223` | undefined symbol: ∵ |
| `¯6 ⌽∵ 5` | `0` | undefined symbol: ∵ |
| `¯3 ⍴ ⍳10` | Error: negative size | `(0 1 … 9)` wrong |
| `2 ¯1 ⍴ ⍳20` | 2×10 matrix | `(2 10)` ✓ |
| `256 ⊥ 104 105 0` | `6842624` | `6842624` ✓ |
| `64 ⊤ 66051` | `⟨16 8 3⟩` | `(16 8 3)` ✓ (glyph) |
| `3 \| 2` | `2` | `2` ✓ (curated row stale) |
| `5 ∊ 1 2 3 4` | `0` | `0` ✓ (curated row stale) |
| `unicode:toLower "ABC"` | `"abc"` | `"abc"` ✓ |
