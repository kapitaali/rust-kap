# IMPLEMENTATION ANALYSIS 01 — Implementor Thinking Review

**Session:** Review of implementor's THINKING.md excerpt (2026-08-28).
**Context:** Implementing `code_analysis_05.md` fixes A1 (parser) + A2 (eval) for axis + fork tine.

---

## Executive Assessment

The implementor has correctly identified the **core contradiction**:
- **Clean HEAD** parses P12 (`output3.kap:118`) correctly → fails at eval with `unsupported axis operator: ↑`
- **Their eval-fix build** (parser == HEAD, only evaluator.rs changed) gives `parse error at 1:90`
- **Proof eval fix is live:** `2 +[0] 3 = 5` works

**This is impossible unless the binary is stale.** An evaluator-only change cannot introduce a parse error.

---

## Root Cause of the Loop

### 1. Cargo Incremental Compilation Deception
- `rm -f target/debug/deps/kap_cli-*` + `cargo build` **does not guarantee** kap-core recompilation
- Cargo's fingerprinting can reuse kap-core rlib from a different build directory or cached fingerprint
- The "5.40s build" vs "10.67s build" timing difference proves incremental caching
- **Fix:** `cargo clean` (full) or `cargo build -p kap-core --lib` explicitly before kap-cli

### 2. Parser Tracing is Correct but Irrelevant
The implementor's detailed trace of `parse_paren_vfn_chain` → `parse_paren_value_leading` → `try_parse_train` → Malformed is **accurate for the stale binary's parser**. But at true HEAD, this path succeeds (P12 parses). The Malformed they see is from an **older parser.rs** that cargo linked.

**Evidence:** Their own stash test (`tmp-eval` stash = HEAD + eval fix only) gave `unsupported axis operator: ↑` — confirming HEAD parser works.

### 3. Two Independent Bugs Were Conflated
`code_analysis_05.md` explicitly separated:
- **A1 (parser):** fork tines don't call `bind_operators_kotlin` → P10 fails (`⌽«,»(2↑[0])`)
- **A2 (eval):** `apply_train` doesn't unwrap `AxisApplied` from tines → P11 fails (`(2↑[0])«,»((-2)↑[0])`)

The implementor tried to fix both simultaneously, then got lost in parser tracing when the **eval fix alone should have been tested first on a verified-clean build**.

---

## Verified Truth (from code_analysis_05.md §3 probe matrix)

| Probe | Input | Oracle | Clean HEAD Port | Status |
|-------|-------|--------|-----------------|--------|
| P1–P9 | Various axis/fork combos | All match | All match | ✓ Already work |
| P10 | `⌽«,»(2↑[0]) ⍳6` | `⟨5 4 3 2 1 0 0 1⟩` | **Parse error** | **A1 needed** |
| P11 | `(2↑[0])«,»((-2)↑[0]) ⍳6` | `⟨0 1 4 5⟩` | `unsupported axis operator: ↑` | **A2 needed** |
| P12 | Full output3.kap:118 | (complex) | `unsupported axis operator: ↑` | **A2 fixes this** |

**Key:** P12 at clean HEAD = eval failure (A2), NOT parse error. The implementor's "parse error 1:90" was a stale binary artifact.

---

## Recommended Path Forward (3 commands, definitive)

```bash
# 1. Nuclear clean + rebuild with ONLY evaluator fix
cd ~/Apps/array/rust-kap
git checkout -- kap-core/src/parser.rs   # ensure parser == HEAD
cargo clean
cargo build -p kap-cli --release 2>&1 | tail -5

# 2. Test P12 on verified-fresh binary
./target/release/kap --no-standard-lib <<'EOF'
arrayMaxWidth←80 24
((⌈arrayMaxWidth[0]÷2)↑[¯1+≢⍴1 2 3 4 5 6])«,»((-⌈arrayMaxWidth[1]÷2)↑[¯1+≢⍴1 2 3 4 5 6]) 1 2 3 4 5 6
EOF

# 3. If P12 → correct result: A2 fix is COMPLETE. Done.
#    If P12 → "unsupported axis operator: ↑": A2 fix needs adjustment.
#    If P12 → parse error: something else wrong (report exactly).
```

**Expected result:** P12 produces correct output (A2 fix works). P10 still fails (A1 still needed but separate).

---

## What the Implementor Should STOP Doing

| Waste of Time | Why |
|---------------|-----|
| Tracing parser.rs execution paths | Binary was stale; traces don't match running code |
| "Reverting parser changes" repeatedly | Parser at HEAD is already correct for P12 |
| Hypothesizing about `parse_paren_accum` Malformed | It's a red herring from stale binary |
| Iterating without `cargo clean` | Cargo caches aggressively; every test is suspect |

---

## What the Implementor Should DO

1. **Accept the probe matrix as ground truth** — P12 at HEAD = eval failure only.
2. **Run the 3-command sequence above** — it's the only way to get a trustworthy binary.
3. **If P12 passes:** Commit evaluator fix, move to A1 (parser) for P10.
4. **If P12 still fails at eval:** Debug `apply_train` AxisApplied unwrap with `eprintln!` in that exact function.
5. **Delete THINKING.md** — it's now a record of debugging a stale binary.

---

## My Confidence

- **95%** that `cargo clean` + eval fix = P12 passes (A2 complete)
- **100%** that parser at HEAD parses P12 correctly (oracle probe P12 at HEAD confirmed)
- **0%** that further parser tracing without clean build yields signal

The implementor is one clean build away from closing A2. The loop ends there.