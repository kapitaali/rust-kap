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

# PROBLEM 2 — bound-constant commute `2÷⍨` (stat.kap median) — OPEN (2026-08-25)

`stat:median 1 2 3 4`: oracle `5/2`, port `(2 1 2/3 1/2)`. Root cause chain:

1. Median body `(¯1r2 0+2÷⍨≢)` needs `2÷⍨≢` to parse as ONE derived atom
   (`(2÷⍨)∘≢`). The port splits the leading literal into separate members.
2. Deeper issue: **bound-constant commute is inverted in the port.**
   `z ⇐ 2÷⍨ ⋄ z 8` → oracle `4` (=8÷2), port `1/4` (=2÷8).
   Plain monadic commute matches (`÷⍨ 8` → 1 both).

Three attempted fixes (evaluator ⍨ bind-case ×2 shapes, parser
bound-constant exemption in bind_operators_kotlin) each fixed one probe while
breaking another — the shape flows through try_parse_train, impl's adverb arm,
AND bind_operators_kotlin depending on context, so a single-site fix can't
cover it. Reverted all three; tree back to committed state `9ba7eab`.

Correct design (next session): introduce a first-class `BoundConstant`
concept at parse time — when a Literal/Array is followed by an adverb in
function position, wrap as `Derived{func: Value(x), op}` AND make the ⍨
monadic evaluator arm check `matches!(func, Instr::Literal(_) | Instr::Array{..})`
(syntactic, not eval-based) to select bind semantics `y f x`. All three parse
sites must agree. Verify against oracle probes:
- `(2÷⍨) 8` → 4   · `z⇐2÷⍨ ⋄ z 8` → 4   · `÷⍨ 8` → 1   · `(÷⍨≢) y` → y÷≢y

Gates green throughout (lib 96/0 · curated 1/0); no partial edits left in tree.
