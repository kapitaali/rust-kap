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

## Status
No fix attempted yet. Gates were GREEN before this investigation
(lib 96/0, curated_kap_parity 1/0); no debug eprintln! left in the tree.
