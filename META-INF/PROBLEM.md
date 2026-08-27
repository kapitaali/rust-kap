# PROBLEM — OPEN-A (output3.kap:118 axis-applied fork tine)

> Status: **RESOLVED (2026-08-27).** output3.kap:118 now produces the exact
> 52-element oracle vector, both standalone and inside its `⍺`/`⍵` dfn context.
> P1–P12 oracle matrix all match. Gates green: `cargo test -p kap-core --lib`
> → 96 passed / 0 failed; `curated_kap_parity` → 1 passed / 0 failed.
> Commit `fixed (see PROGRESS-20260827.md)`.

## Oracle target (output3.kap:118)

```kap
arrayMaxWidth←80 24
((⌈arrayMaxWidth[0]÷⍺)↑[¯1+≢⍴⍵])«,»((-⌈arrayMaxWidth[1]÷⍺)↑[¯1+≢⍴⍵]) ⍵
```

- **oracle** (`kap-jvm-text`): `⊢ ⟨1 2 3 4 5 6 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 2 3 4 5 6⟩` — a 52-element centred vector.
- **port (fixed tree, fresh rebuild)**: identical 52-element vector ✓.

## Root cause (verified, two compounding defects in `parse_paren_accum`'s value-stranding arm)

A fork tine `((⌈arrayMaxWidth[0]÷⍺)↑[k])` is parsed as a parenthesised group via
`parse_primary` → `parse_paren_vfn_chain` → `parse_paren_accum`. Inside that
accumulator, the value arm stranded a variable by emitting a *bare* token and never
consuming its trailing index selector. Two faults:

1. **Dangling `[…]` index selector.** The arm emitted the variable and stopped —
   `arrayMaxWidth[0]` left the `[0]` dangling. The fallback `parse_function_atom`
   then rejected `OpenBracket` → `parse error at 1:68: unexpected token in primary`.
2. **Wrong `Instr` shape for the variable.** The arm wrapped the variable as
   `Instr::Literal(LiteralValue::Symbol{..})` — a *literal* symbol. `eval_instr`
   rejects a lone literal symbol with `lone symbol literal`. A variable reference
   must be `Instr::Symbol{name, namespace}` (the lookup form); only non-symbol
   literals (number/string/char/`SymbolValue`) become `Instr::Literal`.

The minimal trigger proved against the oracle (only the indexed variable fails;
constants in the identical shape pass):

| case | result |
|---|---|
| `a←1 2 3 ⋄ ((⌈a[0]÷2)↑[0])«,»((-⌈a[1]÷2)↑[0]) ⍳6` | oracle `⟨0 5⟩`; port now matches ✓ |
| `((⌈80÷2)↑[0])«,»((-⌈24÷2)↑[0]) ⍳6` | oracle `(0 1 … ¯5 0…0)`; port = oracle ✓ (constant tines, no index) |
| `((⌈80÷2)↑[¯1+≢⍴1 2 3 4 5 6])«,»((-⌈24÷2)↑[¯1+≢⍴1 2 3 4 5 6]) 1 2 3 4 5 6` | oracle 52-elem; port = oracle ✓ |

## The fix (parser.rs, `parse_paren_accum` value arm)

Mirror `parse_primary`: for a `Symbol`/`SymbolValue` token emit the *lookup* form
(`Instr::Symbol{name, namespace}`), and for every value literal call
`parse_index_suffix(v)` to fold any trailing `[…]` into an `Index` instruction.

```rust
let t = self.peek().map(|t| t.token.clone()).unwrap();
self.advance();
if let Token::Literal(lv) = t {
    // A Symbol/SymbolValue token is a VARIABLE reference: emit Instr::Symbol
    // (lookup form), NOT Instr::Literal (which eval rejects as "lone symbol
    // literal"). Non-symbol literals stay Instr::Literal. Then fold the trailing
    // `[…]` index selector exactly as parse_primary does.
    let v = match lv {
        LiteralValue::Symbol { name, namespace } => Instr::Symbol { name, namespace },
        LiteralValue::SymbolValue { name, namespace } =>
            Instr::Literal(LiteralValue::SymbolValue { name, namespace }),
        other => Instr::Literal(other),
    };
    match self.parse_index_suffix(v) {
        Ok(v) => left_args.push(v),
        Err(_) => return ParenHolder::Malformed,
    }
}
continue;
```

### False lead (explicitly rejected — do NOT re-add)

Extending `parse_function_atom`'s `Symbol` arm to consume `[…]` via
`parse_index_suffix` **broke axis binding**: `↑[0]` became `Index{↑,[0]}` instead of
`AxisApplied{↑,0}`, yielding `only symbol/lambda functions supported yet` at eval.
Axis binding must stay in the `parse_paren_accum`/`parse_apply` → `bind_operators_kotlin`
path, NOT in `parse_function_atom`. Reverted. (`Index` on a *variable* `a[0]` is
correct and supported — only a primitive like `↑[0]` must remain `AxisApplied`.)

## Note on the remaining output3.kap load warnings

`output3.kap` still logs `parse error at 146:33` (and sibling errors in
`math.kap`/`http.kap`/`map.kap`/`fhelp.kap`) when loaded via the stdlib `use()`
machinery. Line **146** is a *different* expression and line **118** (this issue) is
fixed. Those are pre-existing stdlib gaps unrelated to this change (which touched only
`parse_paren_accum`'s variable arm and `evaluator.rs`'s left-bind short-circuit guard).
Track separately.

## Gates (post-fix, both GREEN)

```
cargo test -p kap-core --lib          # 96 passed / 0 failed
cargo test -p kap-core --test conformance curated_kap_parity  # 1 passed / 0 failed
```

## P1–P12 oracle matrix (all match after fix)

| # | Input | Oracle | Port |
|---|-------|--------|------|
| P1 | `((-2)↑) ⍳6` | ⟨4 5⟩ | (4 5) ✓ |
| P2 | `⌽«,»((-2)↑) ⍳6` | ⟨5 4 3 2 1 0 4 5⟩ | (5 4 3 2 1 0 4 5) ✓ |
| P3 | `(2↑)«,»((-2)↑) ⍳6` | ⟨0 1 4 5⟩ | (0 1 4 5) ✓ |
| P4 | `(2↑[0]) 1 2 3` | ⟨1 2⟩ | (1 2) ✓ |
| P5 | `((-2)↑[0]) ⍳6` | ⟨4 5⟩ | (4 5) ✓ |
| P6 | `((1+1)↑[0]) ⍳6` | ⟨0 1⟩ | (0 1) ✓ |
| P7 | `2↑[¯1+1] 1 2 3` | ⟨1 2⟩ | (1 2) ✓ |
| P8 | `((⌈5÷2)↑[0]) ⍳6` | ⟨0 1 2⟩ | (0 1 2) ✓ |
| P9 | `x←2 ⋄ ((-x)↑[0]) ⍳6` | ⟨4 5⟩ | (4 5) ✓ |
| P10 | `⌽«,»(2↑[0]) ⍳6` | ⟨5 4 3 2 1 0 0 1⟩ | (5 4 3 2 1 0 0 1) ✓ |
| P11 | `(2↑[0])«,»((-2)↑[0]) ⍳6` | ⟨0 1 4 5⟩ | (0 1 4 5) ✓ |
| P12 | `arrayMaxWidth[0]/[1]` fork (52-elem) | ⟨1 2 3 4 5 6 0…0 1 2 3 4 5 6⟩ | identical 52-elem ✓ |
