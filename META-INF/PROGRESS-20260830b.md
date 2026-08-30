# PROGRESS-20260830b — reshape.kt parity (P3 file 1, CLOSED)

**Branch:** `feature/wheres-extra` @ `833bf45` (sync: `main == strings == 833bf45`, pushed).
**Invariant restored:** `git branch -f main && git branch -f strings && git push origin main strings feature/wheres-extra`.

## Gates (green after this work)
- `cargo test -p kap-core --lib` → **96 passed / 0 failed**
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed / 0 failed**
- Broad sweep (truncated `/tmp/conform_dump.txt`): **OK 1596 / MISMATCH 205 / UNSUPPORTED 746**
  (baseline before: OK 1593 / MISMATCH 208 — net +3 OK, −3 MISMATCH)

---

## DONE — reshape.kt (`kap-core/src/evaluator.rs::reshape`)

Kotlin anchor: `array/.../builtins/reshape.kt` `findSizeCalculationMethod` (:375),
left-spec guard (:244-246), empty left shape `⍬` (:249-257), `⍴` left must be
scalar/1-D (:376).

### (1) scalar `¯1` left spec → MATCH, not empty  [FIX]
Old code routed `APLValue::Number(Long(-1))` straight to `DimSpec::Fixed(-1)`
→ `dims=[max(0,-1)]=[0]` → empty result `()`. Kotlin: scalar `¯1` is the
computed-dimension sentinel → `findSizeCalculationMethod` returns `MATCH`,
computed count = `b.contentSize()` (4), so `¯1 ⍴ 1 2 3 4 ⇒ (1 2 3 4)`.
Fix: scalar-Long arm now runs through `method_of` (which maps a `Number(Long(-1))`
instance to `SizeMethod::Match`), so `¯1` becomes `DimSpec::Computed(Match)`.

### (2) rank>1 left shape → error verbatim  [FIX]
Added guard after `dims_val` is forced: `if dims_val.dimensions().len() > 1`
return `AplError::runtime("Left side of rho must be scalar or a one-dimensional array")`
(Kotlin reshape.kt:244-246). Resolves `(2 2 ⍴ 3 4 5 6) ⍴ 1 2 3 4` which the
port had silently flattened into a 16-element reshape.

### (3) `⍬` left shape → first element, depth-0 scalar  [FIX]
New `APLValue::Null` arm: empty left shape returns `valueAt(0)` of the right
arg. If the right arg is empty/Null, default `0`. An atomic element (number/char/
null) is returned as a scalar (depth 0) — mirrors `enclose` (Kotlin
reshape.kt:249-257, and `enclose`/`⊂` atom-pass-through). This matches oracle
`⍬⍴5 → 5` (depth 0), `⊂5 → 5`, `⍬⍴⊂1 2 3 → (1 2 3)`.

---

## Oracle-verified probe table (port == oracle)

| Expr | Result | Expr | Result |
|------|--------|------|--------|
| `¯1 ⍴ 1 2 3 4` | `(1 2 3 4)` | `(2 2 ⍴ 3 4 5 6) ⍴ 1 2 3 4` | **error** (Left side of rho must be scalar or a one-dimensional array) |
| `⍬⍴ 1 2 3` | `1` | `⍬⍴⍬` | `0` |
| `⍬⍴5` | `5` (`≡`=0) | `⍴⍬⍴5` | `⍬` |
| `⍬⍴ ⊂1 2 3` | `(1 2 3)` | `2 ¯1⍴⍳4` | `(0 1 2 3)` |
| `3 :fill ⍴ ⍳12` | `(0 1 … 11 0 1 2)` | `2 :recycle ⍴ ⍳3` | `(0 1 2 0)` |
| `5 :truncate ⍴ ⍳12` | `(0 1 2 3 4 5 6 7 8 9)` | `3 :match ⍴ ⍳12` | **error** (Invalid size of right argument: 12. Should be divisible by 3.) |

## Curated rows added (`conformance.rs`, after the reshape-spec ladder block)
- `("¯1 ⍴ 1 2 3 4", "(1 2 3 4)")`
- `("⍬⍴ 1 2 3", "1")`
- `("⍬⍴ ⍬", "0")`
- (Note: `(2 2 ⍴ 3 4 5 6) ⍴ 1 2 3 4` is an error case — the curated harness has
  no `kind:"fails"` slot for `(&str,&str)` rows, so it is documented in the
  comment block at L471-474 per the existing convention, not asserted.)

## ReshapeTest broad-sweep status
- Before: 4 MISMATCH (`¯1⍴…`, `(2 2⍴…)⍴…`, `⍬⍴1`, `⍬⍴1 2 3 4 5 6`, `⍬⍴⊂1 2 3`, `⍬⍴⍬`).
- After: 1 MISMATCH — `⍬ ⍴ ⊂1 2 3`. Values are correct (depth 2, rank 0); the
  divergence is the rank-0 *display* glyph (`⟨0⟩` vs port `()`), classified
  DISPLAY not value (P8). `⍬⍴5` value/shape/depth all match.

## Remaining reshape.kt deltas (out of scope)
- `:fill`/`:recycle` with a non-default fill element, `⍴[axis]` axis form, and
  the `¯1` keyword-vs-literal interaction on multi-dim left specs — port already
  matches oracle on the sampled cases; not probed exhaustively.
- Rank-0 ⟨⟩ display normalization is a P8 renderer decision, not a reshape bug.
