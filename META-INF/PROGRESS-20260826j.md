# PROGRESS 2026-08-26 (j) — paren-group value as a left-bind in ANY position

## [2026-08-26] `((-2)↑)` now parses in fork-tine position; output3 90/123 → 53/88

Continuation of the roadmap. Target was the long-standing `output3.kap:118:62`.

### The tell

`((-2)↑)` worked as a fork's LEFT tine but not as its RIGHT tine — and the error
column pointed at the *inner* group's closing paren:

| expr | port (before) | oracle |
|---|---|---|
| `((-2)↑)«,»(2↑) ⍳6` | `(4 5 0 1)` ✓ | `⟨4 5 0 1⟩` |
| `⌽«,»((-2)↑) ⍳6` | `parse error at 1:9: unexpected token in primary` | `⟨5 4 3 2 1 0 4 5⟩` |
| `(2↑)«,»((-2)↑) ⍳6` | `parse error at 1:12` | `⟨0 1 4 5⟩` |
| `⌽«,»((1+1)↑) ⍳6` | `(5 4 3 2 1 0 0 1)` ✓ | same |

So a nested group starting with a **function glyph** (`-`) failed where one starting
with a **number** (`1+1`) succeeded. Position-dependence + that asymmetry were the
whole diagnosis.

### Root cause (TRACED, per the skill's own rule — not guessed)

A temporary `KAP_TRACE_FA` probe in `parse_function_atom`'s `OpenParen` arm printed
the branch taken and the resulting instr:

```
⌽«,»((-2)↑)   → FA-OpenParen enter … next=Some(OpenParen)
                FA-OpenParen enter … next=Some(Symbol{-})
                FA-OpenParen -> vfn_chain OK Array { elements: [Symbol{-}, Literal(2)] }
                error: No arguments specified for function
⌽«,»((1+1)↑)  → FA-OpenParen -> vfn_chain OK Train { funcs: [Apply{+,1,1}, Symbol{↑}] }
```

`Array[Symbol{-}, Literal(2)]` is the smoking gun: the inner `(-2)` was **stranded**
instead of applied. Two defects, in sequence:

1. **`parse_paren_vfn_chain`'s `first_is_value` guard (parser.rs:2957)** listed
   Number/Char/Str/SymbolValue/OpenBracket/QuotePrefix but **not `OpenParen`**, so a
   paren-leading group was routed to `try_parse_train` (function-members only), which
   cannot represent `(-2)` — a *value expression* whose first token is a function glyph.
2. **`parse_paren_accum`'s catch-all arm** then handled the nested `(` via
   `parse_function_atom`, which re-entered the same fn-only path and produced the
   strand. That strand ends in an unbound primitive, so the evaluator's `Instr::Array`
   guard (evaluator.rs:720) fired `No arguments specified for function`.

Kotlin has no such split: the inner `parseExpr` (parser.kt:939) recurses on `(` and
returns *either* a value or a function holder.

### Fix — two coordinated edits in `kap-core/src/parser.rs`

- `first_is_value` now includes `Some(Token::OpenParen) => true`, so paren-leading
  groups use the Kotlin value accumulator.
- New explicit `Some(Token::OpenParen)` arm in `parse_paren_accum`, before the
  value-atom catch-all. It parses the group as a **self-contained balanced unit** via
  `parse_function_atom` and classifies the result with `is_function_expr`:
  a function continues exactly like the symbol-function arm (Empty ⇒ left-bind,
  Fn ⇒ Chain2/atop, Value ⇒ Apply, all with the leading-values variants);
  anything else is re-parsed with `parse_primary` and stranded as a left arg.

**Failed first attempt (recorded, not hidden):** my first version of that arm recursed
directly into `parse_paren_accum` after consuming the `(`. That is WRONG — the
accumulator returns at the *first* `)` it meets, so parsing `-`'s argument consumed the
paren that closes the inner group and the parse ran on past it, converting the runtime
error into `parse error at 1:9` again. Balanced-unit parsing plus `is_function_expr`
classification is what actually works.

### Verification — all oracle-exact

`⌽«,»((-2)↑) ⍳6` → `(5 4 3 2 1 0 4 5)`; `(2↑)«,»((-2)↑) ⍳6` → `(0 1 4 5)`;
`((-2)↑)«,»(2↑) ⍳6` → `(4 5 0 1)`; `⌽«,»((-2)+) 5` → `(5 3)`.

21-shape regression sweep, all unchanged and oracle-exact: `((-2)↑) ⍳6`, `(-2)`,
`((-2))`, `f ⇐ (⌽ ⊢)`, `((+ ×) -) 1 2`, `3 ((+ ×) -) 4`, `((⌽ ⊢) ≢) 1 2 3`,
`((2+) ×) 3`, `((1+1)↑) ⍳6`, `((⌈3÷2)↑) ⍳6`, `(2↑)«,»⌽ ⍳6`, `3 (+ « × » -) 4`,
`(⌽⍢⌽) 1 2 3 4`, `¯2 3 4 (×∘-) 1000`, `10 (-⍛+) 100`, `(1 2+≢) 5`, `f ⇐ ×- ⋄ f 3`,
`(1+2)(3+4)`, `1 2 3 +∙× 1 2 3`.

### Regression rows added — `kap-core/tests/conformance.rs` (~:719)

`⌽«,»((-2)↑) ⍳6`, `(2↑)«,»((-2)↑) ⍳6`, `((-2)↑)«,»(2↑) ⍳6`, `⌽«,»((-2)+) 5`
(all four tine positions).

### Gates — GREEN

- `cargo test -p kap-core --lib` → **96 passed; 0 failed**
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed; 0 failed**
- `cargo build -p kap-cli` clean; no `KAP_TRACE_FA` instrumentation remains.

### stdlib load state — output3 IMPROVED

| file | before | after |
|---|---|---|
| `output3.kap` | 90/123 failed | **53/88 failed** |
| util / stat / structure / io / standard-lib | CLEAN | CLEAN |
| `map.kap` | 18/24 @13:6 | unchanged |
| `math.kap` | 1/9 (`math:pi`, oracle-consistent) | unchanged |

`output3.kap:118` is now fully visible:

```kap
((⌈arrayMaxWidth[0]÷⍺)↑[¯1+≢⍴⍵])«,»((-⌈arrayMaxWidth[1]÷⍺)↑[¯1+≢⍴⍵]) ⍵
```

Still `118:62` — the paren-value left-bind is fixed, but the **axis form inside a
tine** is not. Two further gaps, freshly isolated:

| expr | port | oracle |
|---|---|---|
| `⌽«,»(2↑[0]) ⍳6` | `parse error at 1:11: unexpected token in primary` | `⟨5 4 3 2 1 0 0 1⟩` |
| `(2↑[0])«,»((-2)↑[0]) ⍳6` | `error: unsupported axis operator: ↑` | `⟨0 1 4 5⟩` |

Note `(2↑[0]) 1 2 3`, `((-2)↑[0]) ⍳6`, `((1+1)↑[0]) ⍳6`, `2↑[¯1+1] 1 2 3` are ALL
already correct — so it is specifically **axis-applied fn as a fork tine** (a parse
gap) and **`↑` with an axis reaching the axis-operator dispatch** (an evaluator gap,
`unsupported axis operator: ↑`). Those are the next two items for line 118.

### Remaining / next

- **`output3.kap:118:62`** — needs the two axis gaps above.
- `(n↑)` — variable as a left-bind value (`unknown function: n`), parser-side.
- Carried over: dyadic structural under; pick-based under (lookup.kt:232);
  `typeof "hi"` → `kap:string` vs oracle `kap:array`; `map.kap` 13:6.

### Process notes

- **The trace paid off immediately again.** One probe printing the chosen branch AND
  the built instr localised a two-layer parser defect in a single build, after the
  previous session burned three speculative edits on the same area. `Array[Symbol{-},
  Literal(2)]` in the dump was the entire diagnosis.
- **"Finished in 0.04s" struck twice more.** `touch <one file>` was not enough; only
  `touch kap-core/src/*.rs` forced the 9.4s relink. Confirmed the binary was fresh via
  `ls -l --time-style=+%H:%M:%S target/debug/kap` before trusting any probe — worth
  making the default habit.
- **Position-dependent failure ⇒ suspect the DISPATCH GUARD, not the parse of the
  construct.** `((-2)↑)` was fine standalone and as a left tine; only the right-tine
  route differed, and the bug was a missing token in a `matches!` list two levels up.
