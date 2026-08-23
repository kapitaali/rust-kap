# CODE ANALYSIS 02 — `defsyntax` / `unwindProtect` never registers via `use()`

**Session:** 02 (2026-08-23). **Role:** analysis only — nothing was edited.
**Input:** `META-INF/PROBLEM.md` + live tree (`HEAD db6b375`, uncommitted debug
instrumentation in parser.rs/evaluator.rs).
**Method:** every claim below was verified empirically on the rebuilt port
(`cargo build -p kap-cli`, then scripted stdin/file probes) and/or against the
Real Kap oracle, plus source reading of both trees. Gates re-run at analysis
time: lib **92/0**, curated **1/0** — i.e. **no test exercises this path at all**
(see §T1).

---

## 0. Executive summary

PROBLEM.md's central mystery ("directive parsed but instruction never reaches the
registration arm") is solved. The primary cause is a **one-token bug**: 
`parse_defsyntax_directive` checks for the opening `{` of the macro body but
never consumes it before calling `parse_block()`, which assumes `{` was already
consumed. The resulting mis-parse swallows the body *and its closing brace*,
runs the tokenizer to end-of-input, and errors — so `Instr::DefSyntax` is never
produced, nothing registers, and every subsequent construct in the file dies.

Around that primary bug sit **five more defects** PROBLEM.md did not know about,
two of which will block the very next steps after the primary fix:

| ID | Sev | One-line |
|----|-----|----------|
| A1 | **P0** | missing `self.advance()` before `parse_block()` in `parse_defsyntax_directive` (parser.rs:2455–2458) |
| A2 | P2 | `parse_block` treats the tokenizer's trailing `EndOfFile` token as a statement start (parser.rs:280 vs 187) — explains the off-by-one-line `27:1` error |
| A3 | **P1** | `use_file` inserts the **basename** into `include_stack` but the RAII guard removes the **full path** → entries leak; any later `use` of the same file silently no-ops (evaluator.rs:7667 vs 7670) |
| A4 | **P1** | startup stdlib load **fails** (`parse error at 27:1` = `standard-lib.kap`'s `use("structure.kap")`), so in default mode the whole Tier-B surface is dead and the failed load poisons A3 |
| A5 | **P1** | `expand_macro`'s `:function` arm consumes `{` correctly (2596→2599) but `apply_optional_rule` (:function) and `expand_sub_macro` (:function) do **not** check/consume symmetrically — same trap, different sites |
| A6 | P2 | native `int:unwindProtect` errors "Invalid dimensions" after running both blocks when given two brace lambdas; **the oracle rejects that spelling too** — PROBLEM.md's "confirmed working in isolation" premise is stale |
| A7 | P2 | `declare(:export unwindProtect)` errors `undefined symbol` for a macro-trigger-only name → will abort structure.kap **line 7** immediately after A1 is fixed |
| A8 | P3 | `--no-standard-lib` CLI flag is captured as a positional file argument (main.rs:52–58) — the documented escape hatch doesn't work |
| T1 | — | test-coverage gap: gates green while the entire defsyntax path is broken |

Also assessed: PROBLEM.md's "Secondary issue" (Parser.macros static snapshot) is
**largely a non-issue** in the current architecture — see §5.

---

## 1. A1 (P0) — the missing `self.advance()` before the body

### Code

`parser.rs:2454–2458`:

```rust
let rules = self.parse_syntax_rules()?;
self.skip_newlines();
if !matches!(self.peek(), Some(t) if matches!(t.token, Token::OpenBrace)) {
    return Err(self.err("expected '{' before defsyntax body"));
}
let body = self.parse_block()?;
```

`parse_block` documents its own contract (`parser.rs:260–262`):

```rust
/// block := `{` statement* `}`  …
fn parse_block(&mut self) -> Result<Instr, AplError> {
    // Assumes the opening `{` (OpenBrace) has already been consumed.
```

The check at 2455 peeks at `{` but nothing advances past it.

### Cascade (traced step by step)

For `defsyntax unwindProtect (:function statement :function handler) {\n int:unwindProtect statement handler\n}`:

1. Trigger + rules parse fine (DBG confirms `DBG-TRIGGER name="unwindProtect"`).
2. `parse_block()` is entered with `{` still current. Its loop hits `Some(_)`
   → `parse_expr()` → `parse_keyword_prefix` miss → `parse_defsyntax_directive`
   re-entered on the body tokens (this is exactly the observed
   `DBG-DEFSYNTRY name="unwindProtect" ns=Some("int")` — the directive scanner
   running over `int:…` mid-body).
3. `parse_assign`/`parse_apply` see `{`, which the dyadic loop classifies as a
   function atom (parser.rs:1346 `Token::OpenBrace => true`) → the whole body is
   consumed as a lambda operand **including its closing `}`**.
4. Outer `parse_block` resumes at EOF. Because the lexer emits a terminal
   `EndOfFile` token (see A2), the loop's `Some(_)` arm fires once more →
   `parse_primary` catch-all (parser.rs:2358) → `parse error at 27:1`.
5. `eval_string_in_env` therefore never yields a `DefSyntax` statement — matching
   PROBLEM.md's evidence #2 verbatim (`DBG-STMT tag=DefSyntax` count 0;
   `DBG-DEFSYNTAX register:` count 0).

### Empirical proof

Every prefix of structure.kap fails exactly one line past its end — the parse
consumed the closing `}` and ran to EOF:

| input | error |
|-------|-------|
| head −5  | `parse error at 6:1` |
| head −7  | `parse error at 8:1` |
| head −13 | `parse error at 14:1` |
| full (26 lines) | `parse error at 27:1` |

And the single-line form `defsyntax mfoo (:value v) { v } ⋄ mfoo 42` errors at
`2:1` (the line *after* the directive) with `mfoo` undefined — same shape, any
rule list, any body.

### Kotlin ground truth

`array/src/commonMain/kotlin/com/dhsdevelopments/kap/syntax/syntax.kt:262–278`
(`processDefsyntax`): reads trigger, `processPairs` (rules), then
`tokeniser.nextTokenWithType<OpenFnDef>()` — **consumes** the `{` — then
`parser.parseValueToplevel(CloseFnDef)` parses to the matching `}`. The Rust
port split this into peek + `parse_block(consumed-brace)` but dropped the
consume. `processDefsyntaxSub` (same file, :251) is identical — so the fix must
cover both, though they share the one code site here.

### Suggested fix (for the editor)

Insert `self.advance();` between the OpenBrace check and `parse_block()` at
parser.rs:2458. **Regression traps:**

- Do **not** also add a consume inside `parse_block` — its other callers
  (parse_primary:2279–2282, parse_function_atom:2077–2082, `⇐` RHS, if/while
  bodies) already consume before calling; a second consume would break every
  lambda in the language. Gate: lib suite's many `{…}` tests.
- Do **not** add a consume to `expand_macro`'s `:function` arm — it already
  checks-then-consumes correctly (2596 check → 2599 `parse_block`; verified
  consistent). The sites that need the same audit are A5 below.
- Verify afterwards with BOTH forms:
  - multi-line `defsyntax unwindProtect (…) { \n … \n }` then
    `unwindProtect { io:print "x" } { io:print "y" }` → expect `yx`,
  - single-line `defsyntax mfoo (:value v) { v }` then `mfoo 42` → `42`.

---

## 2. A2 (P2) — EOF is a real token; `parse_block` mistakes it for a statement

The lexer emits a terminal `Token::EndOfFile` (parse_statements guards it at
parser.rs:187; the dyadic loop excludes it at :1059). But `parse_block`'s match
arms handle `CloseBrace`, separators, `Some(_)`, and `None` — so at end-of-input
the `Some(EndOfFile)` falls into the **statement-start arm**, calls
`parse_expr`, and lands in `parse_primary`'s catch-all (parser.rs:2358),
reporting the error at the EOF *position* — which is why every overrun manifests
as `N+1:1`. This is why PROBLEM.md saw "27:1" for a 26-line file and couldn't
locate the offending construct by the line number alone.

Suggested hardening (secondary to A1): treat `EndOfFile` in `parse_block` like
`None` ("expected '}' to close block"), or strip the terminal EOF token in the
tokenizer. Low risk either way; A1 alone stops the bleeding.

---

## 3. A3 (P1) — include-stack key mismatch makes `use()` silently no-op

`evaluator.rs`:

```rust
7667:  self.include_stack.borrow_mut().insert(basename);        // "structure.kap"
7668:  let _guard = IncludeGuard {
7669:      stack: self.include_stack.clone(),
7670:      name: path.to_string_lossy().into_owned(),           // "/home/…/structure.kap"
7671:  };
7630:  if stack.contains(&basename) { return Ok(Rc::new(APLValue::Null)); }
```

The guard pops a **different key** than insert pushed, so every loaded file
stays in `include_stack` for the life of the Engine (until the same full-path
string happens to be checked — never). Consequence, reproduced live:

1. Startup tries `standard-lib.kap` → chain reaches `structure.kap` → A1 parse
   error → load aborts **after** the insert (insert happens before parse).
   `"structure.kap"` now leaks in the set.
2. User types `use("structure.kap")` at the REPL → guard check hits the leaked
   entry → returns `Ok(Null)` instantly. Zero eval traces, no error — exactly
   the "silently does nothing" PROBLEM.md struggled with (its instrumented run
   showed only `DBG-STMT tag=Apply pos_after=5` for the use line and nothing
   else).

Fix suggestion: make `IncludeGuard` store/derive the same string that was
inserted (pass `basename.clone()` as the guard name), or normalize both sides
to the resolved canonical path. Also consider whether a **failed** load should
pop the entry eagerly (it currently does pop the wrong key; after the key fix a
failed load will pop correctly via Drop — good), and whether repeat-`use` of an
already-loaded file should re-evaluate or stay idempotent-by-design (Kotlin
re-includes are allowed; the recursion guard should only block the in-flight
cycle — the current design intent matches that, once keys agree).

---

## 4. A4 (P1) — the startup stdlib load is failing today

Default-mode REPL (no flags) prints:

```
warning: standard library failed to load: parse error at 27:1: unexpected token in primary
```

`standard-lib.kap` line 27 is its `use("structure.kap")` — i.e. **A1 is what
kills the default startup**, and with it `when`, `unwindProtect`, and every
other stdlib symbol in default mode. PROBLEM.md's reproduce command filters this
warning out (`grep -v 'standard library failed to load'`), which hid the fact
that the environment under test was already degraded. Suggestion: keep the
warning visible in future repro scripts; treat a red startup load as the first
suspect whenever `use`d symbols come up undefined.

---

## 5. PROBLEM.md's "Secondary issue" — assessed as mostly moot

Claim: "macros cloned per-statement… a defsyntax registered by statement N is
NOT visible to the parser of statement N+1."

Reading `eval_string_in_env` (evaluator.rs:414–446): the `macros` snapshot is
taken **inside the statement loop**, after the previous statement's
`eval_instr` ran — so statement N+1's parser sees N's registration. Cross-call
(REPL line 2 after `use` on line 1) likewise re-snapshots at the new call's
first iteration. The design is sound; the observed failures are fully explained
by A1/A3/A4. No change needed for correctness.

Two genuine residual notes on macros (minor):
- Keying: the DefSyntax arm qualifies by the *trigger's* namespace field
  (`kap:foo` if written `defsyntax kap:foo …`), while `parse_primary` looks up
  **bare** names only (namespace.is_none() gate, parser.rs:2118). A namespaced
  trigger registers under a key the parser can never hit. structure.kap uses a
  bare trigger, so this doesn't bite today; worth a comment or an assert.
- Perf/noise: the `DBG-MACRO miss` print fires for **every bare identifier**
  parsed (see any probe output — dozens per line). Beyond removal (§9), this is
  a hot-path eprintln that measurably slows file loads; remove before commit.

---

## 6. A5 (P1) — asymmetric brace consumption in the other macro paths

Same trap as A1, three sibling sites in the expansion machinery:

| site | rule arm | status |
|------|----------|--------|
| `expand_macro` :function/:nfunction (2594–2604) | check `{` at 2596 → `parse_block()` 2599 | **BUG — no consume** |
| `apply_optional_rule` :function (2724–2729) | `parse_block()` directly | **BUG — no check, no consume** |
| `expand_sub_macro` :function (2787–2794) | `parse_block()` directly | **BUG — no check, no consume** |
| `expand_macro` :value/:exprfunction (2605–2632) | check `(` → advance → … → expect `)` | correct |
| `expand_sub_macro` :value family (2795–2808) | check `(` → advance → expect `)` | correct |

These run when *expanding* a macro whose `:function` rule binds `{ … }` — i.e.
exactly `unwindProtect { io:print "x" } { io:print "y" }` at the call site once
registration works. With the outer brace unconsumed, the first `:function`
binding would swallow `{ A } { B }` as one nested lambda-pair and desynchronize
the stream (same failure family as the pre-expansion probes: `ch←65 ⋄ ch-{…}`
errors "unexpected token in primary"). Fix suggestion: mirror the
`:value` pattern — check `OpenBrace`, `self.advance()`, then `parse_block()` —
in all three arms. Gate afterwards: `unwindProtect {…}{…}` expands AND
`when { (2>1){result←"a"} }` still returns `"a"` (`when` goes through
`:repeat`/`expand_sub_macro`).

---

## 7. A6 (P2) — `int:unwindProtect` argument shape + stale premise

PROBLEM.md asserts `int:unwindProtect { io:print "x" } { io:print "y" }` "is
already confirmed working in isolation: prints yx". Current reality:

- **Oracle:** that bare spelling errors (`No arguments specified for function`)
  — Kotlin strands `int:unwindProtect statement handler` only *inside the macro
  body* where `statement`/`handler` are bound variables; two literal braces at
  top level do not strand into a monadic pair there.
- **Port:** runs both blocks (`yx` printed) and THEN errors
  `Invalid dimensions in unwindProtect call` (evaluator.rs:1394–1423). Root
  shape problem: `f {A} {B}` binds the first `{A}` as the dyadic right operand
  (brace = function atom, parser.rs:1346) and the second `{B}` as *its* right
  operand → the engine receives left=`{A}`, right=`{B}` (or a nested apply), not
  the `[fn, handler]` vector the arm expects; the side effects still fire
  because the blocks evaluate during arg forcing.

So the port's dyadic fallback prints yx by accident and then errors; the oracle
rejects up front. After A1+A5 make the macro work, the macro path passes bound
lambdas and this raw-spelling divergence becomes low priority — but the
"works in isolation" belief should be corrected in docs, and the eval arm's
error-after-side-effects ordering is worth a look (validate shapes BEFORE
forcing/running anything).

---

## 8. A7 (P2) — `declare(:export <macro>)` will abort structure.kap line 7

Reproduced: `declare(:export zork)` (name bound to nothing) → 
`error: undefined symbol: zork`. Once A1 lands, `unwindProtect` exists only as
an Engine.macros entry — **not** as an environment symbol — so structure.kap's
line 7 `declare(:export unwindProtect)` will throw and abort the remainder of
the file (including `when`'s definitions, which come later). The oracle
tolerates this (its `declare(:export when)` yields `⊢ kap:when` — observed in
the ground-truth probe). Suggested direction: `declare(:export X)` should
resolve X as symbol OR macro trigger (and error only if neither), matching the
oracle. Probe the oracle for `declare(:export nonexistent)` to pin the exact
tolerance before coding.

---

## 9. Housekeeping

**Debug instrumentation currently in tree (all counted; remove before commit):**

| location | tag |
|----------|-----|
| parser.rs:2121 | `DBG-MACRO hit` |
| parser.rs:2125 | `DBG-MACRO miss` (hot path — every identifier) |
| parser.rs:2426 | `DBG-DEFSYNTRY` |
| parser.rs:2447 | `DBG-TRIGGER` |
| evaluator.rs:406 | `DBG-EVALINENV mark` |
| evaluator.rs:441 | `DBG-STMT` |
| evaluator.rs:646 | `DBG-DEFSYNTAX register` |

Matches PROBLEM.md §"Debug instrumentation" (its line numbers drifted slightly;
these are current).

**A8 (P3):** main.rs:52 sends any `-`-prefixed arg that isn't `-n`/`--no-*`
through the flag-ignore branch, but the final `else` still captures
`--no-standard-lib` as the positional file — net effect: `kap --no-standard-lib`
tries to open a file named `--no-standard-lib` (`cannot read … os error 2`) and
the stdlib loads anyway. Fix: exclude `a == "--no-standard-lib"` in the arg loop
like `-n`. This mattered here: it forced every diagnostic in this session onto
the polluted default-mode path until worked around.

**T1 — coverage gap:** lib 92/0 and curated 1/0 pass while the entire
defsyntax→macro→stdlib path is broken, because no Rust unit test parses a
`defsyntax` or loads structure.kap. Per D5 the real gate is Kap-native: suggest
a minimal `kap-stdlib/test/macro_smoke.kap` (define a trivial defsyntax, use it,
plus `use("structure.kap")` + `unwindProtect`/`when` cases) wired into the
existing Kap-native harness so this surface can never silently rot again.

---

## 10. Recommended action order (for the editor)

1. **A1** — one-line `self.advance();` at parser.rs:2458 (before `parse_block`).
   Rebuild `kap-cli`; probe: prefix-of-structure.kap files must stop erroring
   one-past-EOF; `head -26` must load clean.
2. **A5** — add check+consume of `{` in the three :function arms (2596→2599 is
   already correct-shaped: copy it; 2726 and 2789 need the guard added).
   Probe `unwindProtect {…}{…}` → `yx` and `when{…}` → `"a"`.
3. **A3** — align IncludeGuard key with the inserted basename. Probe: two
   consecutive `use("io.kap")` lines both take effect; failed-load retry works.
4. **A7** — teach `declare(:export X)` about macro triggers (probe oracle first).
5. Remove all 7 DBG prints (grep gate: `grep -rn 'DBG-' kap-core/src/` → empty).
6. Fix **A8**, then re-run the FULL startup path with no flags: default REPL
   must load standard-lib clean (A4 closes automatically once A1/A3/A7 land).
7. Gates: lib 92/0, curated 1/0, plus the new T1 smoke file.
8. Defer: A2 hardening, A6 shape-validation reorder, macro keying note.

---

## Appendix — probe log (condensed)

| # | input (mode) | result |
|---|--------------|--------|
| P1 | `use("structure.kap")` then `unwindProtect{…}{…}` (default REPL) | `⍬` from use (silent no-op, A3); `undefined symbol: unwindProtect` |
| P2 | same (oracle) | `xy` printed, result `"x"` ✓ |
| P3 | head −5/−7/−13/−26 of structure.kap as script | `parse error at 6:1 / 8:1 / 14:1 / 27:1` — always EOF+1 (A1/A2) |
| P4 | single-line `defsyntax mfoo (:value v) { v }` + `mfoo 42` | `parse error at 2:1`, `undefined symbol: mfoo` (A1, rule-list-independent) |
| P5 | DBG trace of structure.kap parse | directive re-entered on `int:`/body tokens mid-parse (A1 cascade step 2) |
| P6 | `int:unwindProtect {io:print "x"} {io:print "y"}` (port) | prints `yx` then `Invalid dimensions in unwindProtect call` (A6) |
| P7 | same (oracle) | `Error: No arguments specified for function` (A6 — premise stale) |
| P8 | `declare(:export zork)` (port) | `undefined symbol: zork` (A7) |
| P9 | gates at analysis time | lib `92 passed; 0 failed`, curated `1 passed; 0 failed` (T1) |
