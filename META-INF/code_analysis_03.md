# CODE ANALYSIS 03 — Tier A `⌸` operator value-reference blocker (PROBLEM.md review)

**Session:** 03 (2026-08-23). **Role:** analysis only — nothing edited.
**Input:** rewritten `META-INF/PROBLEM.md` (operator-value-reference blocker).
**Method:** every oracle claim re-probed against `kap-jvm-text`; every port claim
re-probed on the rebuilt binary (`HEAD 5d2a97c`, tree clean of DBG prints,
gates green: lib 92/0, curated 1/0). Kotlin ground truth read at
`parser.kt:939–985`, `parser.kt:768–799`, `common.kt:173/186`.

---

## 0. Verdict up front

PROBLEM.md's two central premises **both fail oracle verification**, and its
minimal repro is flawed. There *are* real bugs in this area — including one
genuine panic — but they are not the ones the write-up describes, and the
proposed fix ("make bare `⌸` return the operator value") would move the port
**away** from Real Kap, not toward it.

| PROBLEM.md claim | Verified reality |
|---|---|
| "Real Kap treats an operator as a first-class value; referencing `⌸` yields the operator" | **False.** Oracle: bare `⌸` → parse error `Operator without left function: ⌸`; `foo ⇐ ⌸` → same; `typeof ⌸` → `No arguments specified for function`. Kotlin `parser.kt:967–971` throws `InvalidOperatorArgument` at **parse time** for any operator name not carrying a function operand. Operators live in `engine.operators` (`parser.kt:778/798`), never as variables. |
| "`data ⌸ fn` is the calling convention" (`1 1 2 3 3 ⌸ +`) | **False.** Oracle: `Error: Operator without left function: ⌸`. The real convention is operand-on-the-LEFT: `keys (≠⌸) values` (works on both engines), or unparenthesized `keys ≠⌸ values` (works on oracle; port gaps — B7). |
| "use(base-functions.kap) aborts wholly at line 13 on the oracle" | **Half false.** The oracle *does* die at 13 (`Assignment to constant variable: kap:⎕A`) — but `⌸` (lines 3–8) still registers and works afterwards (probe OJ: grouping returns `⟨2 2⟩`). |
| "the oracle continues past per-statement errors" (secondary issue) | **Backwards.** Controlled test (`conttest.kap`: assign, error, assign) proves the oracle **aborts the file at the first statement error** — `q2` was never assigned. The port's committed tolerate-per-statement behavior (commit `3a54593`) is *more lenient than the oracle*. |
| Root cause "eager OpCall in parse_apply:939–967 makes bare refs run the body" | **Partly right, wrong mechanism.** Bare `⌸` on the port returns `<operator>` cleanly (no body run). What actually executes the body is `⟨known_fn⟩ ⟨known_op⟩` adjacency — e.g. `typeof ⌸` — which the detector at parser.rs:946–984 parses as an OpCall with `typeof` as the operator's left function operand. See B1. |

Why the write-up's own repro printed `undefined symbol: keyindex`: the inline
repro collapses the two body statements into ONE
(`keyindex ← =keys ((≠keyindex)/…) …` — the newline separating them was lost),
making the assignment self-reference `keyindex`. Any engine that runs that body
errors there. The real stdlib file separates the statements and is fine.

---

## 1. Verified behavior matrix (all probes fresh, both engines)

| form | oracle | port (current tree) | verdict |
|------|--------|---------------------|---------|
| `keys (≠⌸) values` (after def / after use) | works, grouped table | works, identical shape `(3 (1 1 1 1 1) 1 (1 1 1 1))` | ✓ core feature OK on both |
| `use("base-functions.kap")` | errors at 13, `⌸` still callable | **loads clean, `⌸` callable** | port "exceeds" oracle only because `:const` is a no-op (B6) |
| bare `⌸` (defined) | parse error `Operator without left function` | returns `<operator>` value | **divergence** (B2) |
| `typeof ⌸` | parse error `No arguments specified for function` | **runs the operator body → panic** `evaluator.rs:4518 index out of bounds` | **real bug** (B1+B3) |
| `foo ⇐ ⌸` | parse error `Operator without left function` | accepted, `<function>` | **divergence** (B2) |
| `1 1 2 3 3 ⌸ +` (PROBLEM.md's "convention") | parse error | parse error `unexpected token` | both reject; PROBLEM.md's expectation wrong |
| `keys ≠⌸ values` (unparenthesized derivation) | works | `≠ requires numbers` (mis-parse) | port gap (B7) |
| `declare(:export ⌸)` | tolerated (file loads to 13) | works (structural extraction) | ✓ |
| `⎕A` fresh session (no files) | `"ABCDEFGHIJKLMNOPQRSTUVWXYZ"` — **native built-in**, `typeof ⎕A → kap:array` | `undefined symbol: ⎕A` | port gap (B5) |
| `declare(:const x)` then `x ← 6` | `Assignment to constant variable` | silently reassigned | port gap (B6) |
| `use()` hitting a bad statement | **abort file**, keep earlier defs | warn + continue | policy divergence (B4) |

Key Kotlin facts behind the matrix:
- `parser.kt:967–971` — name resolution order is `lookupFunction` → **else if
  `getOperator(symbol) != null` → throw `InvalidOperatorArgument(symbol, pos)`**
  → else variable ref. An operator name is *never* a value.
- `common.kt:186` — the exception text `Operator without left function: ${name}`.
- `common.kt:173` — `IllegalContextForFunction` = `No arguments specified for
  function` (what the oracle emits for `typeof ⌸`, where the partial operator
  derivation lands in a value context).
- `syntax.kt:262–278` + `parser.kt:768–799` — operators register into
  `engine.registerOperator`, a table separate from functions/variables.

Also relevant: the oracle's bundled `standard-lib/base-functions.kap` and the
vendored `kap-stdlib/std/base-functions.kap` are **byte-identical**
(`diff` exit 0) and both are *stale relative to the current engine*: they assign
`⎕A ← @A…@Z` although current Kap ships `⎕A`/`⎕a`/`⎕d` as native read-only
constants (fresh-session probes OF). That staleness is why the oracle's own
load explodes at line 13.

---

## 2. Findings

### B1 (P1) — `⟨known_fn⟩ ⟨known_op⟩` adjacency mis-parses as an OpCall and EXECUTES the operator body

`typeof ⌸` parses via the `is_first_fn` detector (parser.rs:946–984): `typeof`
is a known function → `is_first_fn` true → next token `⌸` ∈ `known_ops` → the
block consumes it and builds
`OpCall { op: ⌸, left_fn: typeof, right_fn: None }`. At eval,
`apply_user_op` runs the operator **body** with a junk left function operand;
inside the body `⍞fn¨ keyindex⫇values` reaches `group_indices` with degenerate
args. Oracle behavior for the same input: clean parse-phase rejection. The
detector is correct for genuine derivations (`⊢ «…» ⌸`-style operand binding);
what it must never do is let the constructed OpCall reach eval *incomplete*
(no data args, no right operand) — that case is the port's equivalent of
Kotlin's `InvalidOperatorArgument`.

### B2 (P2) — operator-as-value is silently accepted

Bare `⌸` → `<operator>`; `foo ⇐ ⌸` → `<function>`. Real Kap forbids both at
parse time. PROBLEM.md proposes *codifying* the value behavior — do NOT. The
leniency is the divergence.

### B3 (P1, crash) — unguarded `b_dims[0]` in `group_indices`

`evaluator.rs:~4518`: after checking `a_dims.len() == 1` it indexes
`b_dims[0]` without checking `b_dims.len()`. A scalar right argument panics
(`index out of bounds: the len is 0 but the index is 0`) instead of returning
the size-mismatch error. Independently worth fixing; today it is the visible
tip of B1.

### B4 (P2, policy) — `use()` error tolerance exceeds the oracle

Oracle: abort file at first failing statement, keep definitions from earlier
statements (probes OH, OE, OG). Port (commit `3a54593`): warn and continue.
This was built on PROBLEM.md's inverted premise. Decide deliberately:
(a) match the oracle (abort), or (b) keep tolerance as a documented extension.
Given the project's conformance goals, (a) is the safer default — but note it
changes `standard-lib.kap` startup dynamics once B5/B6 land (see §4).

### B5 (P2) — missing native `⎕A` / `⎕a` / `⎕d`

Current Kap ships these as native read-only constants (fresh-session oracle
probes). The port only has them via the stdlib file. Implementing them natively
(alphabet/digits char vectors) removes the dependence on the stale file and is
required for any attempt to mirror oracle load behavior.

### B6 (P3) — `declare(:const …)` is a no-op

`eval_declare` (evaluator.rs:5521) handles only `:export`; `:const` marks
nothing and nothing enforces it (probe Q4: reassignment succeeds). Oracle
enforces (`Assignment to constant variable`). Low priority until B5 lands,
since enforcing it against the current vendored file would reproduce the
oracle's line-13 explosion.

### B7 (P3) — unparenthesized derivation `keys ≠⌸ values` unsupported

Works on the oracle; the port errors (`≠ requires numbers`). Minor coverage
gap in the same family; the parenthesized form — the one the stdlib and tests
use — already works.

### T2 (process) — PROGRESS/PROBLEM discipline note

PROBLEM.md's oracle section contains predictions ("result like
`(1 2 3)(1 2)(1 1 2 3 3)`") that were never actually fired — the documented
commands were not run (or their output ignored). Every premise in this file
was checkable in under a minute with the documented probe pattern. Suggest a
hard rule: **an oracle block in PROBLEM.md must contain captured output, not
predicted output** — same spirit as the existing "never answer what Kap does
without evaluating" rule, extended to problem write-ups.

---

## 3. Recommended fix order (for the editor)

1. **B3** — guard `b_dims` in `group_indices` (return the existing
   size-mismatch error when `b_dims.len() != 1`). Trivial, kills the panic.
2. **B1** — in the `is_first_fn` OpCall detector (parser.rs:946–984), reject
   an OpCall that would stand with no function-operand/data structure when it
   reaches eval: specifically, when the OpCall is built from
   `⟨known_fn⟩ ⟨known_op⟩` and neither a right function operand nor a trailing
   data argument follows, raise the Kotlin parse error
   `"Operator without left function: <name>"`. Mirror `InvalidOperatorArgument`,
   don't invent text. Probe `typeof ⌸` → clean parse error, no body execution,
   no panic.
3. **B2** — extend the same rejection to value positions: bare known-op symbol
   at statement/value level, and `⇐` RHS. Keep the working exceptions:
   parenthesized derivation `(≠⌸)`, structural slots (`declare(:export ⌸)`),
   and the detector's legitimate operand-binding path. Target oracle texts:
   `Operator without left function: ⌸` for bare/`⇐` cases.
   *Caution:* this touches the parser's hottest heuristic — land it behind the
   full regression list in §4.
4. **B4** — flip `use()` to abort-at-first-error (oracle semantics), or
   explicitly document the tolerance as an intentional extension in
   KNOWN-NONCONFORMANCE.md. Do not leave it accidental.
5. **B5** — native `⎕A`/`⎕a`/`⎕d` (both registration lists, plus read-only
   treatment once B6 exists).
6. **B6** — `:const` enforcement (assignment-time check; error text
   `Assignment to constant variable: <ns>:<name>`).
7. **B7** — unparenthesized `≠⌸` chaining (mirror whatever the detector does
   for `(≠⌸)` at the strand-loop level). Lowest priority.
8. Update the vendored `base-functions.kap` only after B5/B6 exist and the
   decision on B4 is made — otherwise you will faithfully reproduce the
   oracle's self-destructing load. (Check upstream `array/standard-lib/` for a
   refreshed file first; if upstream still carries the stale assignments,
   prefer keeping the port's cleaner load and documenting the delta.)
9. Gates after each step: `cargo test -p kap-core --lib` (92/0),
   `curated_kap_parity` (1/0), plus §4's probe list.

## 4. Required non-regression probes (run after B1/B2)

Both engines side-by-side, expect identical *behavior class* (value vs error):

```bash
# must KEEP WORKING (port == oracle):
printf '∇ (keys) (fn ⌸) (values) { keyindex ← =keys ⋄ ((≠keyindex)/keys) ,[0.5] ⍞fn¨ keyindex⫇values }\n3 1 3 1 3 1 3 1 3 (≠⌸) 1 2 3 4 5 6 7 8 9\n'
printf 'use("base-functions.kap")\n3 1 3 1 3 1 3 1 3 (≠⌸) 1 2 3 4 5 6 7 8 9\n'
printf '2 -foo+ 3\n'                      # after ∇ (x foo) a def — derived-op call form
printf 'declare(:export ⌸)\n'
# must NOW ERROR like the oracle (currently silent/wrong):
printf '⌸\n'                              # Operator without left function
printf 'foo ⇐ ⌸\n'                        # same
printf '∇ (k) (f ⌸) (v) { ⍵ }\ntypeof ⌸\n' # parse error, NEVER run body / panic
```

Plus the standing suites and the existing operator tests
(`eval_two_arg_operator_with_destructured_left_params` et al.).

## Suggested fix order

Guard `b_dims` (kill the panic) → reject incomplete OpCall with Kotlin's exact error text → extend rejection to value positions (carefully — hottest parser path) → decide `use()` abort-vs-tolerate deliberately → add native quad constants + `:const` enforcement last, since enforcing consts against the stale file would faithfully reproduce the oracle's self-destructing load.

One process note included: PROBLEM.md's "Oracle (ground truth)" section contains predicted outputs, never captured ones. Suggest a rule going forward — oracle blocks must contain pasted transcript, not expectations.

---

## Appendix — probe log (condensed)

| # | input (engine) | result |
|---|----------------|--------|
| P1 | `typeof ⌸` after def (oracle) | `Error: No arguments specified for function` |
| P2 | `1 1 2 3 3 ⌸ +` (oracle) | `Error: Operator without left function: ⌸` |
| P3 | bare `⌸` / `foo ⇐ ⌸` (oracle) | `Error: Operator without left function: ⌸` |
| P4 | `k (≠⌸) v` after `use(base-functions.kap)` (oracle) | line-13 const error, then grouped table `⟨2 2⟩` — ⌸ survived |
| P5 | `conttest.kap` (assign, error, assign) via use (oracle) | file aborted at error; later name unassigned → **abort semantics** |
| P6 | `⎕A` / `typeof ⎕A` fresh oracle session | `"ABCDEFGHIJKLMNOPQRSTUVWXYZ"` / `kap:array` — **native** |
| P7 | `declare(:const x); x←6` (oracle) | `Assignment to constant variable: default:x` |
| P8 | `keys ≠⌸ values` unparenthesized (oracle) | works |
| P9 | bare `⌸` (port) | `<operator>` — lenient value |
| P10 | `typeof ⌸` (port, proper body) | **panic** `evaluator.rs:4518` |
| P11 | `use("conttest.kap")` (port) | warns, continues, `q2` assigned → **tolerance divergence** |
| P12 | `declare(:const x); x←6` (port) | silently reassigns |
| P13 | vendored vs oracle `base-functions.kap` | byte-identical (`diff` clean), both stale vs engine |
| P14 | gates at analysis time | lib 92/0, curated 1/0 |
