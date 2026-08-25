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

The ⍨ INVERSION itself is FIXED and committed (4/4 oracle matrix passes —
see PROGRESS-20260825.md P7d section). What remains open is the NEXT layer:

## Remaining: value-tine-in-fork semantics (median still diverges)

`stat:median 1 2 3 4`: oracle `5/2`; port `(1/2 1 3/2 2)`.
Body: `median ⇐ 2 ÷⍨ +/ (¯1r2 0+2÷⍨≢)⍛⊇ ∧`

New evidence from oracle probes (kap-jvm-text, standard-lib):
- `(¯1r2 0+2÷⍨≢) 1 2 3 4` → `⟨3/2 2⟩`   (port: `1`)
- `(¯1r2 0+2÷⍨≢) 1 2 3 4 5` → `⟨2 5/2⟩`
- `(¯1r2 0) 4`            → `⟨⟨-1/2 0⟩ 4⟩`  ← KEY: a bare VALUE in fn
  position applied to y yields ⟨constant, y⟩ — Kotlin LeftAssignedFunction
  (`functions.kt:628`) treats it as an ⍺-bind, NOT a constant-returning fn.
- `(1r2 0+2) 9`           → `⟨⟨5/2 2⟩ 9⟩`

So the fork's left tine `(¯1r2 0)` is an ⍺-BIND of the strand; the fork then
evaluates with that bound value participating differently than a plain
constant tine. The port's train evaluator treats value tines as constants.
Open question: exact evaluation order for `(A B C) y` when A is a bare value
strand — likely A becomes LeftBind(A, B∘C) or similar per Kotlin
makeLeftBindFunctionParseResult. Next step: read Kotlin functions.kt
LeftAssignedFunction + how Chain3 handles a leading non-function member,
then mirror in the port's Train eval.

Gates green throughout (lib 96/0 · curated 1/0); no debug edits in tree.
