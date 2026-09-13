# Known non-conformances — Rust Kap vs Real Kap (Kotlin oracle)

> **STATUS 2026-09-13 — read this first.** The per-feature tables below are a
> HISTORICAL snapshot frozen at commit `d6372ef` (2545 extracted cases, 1774 ok /
> 24 mismatch / 747 unsupported = 69.7%). Many rows in them are closed since.
> The live register for current work is `META-INF/PROGRESS-*.md` — the arc that
> reached full corpus coverage is `META-INF/PROGRESS-20260913.md`.
>
> Current measurement (branch `feature/wheres-extra`, 2026-09-13):
>
> ```
> extracted Kotlin cases : 2987
> ok                     : 2987   (100.0%)
> mismatch               :    0
> unsupported            :    0
> ```
>
> `cargo test --release -p kap-core` → exit 0: 16 test binaries, 198 tests
> passed, 0 failed. Release is mandatory for the sweep (debug's 10 s per-case
> `recv_timeout` turns a 12.8 s render into a phantom UNSUPPORTED).
>
> Two open limitations the corpus metric does NOT capture — not sweep
> "non-conformances", but real gaps, do not lose them:
>
> 1. **`jvm:jvmMethodCallException` carries a message String, not Kotlin's live
>    throwable.** Kotlin's tag data is `JvmInstanceValue(originException)`
>    (jvm-module.kt:67); the port emits the exception text. Handing a live Jvm
>    value to `xml.kap`'s `handleXmlParseException` raises "cannot use a JVM
>    value as an array element" and then SIGSEGVs inside the JVM. The throwable's
>    registry id is already carried on `JThrown`, and `jvm_thrown` in
>    `evaluator.rs` is where the handover goes. No extracted row exercises this,
>    so the sweep is green regardless.
> 2. **Java-side assertions are run-verified only.** Rows whose Kotlin assertion
>    is Java-side (e.g. `XmlParserTest` asserting `assertIs<Document>` +
>    `nodeName`) carry `expected: None` in `kotlin_tests.jsonl`: the case proves
>    the expression RUNS, not that the value is right. Closing that gap is an
>    extractor change (`tools/extract_kotlin_tests.py`), not an engine change.
>
> ---
>
> ## HISTORICAL SNAPSHOT — table set frozen at `d6372ef` (2545 cases, 69.7%)

Every "port | oracle" pair below was produced by probing **both** the Rust
binary (`./target/debug/kap`) and the `kap-jvm-text` oracle in the same
terminal session — these are observed divergences, not assumptions.

Legend for severity:
- **CRITICAL** — computes a wrong result for a core builtin; high mismatch count.
- **DISPLAY** — value is correct, only the rendered glyph differs (port house
  style). Not a defect, but it inflates the broad-sweep mismatch count.
- **MSG** — correctly rejects, but the error text differs from the oracle.
- **DEFERRED** — feature not built yet; currently errors (counted Unsupported).

---

## Closed (verified fixed, kept for history)

| feature | closed | commit |
|---------|--------|--------|
| `⍎` (execute / parse-number) | 2026-08-24 | pending |
| char/string arithmetic error text | 2026-08-24 | pending |
| `⌷` (squad / index selection) | 2026-08-20 | — |
| bracket indexing `x[sel]` | 2026-08-21 | — |
| `≡` / `≢` (match / depth) | 2026-08-20 | — |
| `⊃` (first / pick) | 2026-08-21 | — |
| `≬` / `toList` | 2026-08-21 | — |
| `⍕` monadic format | 2026-08-21 | — |
| `⍥` leading operator error | 2026-08-28 | `daa39f5` |
| `;`-list destructuring (partial) | 2026-08-28 | `daa39f5` |
| `use()` file loading | 2026-08-22 | — |
| `⎕A ⎕a ⎕d` constants + `declare(:const)` | 2026-08-25 | — |
| `⊥` / `⊤` (decode / encode) rank-1 validation | 2026-08-24 | — |
| `…` (range) complex-number | 2026-08-24 | — |
| `⍸` (where / interval) dyadic | 2026-08-24 | — |
| `~` (without) axis validation | 2026-08-24 | — |
| `bool2` short-circuit / null truthy | 2026-08-24 | — |
| `modulo` signed-divisor fix | 2026-08-24 | — |
| `[0]` at statement start → index deref error | 2026-09-01 | `d6372ef` |

---

## MISMATCH — 24 cases (wrong value, feature partially built)

### Bracket-axis (3)

`f[axis]` syntax parsed but axis not yet applied for these verbs.

| expr | port | oracle |
|------|------|--------|
| `⊃[1] 1` | error / wrong | `1` |
| `⊂[0] 1` | error / wrong | `,1` |
| `+⍨[0] 10 20` | error / wrong | `30` |

### Complex / rational (4)

Complex-number arithmetic and rational bounds not fully wired.

| expr | port | oracle |
|------|------|--------|
| `1r2 0.9 ⍸ 0 0.5 1r2 0.8 0.9 0.91 0.92` | error | interval vector |
| `1r2 7r10 ⍸ 0.0 0.5 0.92 1.5` | error | interval vector |
| `1j2 … 10` | error | complex range |
| `10 … 40j50` | error | complex range |

### Assignment (3)

| expr | port | oracle |
|------|------|--------|
| `a←4 ◊ { declare(:local a) a←3 ◊ ⍵+a } 2 ◊ a+5` | wrong value | `14` |
| `foo bar←10` | silently strands | error (multi-name) |
| `(a b c d e f) ← 3 2 ⍴ 10 20 30 40 50 50` | silently strands | error (destructuring rank) |

### Return / `→` (2)

| expr | port | oracle |
|------|------|--------|
| `{S ⇐ → ⋄ {(S⍣(81=×⍨⍵)) ⍵}¨⍳10} 0` | error / wrong | return value |
| `{ S ⇐ → ◊ 100 + { S ⍵+20 ◊ ⍵+1 } 10 } 0` | error / wrong | return value |

### Under / `⍢` (2)

| expr | port | oracle |
|------|------|--------|
| `{,100}⍢(6↑) ⍳3` | error / wrong | `100 1 2 3 0 0` |
| `((0 1↓)⍢(2↑)) 5 4 ⍴ ⍳20` | error / wrong | matrix |

### Compose / train with left arg (2)

| expr | port | oracle |
|------|------|--------|
| `2 (3 (+⊢)) 5` | error / wrong | `16` |
| `2 (3 (+«⊢»⊣)) 5` | error / wrong | `16` |

### Multi-line `∇` (3)

| expr | port | oracle |
|------|------|--------|
| `∇ foo x {` (×2 test cases) | parse error | valid function |
| `∇ foo (X) {` | parse error | valid function |

### Lambda bare (1)

| expr | port | oracle |
|------|------|--------|
| `λfoo` | error / wrong | function ref |

### Namespace (1)

| expr | port | oracle |
|------|------|--------|
| `namespace("foo")` | error / wrong | enters namespace |

### Syntax defs (1)

| expr | port | oracle |
|------|------|--------|
| `defsyntax foo (:nfunction a) { ⍞a 2 }` | error / wrong | defines syntax |

### Transpose (1)

| expr | port | oracle |
|------|------|--------|
| `0 0⍉˝2 3⍴ 1 2 3 4 5 6` | error / wrong | transposed matrix |

### Big int (1)

| expr | port | oracle |
|------|------|--------|
| `math:isPrime 36893488147419103873+⍳10` | error / wrong | prime vector |

---

## DEFERRED — not yet implemented (currently Unsupported, 747 cases)

Features the engine does not build yet. Grouped by cluster.

### Bracket-axis (75 total)

`f[axis]` syntax parsed but axis ignored or errored for these verbs.

| verb | count | example |
|------|-------|---------|
| `,` (concatenate) | 5 | `(4 5 ⍴ ⍳20) ,[0] 1000+⍳5` |
| `⊃` (disclose) | 10 | `⊃[1] 2 3 2 ⍴ (0 1)(2 3)(4 5)(6 7)(8 9)` |
| `⊂` (enclose) | 15 | `⊂[0] 2 3 2 ⍴ ⍳1000` |
| `∊` (enlist) | 7 | `∊[0] (((1 2)(3 4))((5 6)(7 8)))(9 10) 11 12` |
| `\` (expand) | 2 | `1 0 1 1 \\[0] 3 3 ⍴ 100+⍳9` |
| `/` (reduce) | 7 | `+[0]/ (1 2)(2 2 ⍴ 3 4 5 6)` |
| `⌿` (reduce-first) | — | same pattern |
| `labels` | 21 | `"foo" "bar" labels[1] 2 2 ⍴ 1 2 3 4` |
| `sort` | 6 | `∧[0] 2 2 3 ⍴ 7 5 4 9 10 8 3 6 2 11 0 1` |
| scalar ops | 12 | `100 200 ÷[0]⍰ 1000×2 2 ⍴ ⍳4` |

### Big-integer arithmetic (56)

Operations on values exceeding `i64` range. Dominant cluster.

| example | count |
|---------|-------|
| `int:asBigint` arithmetic (`+ - × ÷ \|`) | ~30 |
| `int:ensureGeneric / ensureLong / ensureDouble` | ~10 |
| `int:asBigint¨` applied to large vectors | ~10 |
| `math:gcd / math:lcm` bigint | 4 |
| `math:divisors` bigint | 2 |

### Complex numbers (48)

Complex arithmetic, `math:re`/`math:im`, complex-aware comparisons.

| example | count |
|---------|-------|
| `toBoolean¨ 7j4 0j0 1j1` etc. | 7 |
| `2J6 + int:asBigint 2` | 2 |
| `1.2 4.7 + 2 0x7000000000000000` (mixed bigint/complex) | 13 |
| `3⋆˝6561` (inverse on complex) | 12 |
| `8 ÷∘-˝ 8000` (compose with complex inverse) | 3 |

### Member dereference `.` (44)

`map:with` + `.field` access.

| example | count |
|---------|-------|
| `foo ← map:with 'test 1 'abc 2` | ~12 |
| `(10 (map:with ...)).(1).default:foo.(2)` | ~10 |
| nested map access | ~10 |
| `MemberTest` cases | 24

### Labels (40)

`labels` verb (distinct from bracket-axis `labels[n]`).

| example | count |
|---------|-------|
| `labels "foo" "bar" labels 1 2` | ~10 |
| `labels "a" "b" "c"` | ~10 |
| matrix labels | ~10 |
| `⍉ "a" "b" labels[0] ...` | ~10 |

### Adverb / compose (30)

`⍢ ⍛ ∘ ⍤ ⍥` with left args or complex trains.

| example | count |
|---------|-------|
| `{100,↓⍵}⍢, 2 3 ⍴ 10+⍳6` | 1 |
| `4 (1+)⍢(⊣⍨) 10` | 1 |
| `8 ÷∘-˝ 8000` | 1 |
| `10 (×∘(20+))⍨˝ 60` | 1 |
| `10 ((20+)⍛×)˝ 60` | 1 |
| `a ⇐ { (⍺+) ⍵ } ⋄ 3 a 1 2 3` | 1 |
| `a ⇐ { (⍺+)¨ ⍵ } ⋄ 3 a 1 2 3` | 1 |
| `a ⇐ {,[1]/⍵} ⋄ ⊃ a (2 4 3 ⍴ ⍳100) (2 3 ⍴ 100+⍳100)` | 1 |
| `a ⇐ {,[⍺]/⍵} ⋄ ⊃ 1 a (2 4 3 ⍴ ⍳100) (2 3 ⍴ 100+⍳100)` | 1 |
| `f0 ← λ{⍺+⍵}` (×3) | 3 |
| `a ⇐ {,[1]/⍵} ⋄ ⊃ a ...` | 1 |

### Encode / decode (22)

`⊥` / `⊤` with non-integer, bigint, or edge-case args.

| example | count |
|---------|-------|
| `10 ⊥ 1 2 3` | 1 |
| `10 ⊥ 2` | 1 |
| `9 ⊥ ⍬` | 1 |
| `10 ⊥ 33` | 1 |
| `10 ⊥ 1 33 3` | 1 |
| `2 4 5 ⊥ 1 1 1` | 1 |
| `10 ⊥ 2 3.1 3.1` | 1 |
| `2 2 ⊥ 2 5 ⍴ 1 1 0 0 1 0 1 0 1 0` | 1 |
| `(3⍴2) ⊤ 3` | 1 |
| `(2⍴2) ⊤ 7` | 1 |
| `2 3 6 ⊤ 15` | 1 |
| `(,3) ⊤ 7` | 1 |
| `2 3 6 ⊤ 0` | 1 |
| `(40⍴10) ⊤ 12` | 1 |
| `(10⍴10) ⊤ ¯10` | 1 |
| `(100⍴10) ⊤ 10000000000000000000000000000000000000000` | 1 |
| `(40⍴10) ⊤ ¯123456789012345678901234567890` | 1 |
| `(2⍴2) ⊤ 2 3 ⍴ 10+⍳6` | 1 |
| `2 ⊤ 100` | 1 |
| `2 ⊤ 256` | 1 |
| `10 ⊤ 1234` | 1 |
| `10 ⊤ 1234 100 23456` | 1 |
| `8 ⊤ 123456789012345678901234567890123456789012345` | 1 |

### Numbers / float edge (18)

Mixed bigint/float arithmetic, hex literals, overflow.

| example | count |
|---------|-------|
| `1.2 4.7 + 2 0x7000000000000000 + 900 0x7000000000000000` | 1 |
| `+/ 10 ⍴ 1000000000000000000` | 1 |
| `+/ int:ensureGeneric 10 ⍴ 1000000000000000000` | 1 |
| `(int:asBigint 5) + 0.0` | 1 |
| `0.0 + int:asBigint 5` | 1 |
| `-/ 100000 ⍴ 100000000000000` | 1 |
| `a←×/24⍴2 ⋄ a (a×a×a) ÷ 1000000000000000` | 1 |
| `\|10000000000000000000000000000000 ¯10000000000000000000000000000000 (int:asBigint 0)` | 1 |
| `(int:asBigint¨ 2 2 ¯2 ¯2 2) \| int:asBigint¨ 123 ¯123 123 ¯123 ¯2` | 1 |
| `(int:asBigint¨ 10000 10000 ¯10000 ¯10000) \| int:asBigint¨ 20005 ¯20005 20005 ¯20005` | 1 |
| `4 \| (int:asBigint 2) (int:asBigint 5) (int:asBigint 6) (int:asBigint ¯2) 123456789012345678901234567891` | 1 |
| `-¯9223372036854775808` | 1 |
| `-12 ¯9223372036854775808` | 1 |
| `-12.2 ¯9223372036854775808` | 1 |
| `0-¯9223372036854775808` | 1 |
| `0-12 ¯9223372036854775808` | 1 |
| `0-12.2 ¯9223372036854775808` | 1 |
| `⋆1 2 (int:asBigint 3) ¯10` | 1 |
| `0 2 2J2 3J¯3 ¯3J10.1 ¯3J¯4 ∘∙! 0 3 8.1J1 ¯3.4J4 10J¯3 ¯2J¯8` | 1 |
| `√(int:asBigint 15) (int:asBigint ¯15)` | 1 |
| `3√(int:asBigint 15) (int:asBigint ¯15)` | 1 |
| `√˝ 0 1 2 3 10 12345 ¯1 ¯2 ¯1000 ¯123456789123456` | 1 |
| `√˝ 0.0 1.0 2.0 3.3 10.9 9876.543 ¯1.0 ¯2.3 ¯12.92838 ¯293819384.234` | 1 |
| `√˝ 0.0 1.0 2.1 0 1 ¯1 ¯2 12345 2.4` | 1 |
| `1 2 3 4 5 ¯1 2 3 4 5 ¯3 ¯3 3 3 √˝ 10 11 12 13 14 15 16 17 18 19 40 ¯40 ¯50 50` | 1 |
| `1.0 1.1 2.2 3.2 5.5 ¯3.2 ¯4.3 √˝ 2.0 2.1 3.1 3.7 ¯1.3 ¯4.7 ¯1.141` | 1 |
| `2 1.2 √˝ 3 3.2` | 1 |
| `2 3 4 ¯3 ¯7 ¯7 6 √⍨˝ 5 6 7 2 ¯99 ¯3 ¯3` | 1 |
| `math:pi` | 1 |
| `2.1j3.0 + 3÷5` | 1 |

### Null / edge (51)

`⍬`, `null`, `@char` edge cases across many verbs.

| example | count |
|---------|-------|
| `toBoolean¨ (0 0 0) (,0) (⊂,0) ⍬ (⍬ ⍬) (1 1 ⍴ 0)` | 1 |
| `toBoolean¨ @\0 @a @\s` | 1 |
| `9 ⊥ ⍬` | 1 |
| `⊂[0]⍰ 2 2 ⍴ ⍳4` | 1 |
| `⊂[0]⍰ null` | 1 |
| `100 200 ÷[0]⍰ 1000×2 2 ⍴ ⍳4` | 1 |
| `100 200 ÷[0]⍰ null` | 1 |
| `100 200 ÷[1]⍰ 1000×2 2 ⍴ ⍳4` | 1 |
| `⍮⍰ 1 2` | 1 |
| `1 null ↑ 4 3 ⍴ 1 2 3 4 5 6 7 8 9` | 1 |
| `null null ↑ 4 3 ⍴ 1 2 3 4 5 6 7 8 9 10 11 12` | 1 |
| `2 null 2 ↑ 4 3 5 ⍴ 1+⍳30` | 1 |
| `¯2 null ↑ 4 3 ⍴ 1 2 3 4 5 6 7 8 9 10 11 12` | 1 |
| `null 0 0 ↑ 4 3 2 ⍴ 1 2 3 4 5 6 7 8 9 10 11 12` | 1 |
| `⦻ 2 + 1 ⦻` | 1 |
| `⦻ 3 × 4 ⦻` | 1 |
| `2 3 - ⦻` | 1 |
| `1 0 1 ⫽ 1 2 3` | 1 |
| `2 1 1 /[4] 7 6 5 4 3 ⍴ ⍳1000` | 1 |
| `⍋ (1;2) (2;1)` | 1 |
| `⍋ "foo" "bar" 'somename` | 1 |
| `⍋ 1 2 3 'somename` | 1 |
| `(⊂ 5 10) ⌷ 100+⍳100` | 1 |
| `(3 0) (3 2) ⌷ 4 5⍴100+⍳100` | 1 |
| `(⊂ 3 0) ⌷ 4 6 ⍴ 100+⍳100` | 1 |
| `1 0 1 1 \ 2` | 1 |
| `0 2 2 \ 3 1 ⍴ 100+⍳9` | 1 |
| `2 1 3 1 ,/⌸ "foo" "bar" "xyz" "abcdef"` | 1 |
| `1 0 1 0 % "abc" "FOO"` | 1 |
| `0 1 0 % ("a1" "b1" "c1") ("a2" "b2" "c2")` | 1 |
| `(2 2 ⍴ 0 0 1 0) % (2 2 ⍴ ⍳4) (2 2 ⍴ 100+⍳4)` | 1 |
| `0 10 2 2 % (100×⍳11) + 11 ⍴ (⊂0 1 2 3)` | 1 |
| `0 1 1 % 9 (5 6 7)` | 1 |
| `0 1 1 % (5 6 7) 9` | 1 |
| `(2 2 ⍴ 0 0 1 1) % 9 (2 2 ⍴ 3 4 5 6)` | 1 |
| `0 0 % (,⊂ 1 2)` | 1 |
| `0 1 % (2 3) (⊂1 2 3)` | 1 |
| `0 1 0 % (⊂1 2 3 4 5) (⊂10 11 12 13 14)` | 1 |
| `0 1 % ("foo" "bar") (⊂"testing")` | 1 |
| `1 1 0 0 ∧ 0 1 1 0` | 1 |
| `1.0 1.0 0.0 0.0 ∧ 1.0 0.0 1.0 0.0` | 1 |
| `(int:asBigint 1) (int:asBigint 0) ∧ (int:asBigint 1) (int:asBigint 1)` | 1 |
| `1.0 1.0 0.0 0.0 ∨ 1.0 0.0 1.0 0.0` | 1 |
| `(int:asBigint 0)∨(int:asBigint 0)` | 1 |
| `(int:asBigint 1) (int:asBigint 0) ∨ (int:asBigint 0) (int:asBigint 0)` | 1 |
| `1 1 0 0 ∨ 0 1 1 0` | 1 |
| `~0 1` | 1 |
| `~ int:ensureGeneric 1 0` | 1 |
| `~ 2 3 ⍴ 1 0 0 1 1 1` | 1 |
| `~ int:ensureGeneric 2 3 ⍴ 1 0 0 1 1 1` | 1 |
| `~ (int:asBigint 1) (int:asBigint 0)` | 1 |
| `~ (int:asBigint 1) 0` | 1 |
| `(int:asBigint 1) (int:asBigint 0) (int:asBigint 1) (int:asBigint 0) ⍲ (int:asBigint 0) (int:asBigint 0) (int:asBigint 1) (int:asBigint 1)` | 1 |
| `(int:asBigint 1) (int:asBigint 0) (int:asBigint 1) (int:asBigint 0) ⍱ (int:asBigint 0) (int:asBigint 0) (int:asBigint 1) (int:asBigint 1)` | 1 |
| `2 3 4 + 1.0 1.0 0∧1.0 0.0 1` | 1 |
| `2 3 4 + 1.0 1.0 0∨1.0 0.0 1` | 1 |

### Lambda eval (11)

`⍞` (eval lambda) not implemented.

| example | count |
|---------|-------|
| `a ← λ { 1 + ⍵ } ◊ (⍞a 1) + ⍞a 5` | 1 |
| `foo ← λ{⍺+⍵+1} ◊ 10 ⍞foo 3000` | 1 |
| `foo ← λ{⍵+1} ◊ 20 + ⍞foo 10 20 30 40` | 1 |
| `foo ← λ{⍺+⍵+1} ◊ 20 + 6 ⍞foo 10 20 30 40` | 1 |
| `foo ← λ { 1 + ⍵ } ◊ bar ← λ { ⍵ } ◊ ⍞(⍞bar foo) 7` | 1 |
| `x←λ { 1 + ⍵ } ◊ ⍞x¨ 1 2 3 4` | 1 |
| `∇ foo (x) { λ{ y←⍵ ◊ λ{ ⍵+x+y } } }` | 1 |
| `∇ (x) foo (y) { x + y }` | 1 |
| `{ a←⍵ ⋄ ⍞a/ 10 11 12 13 }¨ λ× λ+` | 1 |
| `{ ⍞⍵/ 10 11 12 13 }¨ λ× λ+` | 1 |
| `{ a←⍵ ⋄ {⍞a/ ⍵} 10 11 12 13 }¨ λ× λ+` | 1 |

### Map (11)

`map:with` / `map:get` / `map:size`.

| example | count |
|---------|-------|
| `a ← map:with 2 2 ⍴ "foo" "abc" "bar" "bcd"` | 2 |
| `a ← map:with (:a ; :b) 10 (:a ; :b ; :c) 20 ⋄ a[(:a ; ; :c)]` | 1 |
| `a ← map:with 7j6 1 8j6 2 ⋄ a[7j6]` | 1 |
| `a ← map:with (4÷5) 1 (6÷7) 2 ⋄ a[4÷5]` | 1 |
| `a ← map:with "foo" 3 "bar" 4 ⋄ a["foo" "bar"]` | 1 |
| `a ← map:with "foo" 3 "bar" 4 ⋄ a["foo" "bar" "abc"]` | 1 |
| `a ← map:with "foo" 3 "bar" 4 "abc" 5 "def" 6 "ghi" 7 ⋄ a[2 2 ⍴ "foo" "bar" "abc" "def"]` | 1 |
| `a ← map:with 10 100 20 200 30 300 ⋄ a[30 20]` | 1 |
| `a ← map:with 10 100 20 200 30 300 40 400 50 500 60 600 70 700 ⋄ a[2 2 ⍴ 30 20 10 70]` | 1 |
| `a ← map:with 10 100 20 200 30 300 40 400 50 500 60 600 70 700 ⋄ a[2 2 ⍴ 30 20 11 71]` | 1 |
| `foo` | 1 |
| `map:size map:with ⍬` | 1 |

### List / `≬` (11)

`fromList`, `toList˝`, `≬˝`, `⌷˝`.

| example | count |
|---------|-------|
| `1;2;3` | 1 |
| `1+2;2+3;3 ◊ 5+1+1;6+1+1;7;8` | 1 |
| `1+2;3+4` | 1 |
| `fromList (1;2;3)` | 1 |
| `(1+)⍢fromList (10 ; 20 ; 30)` | 1 |
| `toList˝ (1;2;3)` | 1 |
| `≬˝ (1;2;3)` | 1 |
| `fromList˝ 1 2 3` | 1 |
| `⌷˝ 1 2 3` | 1 |
| `⌷˝ (1;2;3)` | 1 |
| `⌷˝ 1` | 1 |
| `fromList˝ (1;2;3)` | 1 |

### Rank / `⍤` (10)

`rank adjust` verb.

| example | count |
|---------|-------|
| `< 1` | 1 |
| `< ⍳9` | 1 |
| `< 3 3 ⍴ ⍳9` | 1 |
| `⍳ 4 5` | 1 |
| `⍳ 2 3 2` | 1 |
| `⍳,9` | 1 |
| `⍳⍬` | 1 |
| `⍳2 2` | 1 |
| `⍤` applied to user fn | 1 |
| `⍤` with left arg | 1 |

### Custom function multi-line / paren (10)

`∇ foo (A;B;C;D) { ... }`, `(E;F) foo (A;B;C;D) { ... }`, etc.

| example | count |
|---------|-------|
| `∇ foo (A;B;C;D) { A+B+C+D+1 } ◊ foo (10;20;30;40)` | 1 |
| `∇ (E;F) foo (A;B;C;D) { A+B+C+D+E+F+1 } ◊ (1000;2000) foo (10;20;30;40)` | 1 |
| `∇ foo (A;B) { A+B+1 } ◊ foo (10 ; foo (1;2))` | 1 |
| `∇ (A;B) foo (C;D) { A+B+C+D+1 } ◊ (8;11) foo (10 ; (100;200) foo (1;2))` | 1 |
| `∇ foo (x0;x1) { x0+x1+1 } ◊ foo (1;2)` | 1 |
| `∇ (x0;x1) foo (y0;y1) { x0+x1+y0+y1+3 } ⋄ (10;11) foo (1;2)` | 1 |
| `∇ (foo) (x0;x1) { x0+x1+1 } ◊ foo (1;2)` | 1 |
| `∇ (x0;x1) (foo) (y0;y1) { x0+x1+y0+y1+3 } ◊ (10;11) foo (1;2)` | 1 |
| `∇ a (x foo y) b {` | 1 |
| `∇ foo x {` | 1 |

### Syntax `defsyntax` (10)

| example | count |
|---------|-------|
| `defsyntax foo (:constant x) { 10 }` | 1 |
| `defsyntax foo (:value a :optional (:value b)) {` | 1 |
| `defsyntax xif (:value cond :function thenStatement :optional (:constant xelse :function elseStatement)) {` | 1 |
| `defsyntax xif (:value cond :function thenStatement :optional (:constant xelse :function elseStatement)) {` | 1 |
| `defsyntax xif (:value cond :function thenStatement :optional (:constant xelse :function elseStatement)) {` | 1 |
| `defsyntax xif (:value cond :function thenStatement :constant xelse :function elseStatement) {` | 1 |
| `defsyntax foo (:exprfunction a) { 1+⍞a 0 }` | 1 |
| `defsyntaxsub bar (:constant ab :value x) {` | 1 |
| `declare(:singleCharExported "a")` | 1 |
| `defsyntax foo (:nfunction a) { ⍞a 2 }` | 1 |

### Structural under (9)

`⍢` with structural left arg.

| example | count |
|---------|-------|
| `a⇐↑ ⋄ (100+)⍢(¯1 a ¯1↓) 3 3 ⍴ ⍳9` | 1 |
| `(100+)⍢(×∘(10+)) 100` | 1 |
| `0 1 (100+)⍢(⊇∘(3↓)) 10 20 30 40 50 60 70 80` | 1 |
| `⍢` with drop/take | 6 |

### Enlist (8)

`∊` with nested arrays, `@char`.

| example | count |
|---------|-------|
| `∊ (1 2 (3 4)) 5 6` | 1 |
| `∊ 2 3 ⍴ 1 (10 20) 3 ((41 42) (43 44)) 5 6` | 1 |
| `∊ (1 2) (3 4 (5 @a @b @c))` | 1 |
| `∊⍬` | 1 |
| `∊⊂1 2` | 1 |
| `∊ (⊂1 2) (⊂10 20)` | 1 |
| `∊[2] ((⊂1 2) (⊂10 20)) 3` | 1 |
| `∊[1]⊂1 2` | 1 |
| `∊2 3 ⍴ 1 2 3 4 5 @a` | 1 |
| `∊2 3 ⍴ 1 2 3 4 5 6` | 1 |
| `∊0.0+2 3 ⍴ 1 2 3 4 5 6` | 1 |
| `∊2 3 ⍴ (3 4 ⍴ (10+⍳11),@a) 2 3 4 5 @a` | 1 |
| `∊2 3 ⍴ (3 4 ⍴ 10+⍳12) 2 3 4 5 6` | 1 |
| `∊0.0+2 3 ⍴ (3 4 ⍴ 10+⍳12) 2 3 4 5 6` | 1 |

### Function call paren (8)

`∇ foo (a;b;c) { a+b+c }`, `foo⟦⟧`, etc.

| example | count |
|---------|-------|
| `∇ foo (a;b;c) { a+b+c }` | 1 |
| `∇ foo (a;b;c) { a+b+c }` | 1 |
| `∇ foo (a;b;c) { "test" }` | 1 |
| `∇ foo (a;b;c) { a+b+c }` | 1 |
| `∇ foo (a;b;c) { a+b+c }` | 1 |
| `200 + +/⟦1 2 3 4⟧ + 100` | 1 |
| `∇ foo (a) { ⍴ fromList a } ⋄ foo⟦⟧` | 1 |
| `∇ foo (a) { 10 (⍴ fromList a) } ⋄ foo⟦⟧` | 1 |

### IO (8)

`io:readdir`, `io:read`, `io:readFile`.

| example | count |
|---------|-------|
| `x ← io:readdir "test-data/readdir-test/" ◊ x[⍋x;]` | 1 |
| `x ← :size io:readdir "test-data/readdir-test/" ◊ x[⍋x;]` | 1 |
| `io:read "test-data/multi.txt"` | 1 |
| `foo` | 1 |
| `foo` | 1 |
| `foo` | 1 |
| `foo` | 1 |
| `io:readFile "test-data/multi.txt"` | 1 |

### Inverse / `˝` (8)

`f˝` inverse adverb.

| example | count |
|---------|-------|
| `3⋆˝6561` | 1 |
| `8⋆⍨˝6561` | 1 |
| `⍟˝ 2 3 ⍴ 2+⍳6` | 1 |
| `2 *˝512` | 1 |
| `*˝5` | 1 |
| `2 ⍟˝10` | 1 |
| `2 ⍟˝¯3` | 1 |
| `a⇐⍟ ⋄ a˝ 2 3 ⍴ 2+⍳6` | 1 |

### Logic (8)

`∧ ∨ ~ ⍲ ⍱` with bigint.

| example | count |
|---------|-------|
| `1 1 0 0 ∧ 0 1 1 0` | 1 |
| `1.0 1.0 0.0 0.0 ∧ 1.0 0.0 1.0 0.0` | 1 |
| `(int:asBigint 1) (int:asBigint 0) ∧ (int:asBigint 1) (int:asBigint 1)` | 1 |
| `1.0 1.0 0.0 0.0 ∨ 1.0 0.0 1.0 0.0` | 1 |
| `(int:asBigint 0)∨(int:asBigint 0)` | 1 |
| `(int:asBigint 1) (int:asBigint 0) ∨ (int:asBigint 0) (int:asBigint 0)` | 1 |
| `1 1 0 0 ∨ 0 1 1 0` | 1 |
| `~0 1` | 1 |
| `~ int:ensureGeneric 1 0` | 1 |
| `~ 2 3 ⍴ 1 0 0 1 1 1` | 1 |
| `~ int:ensureGeneric 2 3 ⍴ 1 0 0 1 1 1` | 1 |
| `~ (int:asBigint 1) (int:asBigint 0)` | 1 |
| `~ (int:asBigint 1) 0` | 1 |
| `(int:asBigint 1) (int:asBigint 0) (int:asBigint 1) (int:asBigint 0) ⍲ (int:asBigint 0) (int:asBigint 0) (int:asBigint 1) (int:asBigint 1)` | 1 |
| `(int:asBigint 1) (int:asBigint 0) (int:asBigint 1) (int:asBigint 0) ⍱ (int:asBigint 0) (int:asBigint 0) (int:asBigint 1) (int:asBigint 1)` | 1 |
| `2 3 4 + 1.0 1.0 0∧1.0 0.0 1` | 1 |
| `2 3 4 + 1.0 1.0 0∨1.0 0.0 1` | 1 |

### Parallel (8)

`¨∥` parallel each.

| example | count |
|---------|-------|
| `{1+⍵}¨∥ 10` | 1 |
| `{1+⍵}¨∥ 10 11 12 13 14 15` | 1 |
| `{1+⍵}¨∥ ⍳10000` | 1 |
| `∥` with matrix | 5 |

### Assignment edge (8)

| example | count |
|---------|-------|
| `a←1+b←2 ◊ c←10 ◊ a b c` | 1 |
| `(a) ← ,1` | 1 |
| `(a) ← 1` | 1 |
| `(a ; (b ; c) ; d) ← (1 ; (2 ; 3) ; 4) ◊ a b c d` | 1 |
| `(a (b ; c (d ; e))) ← (1 (2 22; 3 (4 ; 5))) ◊ a b c d e` | 1 |
| `b ((b←2) + 10)` | 1 |
| `foo bar←10` | 1 |
| `(a b c d e f) ← 3 2 ⍴ 10 20 30 40 50 50` | 1 |

### Compare (7)

`= ≡ ≠ cmp` with edge types.

| example | count |
|---------|-------|
| `${a}=${b}` | 1 |
| `${a}≡${b}` | 1 |
| `@a @a 1 @a @a = 1 @b @b @a 1.1` | 1 |
| `(1;2) (1;2) (2;1;3) (1;2) 3 = (2;1) (1;2) (2;1) 3 (1;2)` | 1 |
| `@a = @a` | 1 |
| `'foo ≠ 'foox` | 1 |
| `≡ +/¨ ⍳4 5 6` | 1 |
| `≡ comp +/¨ ⍳4 5 6` | 1 |
| `2.1 cmp (int:asBigint 2)` | 1 |
| `¯0.0 = (int:asBigint 0)` | 1 |
| `¯0.0 ≡ int:asBigint 0` | 1 |
| `:a = :a` | 1 |

### Namespace (7)

| example | count |
|---------|-------|
| `foo:bar ← 1 ◊ a:bar ← 2 ◊ foo:abc ← 3 ◊ foo:bar a:bar foo:abc` | 1 |
| `namespace("foo") 'bar` | 1 |
| `namespace("foo")` | 1 |
| `declare(:export kap:foo)` | 1 |
| `namespace("foo")` | 1 |
| `namespace("bar")` | 1 |
| `use("test-data/use-test.kap")` | 1 |

### Scalar (7)

| example | count |
|---------|-------|
| `math:ceilc 1.4` | 1 |
| `math:floorc 5.9` | 1 |
| `math:floorc 3.4J0.01` | 1 |
| `(⊂1 2 3) + 10 20` | 1 |
| `1 + (⊂1 2) + 2 3 ⍴ ⍳6` | 1 |
| `1 + (⊂1 2) + int:ensureGeneric 2 3 ⍴ ⍳6` | 1 |
| `\| ⊂ 1 2 3` | 1 |
| `comp \| ⊂ 1 2 3` | 1 |
| `1.0 ∊ 1.0 1.1 × 2.8` | 1 |

### Dates (7)

| example | count |
|---------|-------|
| `time:toTimestamp 1654321234599` | 1 |
| `time:fromTimestamp time:toTimestamp 1654321234599` | 1 |
| `time:format time:toTimestamp 1654355533333` | 1 |
| `time:parse "2022-06-04T15:12:13.333Z"` | 1 |
| `time:parse "2022-02-03T00:00:04Z"` | 1 |
| `(time:parse "2022-06-04T15:12:13.333Z") ≡ 12345` | 1 |
| `(time:parse "2022-06-04T15:12:13.333Z") = 12345` | 1 |

### Intersection (6)

`∩` with matrices, nested.

| example | count |
|---------|-------|
| `(2 2 ⍴ ⍳4) ∩ (2 2 ⍴ 0 1 4 5)` | 1 |
| `(2 2 ⍴ ⍳4) ∩ (2 2 ⍴ 4 5 6 7)` | 1 |
| `(3 2 2 ⍴ ⍳12) ∩ (2 2 2 ⍴ 0 1 2 3 4 5 6 7 )` | 1 |
| `(2 2⍴⍳4) ∩ (2 2⍴⍳4)` | 1 |
| `(⍳3 3) ∩ ⊂(1 0) (1 1) (1 2)` | 1 |
| `(⍳1 3) ∩ ⊂(0 0) (0 1) (5 2)` | 1 |
| `(2 2 ⍴ ⍳4) ∩ (0 2 ⍴ ⍬)` | 1 |

### Nil (6)

`⦻` nil.

| example | count |
|---------|-------|
| `⦻ 2 + 1 ⦻` | 1 |
| `⦻ 3 × 4 ⦻` | 1 |
| `2 3 - ⦻` | 1 |
| `⦻` with array | 3 |

### Unicode (6)

| example | count |
|---------|-------|
| `unicode:toCodepoints "foo" "zxcvb"` | 1 |
| `unicode:fromCodepoints 99 100 101 (102 103)` | 1 |
| `unicode:toNames "a å\uD83D\uDE3A"` | 1 |
| `unicode:` other | 3 |

### Where (6)

`⍸˝` inverse where.

| example | count |
|---------|-------|
| `⍸˝ 4 5` | 1 |
| `⍸˝ 2 2 4` | 1 |
| `⍸˝ (1 2) (2 3)` | 1 |
| `⍸` with matrix | 3 |

### Optimiser (6)

| example | count |
|---------|-------|
| `⌊(⍳4 4)÷6` | 1 |
| `↑⍋ 2 1 8 1 0 ¯2 0` | 1 |
| `(↑⍋) 2 1 8 1 0 ¯2 0` | 1 |
| `(⊢↑⍋) 2 1 8 1 0 ¯2 0` | 1 |
| `↑⍋⍬` | 1 |
| `↑⍋ 3 3 ⍴ 1 2 3 4 3 3 2 1 1` | 1 |

### SQL (6)

| example | count |
|---------|-------|
| `testing-found` | 1 |
| `fooId` | 1 |
| `test string` | 1 |
| `abc` | 1 |
| `sql:connect "jdbc:sqlite::memory:"` | 1 |
| `c ← sql:connect "jdbc:sqlite::memory:"` | 1 |

### Concatenate bracket-axis (5)

| example | count |
|---------|-------|
| `(4 5 ⍴ ⍳20) ,[1] 1000+⍳4` | 1 |
| `(4 5 ⍴ ⍳20) ,[0] 1000+⍳5` | 1 |
| `1 2 3 4 ,[0.5] ⊂"foo"` | 1 |
| `(⊂"foo") ,[0.5] 10 11 12 13` | 1 |
| `0 ,[2] 0 ,[1.5] 2 2 ⍴ 1 2 3 4` | 1 |

### Eval order (5)

| example | count |
|---------|-------|
| `a + 1 + a←2` | 1 |
| `1 and 2 ; 10 or 11` | 1 |
| `1 and 2 ; 10 and 11` | 1 |
| `1 and 2 and 3 ; 100 or 200 or 300` | 1 |
| `1 and 2 ; 10 or 11 ; 100 or 101` | 1 |

### Exceptions (5)

| example | count |
|---------|-------|
| `{'foo throw 1}catch 1 2 ⍴ 'foo λ{2+⍺}` | 1 |
| `{'foo throw 1}catch 'foo λ{2+⍺}` | 1 |
| `{'foo throw 1}catch 4 2 ⍴ 'xyz λ{2+⍺} 'test123 λ{3+⍺} 'bar λ{4+⍺} 'foo λ{5+⍺}` | 1 |
| `{'foo throw 1}catch 'xyz λ{2+⍺} 'test123 λ{3+⍺} 'bar λ{4+⍺} 'foo λ{5+⍺}` | 1 |
| `∇ foo (x) {` | 1 |

### Math stdlib (5)

| example | count |
|---------|-------|
| `⌹ 5 5 ⍴ 1 0 0 0 0 0` | 1 |
| `(4 4⍴12 1 4 10 ¯6 ¯5 4 7 ¯4 9 3 4 ¯2 ¯6 7 7)⌹93 81 93.5 120.5` | 1 |
| `⌹3 3⍴1 2 3 4 15 16 7 18 9` | 1 |
| `⌹` other | 2 |

### Sort (5)

| example | count |
|---------|-------|
| `⍋ (1;2) (2;1)` | 1 |
| `⍋ "foo" "bar" 'somename` | 1 |
| `⍋ 1 2 3 'somename` | 1 |
| `⍋` with matrix | 2 |

### Array lookup (5)

| example | count |
|---------|-------|
| `(⊂ 5 10) ⌷ 100+⍳100` | 1 |
| `(3 0) (3 2) ⌷ 4 5⍴100+⍳100` | 1 |
| `(⊂ 3 0) ⌷ 4 6 ⍴ 100+⍳100` | 1 |
| `⌷` with nested | 2 |

### Regex (5)

| example | count |
|---------|-------|
| `n98765,0` | 1 |
| `(:multiLine regex:compile "^foo") regex:match "a` | 1 |
| `(:ignoreCase regex:compile "^foo$") regex:match "foO"` | 1 |
| `"x([A-Z])" regex:replace ("fooxCbarxDtest";λ{"A",(⊃⍵[1]),"B"})` | 1 |
| `x123` | 1 |
| `x123yab` | 1 |

### XML (5)

| example | count |
|---------|-------|
| `foo` | 1 |
| `foo` | 1 |
| `a` | 1 |
| `ghi` | 1 |
| `ghi` | 1 |

### CSV (4)

| example | count |
|---------|-------|
| `foo` | 1 |
| `foo` | 1 |
| `b` | 1 |
| `foo` | 1 |

### Unique (4)

| example | count |
|---------|-------|
| `abc` | 1 |
| `a` | 1 |
| `(⍳3 3) ∪ ⊂(1 0) (1 1) (1 1)` | 1 |
| `∪` other | 1 |

### Disclose (4)

| example | count |
|---------|-------|
| `⊃[2 1]2 3 4 ⍴ (2 2 ⍴ 0 1 101 102) (2 2 ⍴ 2 3 103 104) (2 2 ⍴ 4 5 105 106) (2 2 ⍴ 6 7 107 108) (2 2 ⍴ 8 9 109 110)` | 1 |
| `validate` | 1 |
| `(⊂8 7)⊃10 20 ⍴ 100+⍳100` | 1 |
| `({0⍳⍨(↑⍵)=⍵}⍤1) ⍉(30⍴2)⊤10 11 12` | 1 |
| `⊃ 200000 ⍴ (⊂1000000 ⍴ 1)` | 1 |
| `(1 ⍬ 2) ⊃ (1 2) (⊂9 8 7) (3 4)` | 1 |

### Iota (4)

| example | count |
|---------|-------|
| `⍳ 4 5` | 1 |
| `⍳ 2 3 2` | 1 |
| `⍳,9` | 1 |
| `⍳⍬` | 1 |
| `⍳2 2` | 1 |

### Operators (4)

| example | count |
|---------|-------|
| `a ⇐ {,[⍺]/⍵} ⋄ ⊃ 1 a (2 4 3 ⍴ ⍳100) (2 3 ⍴ 100+⍳100)` | 1 |
| `f0 ← λ{⍺+⍵}` | 1 |
| `f0 ← λ{⍺+⍵}` | 1 |
| `f0 ← λ{⍺+⍵}` | 1 |

### Reduce (4)

| example | count |
|---------|-------|
| `6+/1+⍳5` | 1 |
| `+/ (↑ 2 3 × 5000000000000000000) 5000000000000000004 5000000000000000003 5000000000000000003` | 1 |
| `+/ (↑ 5000000000000000000 × 2 3) 5000000000000000004 5000000000000000003 5000000000000000003` | 1 |
| `×/ 68 ⍴ 2` | 1 |

### Transpose (4)

| example | count |
|---------|-------|
| `0 0 ⍉ 4 4 ⍴ ⍳ 16` | 1 |
| `0 0 0 ⍉ 3 3 3 ⍴ ⍳27` | 1 |
| `1 1 0 ⍉ 2 20 3 ⍴ ⍳120` | 1 |
| `1 1 0 0 ⍉ 2 3 4 5 ⍴ ⍳1000` | 1 |
| `⍬ ⍉ 2 3 4 5 ⍴ ⍳100` | 1 |

### Bitwise (3)

| example | count |
|---------|-------|
| `~∵ 202` | 1 |
| `⍸∵ ¯1 ¯2 ¯12345 ¯0x1000000000000000000000000000000000000000000000000000000000000000000000000003` | 1 |
| `⍸∵ ¯8934789534758934790234908234890723894723897589023475239084902384023758 ¯0x1000000000000000000000000000000000000000000000000000000000000000000000000003 (-2⋆200)` | 1 |
| `⍸∵ int:asBigint¨ 2⋆1+⍳60` | 1 |

### toBoolean (3)

| example | count |
|---------|-------|
| `toBoolean¨ 1 1.0 0 0.0` | 1 |
| `toBoolean¨ 2 4 100 100000000000000000000000000000000000000000 ¯1 ¯10000 1.1 ¯1.1 0.1` | 1 |
| `toBoolean¨ (1÷2) (1÷100000000000000000000000000000000000000000000000) (10÷9)` | 1 |

### math:factor (3)

| example | count |
|---------|-------|
| `math:factor 0 1 2 3 4 5 6 7 8` | 1 |
| `math:factor 312430759692903949351680000` | 1 |
| `math:factor int:asRational 10` | 1 |

### Identity (3)

| example | count |
|---------|-------|
| `⊢˝ 1234` | 1 |
| `9 ⊢˝ 1234` | 1 |
| `9 ⊣⍨˝ 1234` | 1 |

### Stdlib simple (3)

| example | count |
|---------|-------|
| `io:toHex¨ 74667 4096 0 16 3` | 1 |
| `12 io:toHex 74667` | 1 |
| `io:base64Encode io:encodeUtf8 "teststring1234"` | 1 |

### Compose (3)

| example | count |
|---------|-------|
| `-+«,»× 2 5` | 1 |
| `a ⇐ { (⍺+) ⍵ } ⋄ 3 a 1 2 3` | 1 |
| `a ⇐ { (⍺+)¨ ⍵ } ⋄ 3 a 1 2 3` | 1 |

### Flowcontrol (3)

| example | count |
|---------|-------|
| `∇ foo (x) { λ{⍵+x} }` | 1 |
| `foo ← λ{ x ← 1 + ⍵ }` | 1 |
| `foo ← λ{ x ← 1 + ⍵ ◊ y ← { declare(:local x) x ← 2 ◊ x+50+⍵ } 190 ◊ y+x }` | 1 |

### JVM (3)

| example | count |
|---------|-------|
| `foostring` | 1 |
| `foo` | 1 |
| `test` | 1 |

### math:divisors (2)

| example | count |
|---------|-------|
| `math:divisors 0 1` | 1 |
| `math:divisors 4 5 6 2` | 1 |

### Expand (2)

| example | count |
|---------|-------|
| `1 0 1 1 \ 2` | 1 |
| `0 2 2 \ 3 1 ⍴ 100+⍳9` | 1 |

### Expand bracket-axis (2)

| example | count |
|---------|-------|
| `1 0 1 1 \\[0] 3 3 ⍴ 100+⍳9` | 1 |
| `1 0 1 1 \\[1] 3 3 ⍴ 100+⍳9` | 1 |

### math:gcd/lcm (2)

| example | count |
|---------|-------|
| `219060189739591200 math:lcm 106` | 1 |
| `math:lcm/ 1+⍳50` | 1 |

### Bigint (2)

| example | count |
|---------|-------|
| `9223372036854775807 + 1` | 1 |
| `1 + 10 9223372036854775807` | 1 |

### Number types (2)

| example | count |
|---------|-------|
| `10 + 2.5r` | 1 |
| `123456789012345678901234567890 + 2.5r` | 1 |

### Output formatter (2)

| example | count |
|---------|-------|
| `o3:format ,1` | 1 |
| `10r11 ¯10r11 (int:asRational 0)` | 1 |

### Prime (2)

| example | count |
|---------|-------|
| `math:isPrime int:asRational¨ 0 1 2 3 4 5` | 1 |
| `math:isPrime 9223372036854775779+⍳10` | 1 |

### Scan (2)

| example | count |
|---------|-------|
| `+\ 10` | 1 |
| `+\ 0⍴0` | 1 |

### Select (2)

| example | count |
|---------|-------|
| `1 0 1 ⫽ 1 2 3` | 1 |
| `2 1 1 /[4] 7 6 5 4 3 ⍴ ⍳1000` | 1 |

### Compose operators (2)

| example | count |
|---------|-------|
| `10 (×∘(20+))⍨˝ 60` | 1 |
| `10 ((20+)⍛×)˝ 60` | 1 |

### Encoder (2)

| example | count |
|---------|-------|
| `x ← encoder:encode 2 ⋆ ⍳ ${n} ⋄ encoder:decode x` | 1 |
| `x ← encoder:encode -2 ⋆ ⍳ ${n} ⋄ encoder:decode x` | 1 |

### HTML (2)

| example | count |
|---------|-------|
| `abctest` | 1 |
| `abc` | 1 |

### Arrow (2)

| example | count |
|---------|-------|
| `arrow:makeVector (1 2 100 200 ; 'arrow:bigint)` | 1 |
| `arrow:makeVector (10 20 30 40 50 6 ; 'arrow:int)` | 1 |

### Exec (1)

| example | count |
|---------|-------|
| `test string` | 1 |

### JSON (1)

| example | count |
|---------|-------|
| `test` | 1 |

### Key (1)

| example | count |
|---------|-------|
| `2 1 3 1 ,/⌸ "foo" "bar" "xyz" "abcdef"` | 1 |

### Null fallthrough (1)

| example | count |
|---------|-------|
| `⍮⍰ 1 2` | 1 |

### Complex-expr (1)

| example | count |
|---------|-------|
| `(2 0x6000000000000000 + 0x6000000000000000) =¨ ⊂10 20 0x6000000000000000 3 + 0x6000000000000000` | 1 |

### Enclose (1)

| example | count |
|---------|-------|
| `⊂[,1] 2 3 2 ⍴ 300+⍳1000` | 1 |

### Filesystem (1)

| example | count |
|---------|-------|
| `abc` | 1 |

### FFI (1)

| example | count |
|---------|-------|
| `content = ${result}` | 1 |

---

## DISPLAY — glyph conventions (faithful design choice, NOT bugs)

The port deliberately renders in Kap's house glyphs. Values compute correctly;
only the textual form differs from the oracle's JVM display. This is why many
broad-sweep cases read as "mismatch" even when the underlying computation is
right.

| thing | port | oracle |
|-------|------|--------|
| negative number | `¯1` | `-1` |
| complex | `3.0J+4.0` | `3.0J4.0` |
| nested vector | `((1 2) (3))` | `⟨⟨1 2⟩ ⟨3⟩⟩` |
| plain vector | `1 2 3` | `⟨1 2 3⟩` |
| scalar result of `⊇`/`⊂` | `(x)` | `x` |

The curated parity gate (`curated_kap_parity`) encodes the port's glyphs
intentionally, so it stays green. The broad sweep compares against the oracle's
exact bytes and thus flags these as mismatch — this is expected noise.

---

## MSG — error-message quality gaps (behavior is correct)

These reject input the same way the oracle does; only the wording differs.
Low priority.

| expr | port | oracle |
|------|------|--------|
| `1 2 3[0]` (stranded idx) | `pick into rank-0 array requires coordinate indices` | `Index list length must be less than or equal to the rank of the argument` |
| undefined `dbl¨ 1 2 3` | `undefined symbol: dbl` | `Operator without left function: ¨` |
| `≡` with no args | `undefined symbol: ≡` | `No arguments specified for function` |

---

## Re-baseline protocol

To refresh the numbers and the mismatch/unsupported sample:

```sh
cargo build -p kap-cli
cargo test -p kap-core --test conformance run_kotlin_conformance 2>&1 | tail -3
# summary is written to conformance_summary.txt (repo root)
```

Gates that MUST stay green (independent of the broad sweep):
- `cargo test -p kap-core --lib` → 92 passed, 0 failed
- `cargo test -p kap-core --test conformance curated_kap_parity` → 1 passed, 0 failed
