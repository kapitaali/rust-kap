# `⍢` Structural Under — Remaining Failures (2026-08-31)

## Done this session

1. **Lambda value semantics** (parser.rs:521): `λ↑` now yields a function VALUE that strands
   (`⟨function 5⟩`) instead of auto-applied. Fixed by NOT pre-advancing before
   `parse_function_atom` — the `λ` arm at 4860 consumes its own operand.

2. **`⍢⍞a` with `↑` wrapper** (evaluator.rs:~9670): `resolve_under_wrapper` now resolves
   `DynamicRef{a}` → `Some(↑)`. Fixed by putting the primitive arm BEFORE the user-fn
   alias arm (which was catching `Symbol(↑)`, trying `env.lookup("↑")`, failing → `None`).

3. **`˝⍞a` for `-`/`+`**: works via the same resolution path.

Gates green: `cargo test -p kap-core --lib` 96/0, `curated_kap_parity` 1/0.

## Remaining failures

### A. Lambda value stranding (parser)

| Expression | Oracle | Port | Root cause |
|---|---|---|---|
| `(λ↑) 5` | `⟨function 5⟩` | `5` | `is_function_expr` (parser.rs:4948) matches `Lambda` → paren group treated as function-shaped → `finish_fn_call` applies it |
| `a←λ↑ ⋄ a 5` | `⟨function 5⟩` | `5` | `a←λ↑` stores `UserFn`; `function_names()` returns `a`; parser treats `a` as function symbol → applies |

**Fix direction**: `Lambda` is a VALUE, not a function atom. Two sub-problems:
- `is_function_expr` should NOT match `Lambda` (it's a value, like a number).
- `a←λ↑` should NOT register `a` in `known_functions` (only `⇐` should). The parser's
  symbol-classification arm (parser.rs:749-751) uses `known_functions` to decide
  function-vs-value. A `←`-bound lambda must stay a value.

### B. Dyadic under with inverse-family wrapper (evaluator)

| Expression | Oracle | Port | Root cause |
|---|---|---|---|
| `12 {⍵+8×⍵≥0}⍢- 11 12 13 14` | `⟨3 4 13 14⟩` | `error: under not supported` | `left.is_some()` bail (evaluator.rs:9309) fires BEFORE the inverse tail (9620) for dyadic `-` wrapper |

**Fix direction**: The `left.is_some()` bail at 9309 is too aggressive. It should only
bail for wrappers that genuinely don't support a left arg (overlay family). Inverse-family
wrappers (`-`, `+`, `÷`, `×`) DO support dyadic-left: `a f ⍢ - b` = `a - (f(a - b))` per
Kotlin `evalInverse2ArgB`. Move the bail to AFTER the inverse tail, or gate it on
`!is_inverse_family(wrapper)`.

### C. Commute + DynamicRef in inverse adverb (evaluator)

| Expression | Oracle | Port | Root cause |
|---|---|---|---|
| `a←λ- ⋄ 8 ⍞a⍨˝ 7` | `15` | `-1` | `adverb_inverse` resolves `⍞a⍨` → `-⍨` but the commute unwrap produces wrong sign |

**Fix direction**: `8 -⍨˝ 7` = `8 -⁻¹(-7)` = `8 - 7` = ... wait, oracle says 15 = `8 - (-7)`.
So `-⍨˝` on `8` and `7` means: commute makes it `7 - 8`... no. `8 -⍨˝ 7` = `(-⍨)⁻¹(8 (-⍨) 7)`
= `(-⍨)⁻¹(8 - 7)` = `(-⍨)⁻¹(1)` = `1`... oracle says 15. Need to trace Kotlin's
`evalInverse2ArgBWithCommute` more carefully. The port gives `-1` = `-(8-7)`, suggesting
the commute is being dropped during resolution.

## Verification

```bash
cd /home/theb/Apps/array/rust-kap
B=./target/debug/kap; OR=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
o() { printf '%s\n' "$1" | $OR 2>&1 | grep -v -E 'WARNING|Restricted' | sed -n '2p' | sed -E 's/^⊢ //'; }
p() { printf '%s\n' "$1" | $B 2>&1 | grep -aE '>>> |error:|parse error' | head -1 | sed -E 's/^>>> //'; }
cmp() { e="$1"; ov="$(o "$e")"; pv="$(p "$e")"; if [ "$ov" = "$pv" ]; then s="OK"; else s="DIFF"; fi; printf '  [%s] %s\n    o=%s\n    p=%s\n' "$s" "$e" "$ov" "$pv"; }
cmp '(λ↑) 5'
cmp 'a←λ↑ ⋄ a 5'
cmp '12 {⍵+8×⍵≥0}⍢- 11 12 13 14'
cmp 'a←λ- ⋄ 8 ⍞a⍨˝ 7'
```

## Key files

- `kap-core/src/parser.rs`: `is_function_expr` @4933; Lambda arm @521; symbol classification @749; `known_functions` regrowth @705 (⇐ only)
- `kap-core/src/evaluator.rs`: `apply_under_op` @9236; `left.is_some()` bail @9309; inverse tail @9620; `resolve_under_wrapper` @9667; `adverb_inverse` @5769
