# Parser Migration Reference (P1)

## Kotlin `parseExpr` loop → Rust mapping

Kotlin `parser.kt:939-1030` is a single `while(true)` loop accumulating `leftArgs`.
The Rust port's `parse_value_kotlin` (parser.rs:295-695) is the equivalent.

### Token dispatch table

| Kotlin token | Rust `parse_value_kotlin` arm | Notes |
|---|---|---|
| `END_EXPR_TOKEN_LIST` (EOF, `⋄`, `]`, `)`, `}`) | line 306-312 (close stack + EOF/sep break) | |
| `Name` → customSyntax | line 560-564 | defsyntax macro triggers |
| `Name` → `⇐` short-form fn def | line 599-628 | `name ⇐ rhs` |
| `Name` → known function | line 658-678 | `finish_fn_call` |
| `Name` → known operator (no left) | line 650-655 | `Operator without left function` |
| `Name` → dual-nature `/⌿\⍀` | line 649 (exception) | value-left → function form |
| `Name` → variable/strand | line 668-673 | accumulate to leftArgs |
| `OpenParen` | line 467-506 | group → fn or value |
| `OpenFnDef` (`{…}` lambda) | line 399-415 | function-shaped |
| `ApplyToken` (`⍞`) | line 423-456 | dynamic ref, function-shaped |
| `LambdaToken` (`λ`) | line 457-466 | function-shaped |
| `FnDefSym` (`∇`) | line 383-388 | short-form or long-form def |
| `LeftArrow` (`←`) | line 507-554 | assignment |
| literals (number, string, char, null) | line 346-361 | accumulate to leftArgs |
| `QuotePrefix` / `SymbolValue` | line 395-398 | symbol literals are values |
| `if`/`while`/`when` | line 369-378 | statement-complete, return |
| `and`/`or` | line 318-343 | infix short-circuit |
| `defsyntax`/`defsyntaxsub` | line 589-593 | bails to legacy `parse_expr` |
| `declare(…)` | line 569-586 | special form |

### Kotlin `processFn` → Rust `finish_fn_call`

| Kotlin | Rust | Notes |
|---|---|---|
| `FunctionCallOpenParen` | line 1081-1083 | `parse_function_call_list` |
| `LeftArrow` (modified assignment) | not yet in `finish_fn_call` | handled at call site |
| empty right + empty left → fn value | line 1105-1107 | |
| empty right + non-empty left → LeftBind | line 1110-1121 | 2-train `[strand, fn]` |
| value right + empty left → monadic | line 1183-1188 | `Apply{fn, left: None, right}` |
| value right + non-empty left → dyadic | line 1192-1207 | `Apply{fn, left, right}` |
| fn right + empty left → Chain2 (atop) | line 1175-1181 | `Train[fn, right]` |
| fn right + non-empty left → Chain2∘LeftBind | line 1141-1161 | `Train[Train[strand, fn], right]` |

### Remaining gaps (from non-regression probes)

1. **`foo ⇐ ⌸`** — B2 check missing in Kotlin short-form fn path. Should error
   `Operator without left function: ⌸`.
2. **`typeof ⌸`** — parsed as two separate statements (`typeof` then `⌸`) instead
   of a single expression `typeof(⌸)`. The `⌸` operator triggers the "Operator
   without left function" error at statement start.
3. **`-⍛+`** — port says `- requires a number` (monadic minus over `⍛+`), oracle
   says `No arguments specified for function` (the whole `-⍛+` is a 3-train
   with no right arg). Display/train-parse divergence.

### Key invariants

- **Strand collection**: consecutive value operands strand (`1 2 3` → `Array`).
- **Valence at eval, not parse**: parser never peeks at glyph meaning.
- **Operator references are parse errors**: known-op with no function operand → error.
- **Single-pass**: no backtracking; fallback to legacy parser only via explicit
  `__KOTLIN_FALLBACK__` sentinel or reset-to-`start`.
