# PROGRESS 2026-08-26 (f) — `⍢` parse gap closed + hex-literal normalisation

## [2026-08-26] `⍢`/`⍣` derived-function parse gap + `0x`/`0b` literal type

Continuation of the roadmap. Target was the long-standing
`error: undefined symbol: ⍢` blocking `output3.kap` (95/130 at session start,
per PROGRESS-20260826e and the skill's OPEN BUG note).

### Oracle ground truth captured FIRST

Oracle: `~/Apps/array/kap-jvm-text/bin/kap-jvm-text --lib-path=$HOME/Apps/array/kap-jvm-text/standard-lib`

| expr | port (before) | oracle |
|---|---|---|
| `⌽⍢⌽ ⍳5` | `(4 3 2 1 0)` ✓ | `⟨4 3 2 1 0⟩` |
| `x ← ⌽⍢⌽ ⍳5` | `error: undefined symbol: ⍢` | `⟨4 3 2 1 0⟩` |
| `{⍵} ⌽⍢⌽ ⍳5` | `error: undefined symbol: ⍢` | `⟨4 3 2 1 0⟩` |
| `{⍵+1} ⌽⍢⌽ ⍳5` | `error: undefined symbol: ⍢` | `⟨5 4 3 2 1⟩` |
| `⍳0x20` | `error: ⍳ needs an integer count` | `⟨0 1 … 31⟩` |
| `0x20` | `32` ✓ | `32` |
| `x ← ⌽⍣2 ⍳5` | `(0 1 2 3 4)` | `Error at: 1:5: No arguments specified for function` |

Note the last row: the oracle ERRORS on a bare `⍣` in that position. Port
leniency, left alone deliberately (not in scope, and "fixing" it would need
its own oracle-grounded arity work).

### Root causes (three distinct, all confirmed against Kotlin source)

1. **`←` RHS used the legacy parser.** `parser.rs` `Token::LeftArrow` arm parsed
   the assignment RHS with legacy `parse_expr()`. Kotlin `processAssignment`
   (`parser.kt:529`) parses it with **`parseValue()`** — the SAME accumulator
   loop, which is the only place `⍢`/`⍣` are handled (`parser.rs:731`). So
   `x ← ⌽⍢⌽ ⍳5` never reached the `⍢` arm and the glyph fell through to a bare
   symbol lookup.
2. **No `⍢`/`⍣` fold on a chained function atom.** The train/chain loops fold
   adverbs (`is_adverb`) and the `∘`/`⍛`/`«»` TOKENS onto each just-parsed atom,
   mirroring Kotlin `parseOperator` (`parser.kt:1273`) running per-function. But
   `⍢`/`⍣` are lexed as plain `Symbol`s (there is **no `UnderToken`** — the skill
   note is accurate) and are NOT in `is_adverb`, so they were invisible to both
   fold mechanisms and were left dangling in `{⍵} ⌽⍢⌽`.
3. **Hex/binary literals were always `BigInt`.** `lex_helpers.rs:158` returned
   `KapNumber::BigInt` unconditionally, while the DECIMAL path
   (`parse_kap_number`, `lex_helpers.rs:231-235`) normalises to `Long` when it
   fits. `iota` (`evaluator.rs:4308`) matches on `KapNumber::Long`, so `⍳0x20`
   errored while `⍳32` worked.

### Fixes

- **`kap-core/src/parser.rs` `Token::LeftArrow` arm (~:505)** — parse the RHS
  with `parse_value_kotlin()`, keeping legacy as the `__KOTLIN_FALLBACK__`
  fallback (reset `self.pos = start` then `parse_expr`).
- **`kap-core/src/parser.rs` new `fold_value_ops_on_atom` (~:2884)** — folds
  `⍢`/`⍣` onto a function atom into `Instr::ValueOp`, looping so `f⍢g⍣2` binds
  left-to-right like the Kotlin operator loop. Wired at THREE sites:
  - the `right` atom fold in the chain continuation (~:3871),
  - the `member` atom fold in the inner chain loop (~:3973),
  - `parse_apply`'s legacy function-atom block (~:1955) — reached because
    `finish_fn_call` routes a monadic right argument through legacy
    `parse_apply` (`parser.rs:1027`), which is how `{⍵} ⌽⍢⌽ ⍳5` arrives.
- **`kap-core/src/parser.rs` leading-function-atom apply arm (~:2048)** — added
  `Instr::ValueOp { .. }`. Without it the folded derived fn was not recognised
  as a function value, so `⌽⍢⌽ ⍳5` STRANDED the fn beside its argument
  (`(⍬ (0 1 2 3 4))`) instead of applying it. This was the subtle second half of
  fix 2 — the fold alone produced wrong VALUES, not an error.
- **`kap-core/src/lex_helpers.rs:158`** — normalise hex/binary to `Long` when it
  fits, matching the decimal path.
- **`kap-core/src/lex_helpers.rs:342` `lex_hex_and_binary` test** — updated; it
  had asserted the buggy `BigInt` behaviour. Now asserts `Long`, with the
  oracle rationale in a comment.

### Verification (port vs oracle, after)

| expr | port | oracle | ok |
|---|---|---|---|
| `⌽⍢⌽ ⍳5` | `(4 3 2 1 0)` | `⟨4 3 2 1 0⟩` | ✓ |
| `x ← ⌽⍢⌽ ⍳5` | `(4 3 2 1 0)` | `⟨4 3 2 1 0⟩` | ✓ |
| `{⍵} ⌽⍢⌽ ⍳5` | `(4 3 2 1 0)` | `⟨4 3 2 1 0⟩` | ✓ |
| `v ← ⍳5 ⋄ {⍵} ⌽⍢⌽ v` | `(4 3 2 1 0)` | `⟨4 3 2 1 0⟩` | ✓ |
| `{⍵+1} ⌽⍢⌽ ⍳5` | `(5 4 3 2 1)` | `⟨5 4 3 2 1⟩` | ✓ |
| `f ⇐ {⍵} ⌽⍢⌽ ⋄ f ⍳5` | `(4 3 2 1 0)` | `⟨4 3 2 1 0⟩` | ✓ |
| `{⍵} ⌽∘⌽ ⍳5` | `error: ⌽: The rank of the rotation specifier…` | same text | ✓ |
| `⍳0x20` | `(0 1 … 31)` | `⟨0 1 … 31⟩` | ✓ |
| `⍳0b101` | `(0 1 2 3 4)` | `⟨0 1 2 3 4⟩` | ✓ |
| `0xFF` | `255` | `255` | ✓ |
| `3 (+ « × » -) 4` | `¯7` | `-7` (oracle plain mode) | ✓ |
| `x ← 1 2 3 ⋄ +/x` | `6` | `6` | ✓ |

(`⟨⟩` vs `()` is the known P8 renderer glyph gap, not a value divergence.)

### Gates (re-run, all GREEN)

- `cargo test -p kap-core --lib` → **96 passed; 0 failed**
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed; 0 failed**
- `cargo build -p kap-cli` clean (warnings pre-existing).

### stdlib load state

| file | before | after |
|---|---|---|
| `output3.kap` | 95/130 `undefined symbol: ⍢` | 90/122 `˝: inverse not supported for this function` |
| `util.kap` | CLEAN | CLEAN |
| `stat.kap` | CLEAN | CLEAN |
| `structure.kap` / `io.kap` / `standard-lib.kap` | CLEAN | CLEAN |
| `map.kap` | 18/24 parse error 13:6 | unchanged (own mini-phase, ROADMAP §10) |
| `math.kap` | 1/9 `math:pi` | unchanged — **oracle-consistent**, deliberately not "fixed" |

`⍢` no longer appears ANYWHERE in the output3 load output (verified: 0
occurrences). The remaining output3 blocker is a genuinely different, deeper
item (below).

### Remaining / next
- **`⍢` structural-under for non-self-inverse wrappers** (evaluator, NOT parser).
  `↓⍢(10↓) ⍳20` → port `error: ˝: inverse not supported for this function`,
  oracle `⟨0…9 11…19⟩`. `apply_under_op` (`evaluator.rs:7848`, dispatched
  ~:1372) only handles self-inverse wrappers (`⌽`, `-`). This is the SAME
  pre-existing gap recorded in earlier PROGRESS notes; it is now the sole
  output3 blocker and is the natural next roadmap item.
- Port leniency on bare `⍣` in assignment position (oracle errors) — noted, not
  scoped.
- **NEW divergence found while re-verifying the `≡`/namespace work (pre-existing,
  NOT caused by this session): `typeof` on a string.**

  | expr | port | oracle |
  |---|---|---|
  | `typeof "hi"` | `kap:string` | `kap:array` |
  | `'kap:string ≡ typeof "hi"` | `1` | `0` |
  | `'kap:array ≡ typeof "hi"` | `0` | `1` |

  In Real Kap a string IS a char array, so `typeof "hi"` is `kap:array`; the port
  has a distinct `Str` representation and reports `kap:string`. This is the
  known nested/string-representation gap surfacing through `typeof`, not a
  namespace-threading defect (`'kap:array ≡ typeof 1 2 3` → `1` in BOTH, and
  `'kap:array ≡ 'kap:array` → `1` in both, so the namespace fix is sound).
  Fixing it means changing `class_name()` for `Str` and auditing every
  `kap:string` consumer — deliberately deferred, recorded here so it is not
  mistaken for a regression of the `≡` work.

### Process notes (burned time, worth recording)

- A `git stash push kap-core/src/parser.rs` baseline attempt **failed to
  compile** (5 errors) because `ast.rs`/`token.rs`/`evaluator.rs` still carried
  the namespace-field changes from the earlier `≡` work — the "baseline" counts
  it printed came from a STALE binary and were meaningless. Restored with
  `git stash pop`. Lesson: a single-file stash is NOT a valid baseline when the
  uncommitted work spans coupled files; stash all of them or none.
- Temp probe files cleaned up: `_b`, `_repro*.kap`, `_o3_*.kap`, `_t.kap`,
  `_z.kap` all removed from `kap-stdlib/std/`.
- No debug instrumentation left in the tree from this session (the
  `KAP_DEBUG_USE` hooks at `evaluator.rs:552/567/587` are pre-existing and
  env-gated).
