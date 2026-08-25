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

## Status (updated 2026-08-25, P7c-2)
The declare(...) half is FIXED and committed (`6441a30`). Remaining blocker narrowed:

`(f ⍞g)` — a known user function followed by a `⍞` DynamicRef inside parens —
fails `expected a function in train` / `unexpected token in primary`, which
blocks util.kap's filter body `(toBoolean ⍞fn)¨ arg`. Two attempted fixes did
not resolve it:
1. `is_definite_function`: added `DynamicRef => true` and
   `self.is_known_fn(...)` for symbols (now an instance method; both call sites
   updated). Gates stayed green but the repro still fails.
2. Verified parse_function_atom's ApplyToken arm handles `⍞g` standalone
   (`⍞g 5` → works), so the failure is in how try_parse_train's member loop or
   the primary OpenParen arm sequences these two members.

Next diagnostic: add a temporary eprintln! in try_parse_train's member loop
showing each parsed member variant, run `(f ⍞g)`, then remove it. That will
pin whether member 2 parses at all or the classifier rejects [Symbol, DynamicRef].

Also confirmed by oracle probing (documented for P7c):
- Oracle's OWN util.kap filter FAILS at call time ("No arguments specified for
  function") — its body does `arg ← arg` on a macro-bound :value, which Kotlin
  rejects. So full filter parity is impossible; matching definition-time parsing
  is the correct goal.
- Oracle call syntax is `filter (arg) {fn}` — `:value` REQUIRES parentheses
  (Kotlin ValueSyntaxRule.isValid = token is OpenParen). Bare `1 2 3 filter {…}`
  is invalid in the oracle too.
- Port defsyntax expansion of `(:value v :function f)` works correctly in
  isolation: `defsyntax m2 (:value v :function f) { (⍞f¨ v) } ⋄ m2 (1 2 3 4) {2|⍵}`
  → `(1 0 1 0)`. Only bodies containing `(known-fn ⍞ref)` fail.

Gates GREEN throughout (lib 96/0, curated 1/0).
