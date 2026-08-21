# Known non-conformances — Rust Kap vs Real Kap (Kotlin oracle)

Live register of behavior that does **not** yet match Real Kap, last
re-baselined at commit `737d631` (branch `feature/wheres-extra`, which equals
`main`/`strings` per the invariant). Coverage measured by the broad sweep
(`conformance/kotlin_tests.jsonl`, 2535 extracted Kotlin cases):

```
ok         : 1109   (parsed + evaluated, value matched when expected known)
mismatch   : 299    (ran to a value, but WRONG vs Kotlin)
unsupported: 1127   (parse or runtime error — feature not built yet)
coverage   : 43.7%
```

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

## CRITICAL — `⌷` (squad / index selection) — FIXED (commit 20260820+)

`⌷` was mis-wired to `disclose`. It is now **index selection** (`AccessFromIndexAPLFunction`),
reusing `pick` for the dyadic axis-selection path:

- Monadic `⌷X` = `⟨X⟩` (a length-1 vector whose sole element is `X`).
- Dyadic `A⌷B`: `A` is the position arg; scalars collapse an axis, vectors select a
  sub-axis, and a `⍬`/Null position arg selects the *entire* axis (identity). Negative
  indices count from the end; out-of-range is an error.

Verified against the `kap-jvm-text` oracle (all match):

| expr | port | oracle |
|------|------|--------|
| `2 ⌷ 1 2 3 4` | `3` | `3` |
| `¯1 ⌷ 1 2 3 4 5` | `5` | `5` |
| `⌷ 1 2 3 4` (monadic) | `((1 2 3 4))` | `⟨⟨1 2 3 4⟩⟩` |
| `⍬⌷1 2 3` | `(1 2 3)` | `⟨1 2 3⟩` |
| `0⌷(1 2)(3 4)` | `(1 2)` | `┌─────┐` |

The `()` vs `⟨⟩` difference is the faithful display-glyph convention (see DISPLAY below),
not a value defect.

---

## CRITICAL — bracket indexing `x[sel]` (`Instr::Index` → `index_select`) — IMPLEMENTED (2026-08-21)

`Instr::Index` (the `[i][j]` suffix path in `parser.rs`) now routes to a new
`Engine::index_select` (Kotlin `APLValue.get` / `indexFromPositionNegativeSupport`
in `dimension.kt`), **distinct from `pick`/`⊇`**. Each `;`-separated section of the
selector drives one axis: `⍬`/empty section → whole axis, a scalar index →
**collapse** that axis (Kap discloses the scalar element, not a length-1 array), a
vector of indices → that many elements along the axis. Negative indices wrap
(`¯1` = last). A result whose every section was a scalar is a rank-0 scalar
(`x[1;2]` → `5`, not `(5)`). Chained `x[i][j]` re-indexes the inner result.

Verified against the `kap-jvm-text` oracle (value matches; only `()` vs `⟨⟩` glyph
differs on the multi-element rows):

| expr | port | oracle |
|------|------|--------|
| `(3 4⍴10×⍳100)[2;]` | `(80 90 100 110)` | `⟨80 90 100 110⟩` |
| `(3 4⍴10×⍳100)[;3]` | `(30 70 110)` | `⟨30 70 110⟩` |
| `(3 4⍴10×⍳100)[;0 3]` | `(0 30 40 70 80 110)` | `⟨0 30 40 70 80 110⟩` |
| `(2 3⍴⍳6)[1;⍳3]` | `(3 4 5)` | `⟨3 4 5⟩` |
| `(2 2 2⍴100+⍳8)[1;0;1]` | `105` | `105` |
| `(2 2 2⍴100+⍳8)[1;1;1]` | `107` | `107` |
| `(1 2 3 4 5 6 7 8)[4 4⍴⍳4]` | `(1 2 3 4 1 2 3 4 1 2 3 4 1 2 3 4)` | matrix view |
| `(10 20 30 40)[2]` | `30` | `30` |
| `(10 20 30 40)[0 2]` | `(10 30)` | `⟨10 30⟩` |
| `(10 20 30 40)[¯4]` | `10` | `10` |

Error semantics also match the oracle byte-for-byte ("Index list length must be
less than or equal to the rank of the argument"), including Kap's deliberate
`1 2 3 4[2]` error (the `[2]` binds to the trailing scalar `8`).

**One divergence — pre-existing nested-array representation, NOT an `index_select`
bug:** a vector-of-vectors like `((1 2 3)(4 5 6)(7 8 9))` is generalized by the
port into a true 2-D `(3 3)` array (so `⍴`→`(3 3)`), whereas Real Kap keeps it
rank-1 `(3)`. Consequently the port's chained `((1 2 3)(4 5 6)(7 8 9))[0][2]` → `3`
(indexing the 2-D array) while the oracle errors (a rank-1 array has no second
axis). The `index_select` algorithm is correct for genuine N-D arrays; this is the
same nested-vector generalization gap already tracked under DISPLAY/DEFERRED. No
change made this session — flagged for later.

---

## CRITICAL — `≡` / `≢` (match) — FIXED (commit 20260820+)

Dyadic `≡`/`≢` are now **type-discriminating** equal (not value-equal): a `Long` never
equals a `Double`, a scalar never equals a vector. This is `type_equal`, distinct from `=`
/`≠` which keep `numeric_cmp` value-equal semantics. Monadic `≡` is **depth** (nesting
levels), and `⊂` of a *primitive* returns the primitive unchanged (so `≡⊂5 = 0`, `≡,5 = 1`).

| expr | port | oracle | note |
|------|------|--------|------|
| `10≡10` | `1` | `1` | OK |
| `10≡10.0` | `0` | `0` | OK (type-strict) |
| `10≢10.0` | `1` | `1` | OK |
| `10=10.0` | `1` | `1` | OK (`=` stays value-equal) |
| `(1 2)≡(1 2.0)` | `0` | `0` | OK |
| `≡ 5` | `0` | `0` | OK (depth) |
| `≡⊂5` | `0` | `0` | OK |
| `≡,5` | `1` | `1` | OK |
| `≡⊂,5` | `2` | `2` | OK |

The depth semantics were clarified from Real Kap's `compareEqualsTotalOrdering` + `disclose`
rules: a simple scalar has depth 0; `⊂` of a primitive returns the value itself (still depth
0); turning a scalar into a 1-element vector via `,` gives depth 1; `⊂` of a non-primitive
produces a 0-dimensional box whose depth equals its content's depth.

As a side effect of the depth clarification, `⊂` (enclose) was fixed (primitive → unchanged;
non-primitive → 0-d box), and monadic `⍮` (pair → `⟨x⟩`) and monadic `,` (ravel → rank-1
vector) were implemented, since `≡⊂,5` and related depth expressions require them.

---

## CRITICAL — `⊃` (first / pick) — FIXED (2026-08-21)

`⊃` now maps to Kap's `DiscloseAPLFunction` (`disclose.kt`):
- Monadic `⊃X` = disclose: drop the outer box level. Simple arrays are identity,
  `⊂`-boxed scalars unwrap (`⊃⊂5`→`5`), `(1 2)(3 4)` drops the outer axis to a 2×2
  (`(1 2 3 4)` with shape `2 2`).
- Dyadic `A⊃B` = nested pick (selector iterates over `B`), distinct from `⊇`/`pick`,
  with Kap's exact dimension errors:
  - scalar selector into a scalar arg → `⊃: Mismatched dimensions for selection`
  - nested selector whose shape ≠ rank of `B` → `⊃: Dimensions does not match`
  - out-of-range (positive; negatives wrap) → `⊃: Selection index out of bounds`

| expr | port | oracle |
|------|------|--------|
| `⊃1 2 3` (monadic) | `(1 2 3)` | `⟨1 2 3⟩` |
| `⊃(1 2)(3 4)` | `(1 2 3 4)` shape `2 2` | matrix `2 2` |
| `⊃⊂5` | `5` | `5` |
| `2 ⊃ 1 2 3 4` | `3` | `3` |
| `1 ⊃ (1 2 3)(4 5 6)` | `(4 5 6)` | `(4 5 6)` |
| `1 2 3 ⊃ 2` | error: Mismatched dimensions for selection | error: Mismatched dimensions for selection |
| `(1 2 3)(4 5 6) ⊃ 1` | error: Dimensions does not match | error: Dimensions does not match |
| `5 ⊃ 1 2 3 4 5` | error: Selection index out of bounds | error: Selection index out of bounds |

Only display-glyph diffs remain (`()` vs `⟨⟩`, matrix borders) — house style, not
defects. Error-text cases verified by hand against the Kotlin oracle + source
(this test harness cannot assert error messages).

---

## DISPLAY — `≬` / `toList` (Kotlin `ToListFunction`) — IMPLEMENTED (2026-08-21)

`≬` is **not** a compose operator (the old roadmap `∘`/`≬` grouping was a mislabel;
`∘`/`⍛` are already-done compose trains). It is Kotlin `ToListFunction`
(div_functions.kt), registered in BOTH `evaluator.rs::is_primitive_name` and
`parser.rs::is_primitive_op` as `≬` / `toList`, with inverse `fromList`.

- Monadic-only. A scalar or 1-D array is boxed into a rank-0 array whose single
  element is the value coerced to a Kap *list* (oracle `⟨⟩` type). Unlike `⊂`,
  `≬` **always** boxes even a primitive scalar (`≬5 → (5)`, not `5`).
- A rank>1 argument errors: "Argument must be a scalar or 1-dimensional array"
  (matches the oracle byte-for-byte).
- Dyadic application errors: "Function cannot be called with two arguments".

Verified against the `kap-jvm-text` oracle (value matches; only `()` vs `⟨⟩` glyph
differs):

| expr | port | oracle |
|------|------|--------|
| `≬ 1 2 3` | `((1 2 3))` | `⟨1 2 3⟩` |
| `≬ 5` | `(5)` | `⟨5⟩` |
| `≬ "abc"` | `("abc")` | `⟨"abc"⟩` |
| `≬ 2 2⍴⍳4` | error: Argument must be a scalar or 1-D | error: same |
| `3 ≬ 5` | error: cannot be called with two arguments | error: same |
| `fromList ≬ 1 2 3` | `(1 2 3)` | `⟨1 2 3⟩` |

The `()` vs `⟨⟩` difference is the faithful display-glyph convention (see DISPLAY
below), not a value defect. Added 3 curated parity rows (value-only; error-text
rows are noted but not assertable by the harness).

---

## DISPLAY — `⍕` monadic format (Kotlin `FormatAPLFunction`) — FIXED (2026-08-21)

Monadic `⍕ x` now uses `formatted(FormatStyle.PLAIN)`: it **recursively flattens
`x` to its scalar leaves and concatenates them with NO separators and NO
parentheses** (matching the oracle). Previously the port used `format_value`,
which wrapped arrays in `( … )` with spaces (`⍕ 1 2 3` → `"(1 2 3)"` instead of
`"123"`). The dyadic `⍕` directive path (`$s`/`$h`/`$$`, Kotlin `format.kt`) was
already correct and is unchanged.

Verified against the `kap-jvm-text` oracle (value matches byte-for-byte):

| expr | port | oracle |
|------|------|--------|
| `⍕ 1 2 3` | `"123"` | `"123"` |
| `⍕ 10 20 30` | `"102030"` | `"102030"` |
| `⍕ (2 2⍴⍳4)` | `"0123"` | `"0123"` |
| `⍕ ⊂1 2 3` | `"123"` | `"123"` (box flattened) |
| `⍕ ⊂5` | `"5"` | `"5"` |
| `⍕ 1.5 2.5` | `"1.52.5"` | `"1.52.5"` |
| `⍕ "ab" "cd"` | `"abcd"` | `"abcd"` (strings flatten to chars) |
| `⍕ ⍬` | `""` | `""` |

Implementation: new `APLValue::format_plain` in `lib.rs` (mirrors Kotlin
`formatted(PLAIN)` — descends into arrays/strings, concatenates scalar leaves);
`evaluator.rs` `⍕` monadic arm now calls `format_plain` instead of `format_value`.
Added 8 curated parity rows (monadic flatten cases).

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

## DEFERRED — not yet implemented (currently Unsupported)

Features the engine does not build yet. Most are tracked in `ROADMAP.md`.

- **Complex numbers** — `math:re`/`math:im`/`×⌻⍨` etc. are absent
  (`math:re 3j4` → `undefined symbol: re`; the oracle returns `3.0`). Complex
  *literals* parse (`3j4`) and some arithmetic works, but the `math:` module
  and complex-aware comparisons are missing. This is the dominant `unsupported`
  cluster in the sweep.
- **Key / major-cell operators** — `⌺` (stencil) and `⌸` (key) are **NOT native
  Kotlin builtins**: they resolve through the `use()`-loaded stdlib (`kap:keys` /
  `kap:stencil` in `base-functions.kap`), which the port's deferred stdlib-kernel
  cannot load. They are therefore out of reach until `use()` is implemented. (Confirmed
  2026-08-21 by reading `engine.kt` — `⌸`/`⌺` appear only in the *lexer symbol set*,
  not as registered native functions; only `keys`/`map` is native.)
- **Compose operators** — `∘` / `⍛` (compose / reverse-compose trains) are wired;
  `≬`/`toList` is now implemented (see DISPLAY section).
- **Axis specifiers** `[axis]` — not accepted for `⌷`, `⊆`, `⍋`/`⍒`, etc.
- **Dyadic interval `⍸`** (`a ⍸ b`) and inverse `⍸˝` (needs `˝` adverb) —
  returns a clean "not implemented" error.
- **`regex:replace` lambda form** — only the `(subject; replacement)` *string*
  pair is supported; a replacement *function* is unimplemented.
- **`use()` file loading / `.kap` stdlib kernel** (`standard-lib.kap`,
  `base-functions.kap`) — deferred; `s:` string helpers and other stdlib
  functions are therefore absent from the REPL.

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
