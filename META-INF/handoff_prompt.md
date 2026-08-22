# HANDOFF PROMPT — rust-kap io.kap port (b3c / b3d / parser `-` regression)

**Project:** `~/Apps/array/rust-kap` (branch `feature/wheres-extra`, HEAD `a2bffde` + uncommitted work). Rust rewrite of the Kap array language. Goal for this thread: load `standard-lib.kap` chain incrementally, gated by side-by-side oracle probes. Sub-target: **`io.kap` end-to-end** — `fromHex`/`toHex`/`base64Encode`/`base64Decode` matching the Real Kap oracle.

**TRUTH SOURCE (do not violate):**
- Oracle binary: `~/Apps/array/kap-jvm-text/bin/kap-jvm-text --lib-path=$HOME/Apps/array/kap-jvm-text/standard-lib`. Port: `./target/debug/kap --lib-path=kap-stdlib/std`.
- Kotlin source (READ only, never run): `~/Apps/array/array`.
- **Stale-binary rule (critical):** after ANY parser/evaluator edit, `cargo build -p kap-cli` (NOT just `cargo build -p kap-core`). If suspicious, `cargo clean -p kap-core && cargo build -p kap-cli`. But see Pain Point 1 — `cargo clean` surfaces REAL bugs too; use `git stash` + rebuild + re-probe as the discriminator (skill calls this mandatory).
- Gates: `cargo test -p kap-core --lib` (currently 92/0) and `cargo test -p kap-core --test conformance curated_kap_parity` (1/0). IGNORE `run_kotlin_conformance` (it's `#[ignore]`d — hangs on incomplete builtins).
- User directive: **document to `META-INF/PROGRESS-<date>.md` before continuing** (meticulous, with file:line + oracle-vs-port result). Reload skill `rust-kap-dev` (pruned this session) before continuing — it contains the full STALE-BINARY / `next_is_paren_operator` pitfall notes that explain this exact regression class.

---

## WHAT IS DONE (all uncommitted; nothing since HEAD `a2bffde`)
- `and`/`or` short-circuit boolean ops (`BooleanOpKind` in `ast.rs`, `parse_assign` boolean loop in `parser.rs`, `eval_boolean_op` in `evaluator.rs`).
- `int:throwNative` dyadic (`evaluator.rs`).
- `∊` membership scalar fix (`evaluator.rs` — preserves left shape so the `and` guard in io.kap works).
- `|` (residue) modulo arg-swap fixed (`number.rs` `modulo()` → `b.rem_euclid(a)`).
- `apply_rank_op` panic guard (`evaluator.rs` ~5456).
- Two debug `eprintln!`s currently in the source that MUST be removed before commit (see Pain Point 2): parser.rs `parse_primary` catch-all (line ~2171) and `parse_apply` dyadic loop top (line ~1266). There are ALSO leftover `DBG` prints from earlier rounds that were already removed — verify with `grep -nE 'eprintln!\("DBG' kap-core/src/parser.rs` before building.

---

## THE CURRENT BLOCKER (root-caused, NOT yet fixed)

**Symptom:** On a clean build, `3 - 4`, `10 - 5`, `ch-@\0`, `65-@\0`, `io:fromHex "FF"`, `io:toHex 255` ALL fail. `2 + 3`, `1+1`, `io:base64Encode` (before `use` load) work. But `git stash` (baseline `a2bffde`) handles `3 - 4 → ¯1` and `use("io.kap") ⋄ io:toHex 255 → "FF"` correctly — so **my changes regressed `-` parsing and io.kap loading.**

**Root cause (confirmed via targeted debug, see Evidence):** The `L f R` block in `parse_apply` (parser.rs ~lines 1144–1196) treats `-` as a *function atom* and calls `parse_function_expr()` for it. But `parse_function_atom` has a unary-minus arm I added at ~line 1919:
```rust
Token::Literal(LiteralValue::Symbol { name, .. }) if name == "-" && namespace.is_none() => {
    self.advance();
    let operand = self.parse_function_atom()?;
    return Ok(Instr::Apply { fn_expr: Symbol("-"), left: None, right: operand });
}
```
That arm **consumes the right operand** (`4` in `3 - 4`, `(@\0)` in `ch-@\0`). So when the `L f R` block then calls `let right = self.parse_apply()?` (line 1190), there's nothing left → it hits the trailing `Newline` → `parse_primary` catch-all → "unexpected token in primary".

`+` works because it has NO unary arm in `parse_function_atom`, so `parse_function_expr()` returns `+` cleanly and the `L f R` block's separate `right = parse_apply()` gets `3`.

**The two unary-minus arms I added and their fates:**
1. **`parse_apply` entry arm** (~line 805, gated `if name == "-" && namespace.is_none()`): this one is CORRECT — it only fires when `-` is the FIRST token of a value expression, returns monadic negate, does not break dyadic subtraction (verified `3 - 4` reaches it with peek=`3`, skips). KEEP.
2. **`parse_function_atom` arm** (~line 1919): this is the BUG. It was added to make `(-x)` / `(-padding)` work inside parenthesised groups (OpenParen → `parse_function_expr` → `parse_function_atom`), but it ALSO fires inside the `L f R` block's `parse_function_expr()` call for a dyadic `-`, swallowing the right operand.

**Why `(-x)` needs SOMETHING:** the lexer folds `-<digit>` into a negative number, but `-x` (variable) is lexed as Symbol `-` then Symbol `x`. Inside `(...)`, the OpenParen path calls `parse_function_expr` → `parse_function_atom`, which returned bare `Symbol("-")` → "undefined symbol: -". So `(-x)` genuinely needs unary handling. The fix must distinguish dyadic `3 - 4` (right operand belongs to OUTER `L f R`) from unary `(-x)` (right operand belongs to the `-` itself).

**The fix (NOT yet applied — candidate approaches):**
- Option A: In `parse_function_atom`'s `-` arm, only treat as unary if the operand after `-` is NOT a *value* that the `L f R` block would want. But that's fragile.
- Option B (cleaner): Have the `L f R` block (1144) detect `peek == "-"` and NOT route it through `parse_function_expr()`; instead handle `-` as a dyadic operator directly (skip the function-expr parse, go straight to `right = parse_apply()` and build `Apply{Symbol("-"), left: Some(first), right}`). This matches how `+` works via the generic path.
- Option C: Remove the `parse_function_atom` unary arm; handle `(-operand)` at the `parse_function_expr`/`OpenParen` boundary only.
- **Recommend validating against the oracle** that `(-x)`, `(-5)`, `3 - 4`, `ch-@\0` (io.kap line 4 `code ← ch-@\0`), `(-padding)↓` (base64Encode) all work before committing.

---

## EVIDENCE (exact, reproducible)
```
# clean build first
cargo clean -p kap-core && cargo build -p kap-cli

# baseline (proves regression is mine):
git stash
cargo build -p kap-cli
printf '3 - 4\n' | ./target/debug/kap --lib-path=kap-stdlib/std   # -> >>> ¯1  (works)
printf 'use("io.kap")\nio:toHex 255\n' | ./target/debug/kap --lib-path=kap-stdlib/std  # -> "FF" (works)
git stash pop

# my tree (regressed):
printf '3 - 4\n' | ./target/debug/kap --lib-path=kap-stdlib/std
# DBG after first=Literal(Number(Long(3))), peek=Some(Literal(Symbol { name: "-", namespace: None }))
# DBG primary unexpected token: Some(SpannedToken { token: Newline, line: 1, col: 6 })
# parse error at 1:6: unexpected token in primary

printf 'use("io.kap")\nio:fromHex "FF"\n' | ./target/debug/kap --lib-path=kap-stdlib/std
# parse error at 4:15: unexpected token in primary   (io.kap line 4 = `code ← ch-@\0`)
# error: undefined symbol: fromHex

# io.kap full probe (probe_iokap2.sh) now shows ALL functions as
#   port=error: undefined symbol: <fn>  oracle=<correct>
# (toHex matched earlier in the session on an incremental build; the clean build re-exposed the - regression)
```

**Key insight from debug:** the `DBG dyadic iter` print (I added at the dyadic loop top, ~1266) did NOT fire for `3 - 4` — proving the parse returns via the `L f R` block (1144–1196) BEFORE reaching the strand/dyadic loop (~1217/1265). The `DBG after first` print (816) fired with `peek=Symbol("-")`, confirming `first=3` then the `-` took the `L f R` path.

---

## PAIN POINTS
1. **Stale binary vs real regression ambiguity.** `cargo clean` makes a failure look like a regression, but it's often a stale-binary ghost; a plain rebuild then "fixes" it. The skill mandates `git stash` + rebuild + re-probe as the discriminator. This session burned many turns on exactly this. The `io.kap` line-4 `ch-@\0` failure is a REAL regression (the `parse_function_atom` unary arm), not a ghost — confirmed by the `git stash` test.
2. **Debug `eprintln!`s left in source.** I have two active DBG prints (parser.rs ~2171 catch-all, ~1266 dyadic loop) plus the `DBG after first`/`DBG entry unary-minus` prints added during this session. They must ALL be removed (grep `eprintln!\("DBG`) before any commit. They don't affect logic but pollute output and would regress the gate's clean build.
3. **`patch` tool lint noise.** Every `patch` reports a giant red→green diff labeled "Pre-existing lint errors" — that's rustfmt reformatting pre-existing style (e.g. collapsing `match` arms), NOT a breakage. Don't `git checkout` on that signal; verify with `cargo build`.
4. **Stream timeouts on large tool calls.** Oversized `patch`/`terminal` payloads time out. Split into <8K-token calls.
5. **`next_is_paren_operator()` is a known pitfall** (skill flags it). It saves/restores `self.pos` via `saved`/`self.pos = saved` at the end, but the inner `$-` arm (1453–1461) consumes tokens only when the first token IS `(`. For non-`(` current tokens it returns early without