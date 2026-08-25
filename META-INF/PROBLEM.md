# PROBLEM — P7c blocker: paren group containing a nested function group fails to parse

## Symptom
With `f ⇐ {⍵+1}` defined, these statements fail with `parse error … unexpected token
in primary` (error column points INSIDE the inner paren):

```
z ← (a (f))        ← minimal repro (fails in BOTH KAP_KOTLIN_PARSER paths)
declare(:export (f))      ← util.kap:6 shape (single-element export list)
declare(:export (cols col))  ← passes when both are plain symbols, fails when one is fn-valued
```

These all PASS:
```
(a f)        ((f) a)    (1 (f))→runtime err but parses    (kw (1 2))
```

## Evidence / what is ruled out
- NOT eval_declare: it reads the AST structurally and never evaluates the target.
- NOT the train classifier alone: `try_parse_train("(f)")` yields `Train[Symbol(f)]`
  (line ~2748 single-member branch) — fine on its own; `((f) a)` parses OK.
- The failing sub-shape is specifically **value-symbol followed by nested fn-group**
  `(a (f))`. Suspect: `next_is_paren_operator` (~2306) classifies ANY inner `(` as
  `Kind::Func` unconditionally (line ~2382), so `(a (f))` classifies as
  left_bind=[Value,Func] → treated as an operator group → downstream a call site
  reaches `parse_primary` with a `(` token (parse_primary has NO OpenParen arm,
  error text matches :3286).
- Fails identically under `KAP_KOTLIN_PARSER=0` AND default → shared code path
  (likely the legacy dyadic/strand loop at ~2162 or the classifier above).

## Open question
Where exactly does `(a (f))` reach `parse_primary` with an unconsumed `(`?
Candidate fix directions:
(a) In `next_is_paren_operator`, classify an inner `(` group by RECURSING its
    classification instead of assuming Func;
(b) Add an OpenParen arm to `parse_primary` delegating to the same group logic
    used by the legacy primary (so a stray `(` never hits :3286).

## Why it matters
Blocks P7c: `util.kap` line 6 `declare(:export (cols col))` cannot load while
`cols`/`col` hold function values — which they always do after their `⇐`.

## Status — RESOLVED 2026-08-25 (commits 6441a30, a996a24, 15ab3a7)
The full chain now works end-to-end in both parser paths:

```
defsyntax filter (:value arg :function fn) {
  ((toBoolean ⍞fn)¨ arg) / arg
}
toBoolean ⇐ {0≠⍵}
filter (1 2 3 4) {0=2|⍵}   →  (2 4)
```

Root causes fixed:
1. `declare(...)` was routed through train classifiers → `parse_declare_special()`
   parses the paren group structurally (Kotlin DeclareToken semantics).
2. `is_definite_function` rejected known user fns and DynamicRefs → now an
   instance method admitting both.
3. `parse_function_atom`'s OpenParen arm hard-errored on train-parse failure →
   falls back to `parse_function_expr`.
4. `try_parse_train` had no classifier for `[fn-shape, DynamicRef]` → added
   (⍞ref always denotes a function; names resolve at eval).
5. Adverbs after a Train/Lambda/DynamicRef atom (`(f g)¨ xs`) were eaten by the
   monadic-apply block → dedicated arm binds Derived{atom, adverb} first.

Oracle ground truth: the JVM oracle REJECTS inline `z ⇐ ((f g)¨ args)`
("Right side of the arrow must be a function") and its own util.kap
trimLeft/filter fail at load/call. The port's filter macro now exceeds
stock-file behavior. util.kap lines 19–21 (`@\s` regex literals) documented
in KNOWN-NONCONFORMANCE.md as beyond-oracle scope.

---

# PROBLEM 2 — bound-constant commute `2÷⍨` (stat.kap median) — PARTIALLY RESOLVED 2026-08-25

The ⍨ INVERSION, value-tine-in-fork, fork-postfix-left-member, and the
**bare** value-left-bind chain are now FIXED and committed this session.

## Resolved this session (P7d-6)
**Bare value-left-bind chain** `f ⇐ 1 2+≢ ⋄ f 5` → `(2 3)` (oracle
`⟨2 3⟩`, value-exact; display `()` vs `⟨⟩` is the P8 renderer gap).
AST now `Train[ Train[Array(1,2), +], ≢ ]` — value strand binds to the
FIRST function only; remaining fns chain. Also `f ⇐ 1 2+×≢ ⋄ f 5` → `(2 3)`.

Root cause of earlier wrong attempts (P7d-5 reverted, re-approached this
session): Kotlin `Chain2.eval1Arg(a) = fn0(fn1(a))` (instr.kt:588) — `fn0`
applied MONADICALLY to `fn1`'s result — and `processFn`'s FnParseResult
branch builds `Chain2[ makeLeftBindFunction(valuestrand, f0), f1 ]`
(parser.kt:479–491). The port folded the value strand as the *outer*
funcs[0] of the whole chain (wrong) and inverted the fold direction. Fixed
in `parse_function_expr_impl` allow_train arm (parser.rs ~3258): collect
value-strand → left-bind inner `Train[Array(v*), f0]`; remaining fns
left-fold as outer fn0.

## Remaining OPEN item: the PAREN-wrapped form `(¯1r2 0+2÷⍨≢)` (median body)

`stat:median 1 2 3 4` still DIVERGES: oracle `5/2`, port errors. Oracle
dissection (kap-jvm-text, standard-lib) proves the body is NOT a fn-train:

- `¯1r2 0 + 2` → `⟨3/2 2⟩` — a **dyadic VALUE** (strand + scalar), not a fn.
- `(¯1r2 0 + 2) 9` → `⟨⟨3/2 2⟩ 9⟩` — paren-wrapped VALUE applied to an arg
  is a **constant** (`⟨value, arg⟩`, Kotlin `LeftAssignedFunction`).
- `(¯1r2 0 + 2 ÷⍨ ≢) 1 2 3 4` → `⟨3/2 2⟩` — here `÷⍨ ≢` IS consumed as a
  **fn-chain**, so the value `3/2 2` left-binds to it and evaluates
  dyadically: `3/2 2 + (÷⍨≢ vec)` = `3/2 2 + 1` = `3/2 2`.
- `(3/2 2 ÷⍨≢) 1 2 3 4` → `⟨2 2 2 2 2 2⟩` — confirms the value-strand binds
  as the LEFT arg of the commute (`÷⍨` = `y÷x`), NOT as a constant.

So the median body `(A + B ÷⍨ ≢)` is **a dyadic value `A + B` that
left-binds to the fn-chain `÷⍨ ≢`** — the full `parseExpr` accumulator
interleaves VALUE parsing (strand + `+` dyadic) with FUNCTION parsing (the
`÷⍨≢` chain). The port's `try_parse_train` (parser.rs:2596) only accepts
fn-ATOMS, so it tries to fold `¯1r2 0 + 2` as an fn-train member (wrong)
and emits `Train[Array(¯1r2,0), +, …]` nested → evaluates to `2`, not `3/2 2`.

**This is the P1 parser-migration scope** (module-mirror of Kotlin's single
`parseExpr` value/function accumulator), NOT a localized train-classifier
patch. Three prior localized attempts (P7d, P7d-4, P7d-5) reverted; the
correct fix is to give paren groups in fn-chain position a VALUE-expression
parse fallback (Kotlin `parseValue` vs `FnParseResult` at parser.kt:455–495)
before/around `try_parse_train`.

Pre-existing (NOT introduced this session, confirmed via `git stash` on clean
HEAD): `(2÷⍨≢) 8` → port `1`, oracle `1/2` (bound-constant commute with a
trailing `≢` in paren form; a separate paren-adverb bug).

Gates green throughout (lib 96/0 · curated 1/0); no debug edits in tree.

---

# PROBLEM 3 — `⇐` RHS multi-function chain: ⍛ binding the accumulated train (RESOLVED 49f1286)

## Symptom
`stat:median 1 2 3 4` → port `(1)`, oracle `5/2`. The body
`2 ÷⍨ +/ (¯1r2 0+2÷⍨≢)⍛⊇ ∧` parses (no error), and the pieces ALL
evaluate correctly in isolation, but the full body gave wrong result.

## Root cause (AST diff confirmed)
In the 2-train chaining loop (`parse_function_expr_continuation_impl`),
`⍛`/`∘` was binding the WHOLE accumulated train
(`Train[Train[2,÷⍨], +/]`) as its left operand. Kotlin's `parseOperator`
(parser.kt:1273) runs per-function — `⍛` must bind only the just-parsed
atom (the paren group), not the accumulated chain.

## Fix (commit 49f1286)
In the 2-train loop, parse each atom, fold its trailing adverb, THEN
check for `⍛`/`∘`/`«»` on the atom itself (not `cur`). Bind the atom
(not `cur`) as compose's left, get compose's right operand, then chain
the wrapped atom into the 2-train. For `(…)⍛⊇ ∧`: `paren⍛⊇` becomes
`RevComp(paren, ⊇)`, then `∧` chains as a 2-train atop →
`Train[RevComp(paren, ⊇), ∧]`.

## Verified
```
stat:median 1 2 3 4          → 5/2   oracle 5/2   ✓
stat:median 1 3 5 7 9 11    → 6     oracle 6     ✓
m ⇐ 2 ÷⍨ +/ (¯1r2 0+2÷⍨≢)⍛⊇ ∧ ⋄ m 1 2 3 4 → 5/2   ✓
```
Gates: lib 96/0 · curated 1/0.
