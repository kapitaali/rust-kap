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

## STRINGS — `⍎` (execute) — CLOSED (2026-08-24, commit pending)

`⍎` is Kotlin's `ParseNumberFunction` (format.kt:250): a **strict number parser**
(integer → double → rational, anchored regexes using ASCII `-`, NOT Kap's `¯`),
which throws `Value cannot be parsed as a number: '<s>'` when nothing matches. It
does NOT evaluate arbitrary expressions (`⍎"1+2"` → error, not 3). The port's old
`eval_string` fallback (which silently evaluated expressions and used `i64` for
rationals, breaking `⍎"1/1e41…"`) is removed. New `number.rs::parse_kap_number_string`
is the faithful port; 14 curated value-rows added and all oracle matrix cases match
on VALUE. Re-baseline numbers above are stale for this builtin — `⍎` is now conformant
(value-level) except the DISPLAY `¯` vs `-` convention and the MSG items below.

## STRINGS — char/string arithmetic error-text — CLOSED (2026-08-24)

Value-level char/string `+ -` and comparisons are conformant (18/18 oracle matrix
cases match on value). The five MSG-class error texts were aligned verbatim to the
Kotlin throw sites (`compare_functions.kt:273/281`, `number.kt:400`,
`types.kt:1617`) and now match the oracle byte-for-byte:

| expr | port = oracle (verbatim) |
|------|--------------------------|
| `98 200 - "aj"` | `-: Incompatible argument types. Left arg: integer, Right arg: char` |
| `@a - 98` | `-: Codepoints cannot be negative: -1` |
| `@a + 1j1` | `+: Number is complex: Complex(re=1.0, im=1.0)` |
| `"a" + "b"` / `@a + @A` | `+: Function does not support char arguments` |

`Str-Str` result `¯1` vs oracle `-1` remains DISPLAY-glyph only (value `[-1]`
correct).

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
- **Axis specifiers** `[axis]` — not accepted for `⌷`, `⊆`, `⍋`/`⍒`, etc. (Scalar
  arithmetic `+ - × ÷ *` DOES now honor `f[axis]`, matching Kotlin — including the
  scalar+scalar short-circuit that ignores the axis. Fixed 2026-08-21.)
- **Dyadic interval `⍸`** (`a ⍸ b`) — IMPLEMENTED (2026-08-24, see where_interval
  reference + PROGRESS). Remaining gap: inverse `⍸˝` (needs the `˝` adverb) — not built.
- **`regex:replace` lambda form** — only the `(subject; replacement)` *string*
  pair is supported; a replacement *function* is unimplemented.
- **`use()` file loading / `.kap` stdlib kernel** (`standard-lib.kap`,
  `base-functions.kap`) — `use()` **IS** implemented and resolves `kap-stdlib/std/*.kap`
  (verified 2026-08-22: `use("io.kap") ⋄ io:encodeUtf8Char @€ → (226 130 172)` matches the
  oracle). Symbols in a `namespace("io")` file land in the `io:` namespace (`io:toHex`,
  `io:encodeUtf8Char`, `io:base64Encode`), NOT bare names. The remaining stdlib gaps are
  individual builtins the stdlib bodies call, not `use()` itself:
  - `io:toHex 255 16` (2-arg form using `isLocallyBound('⍺)`) — port lacks the 2-arg `/⍟`/rank
    plumbing this needs; returns an error today (oracle: `"F1F0"`).
  - `io:base64Encode "Hello"` — needs `256 (⊥⍤1) …` (rank-op applied to a char-multidim
    value) plus `⊤`/`⊤`-on-chars; errors in the port today (oracle: `"SGVsbG8="`).
  These are stdlib-kernel items, tracked separately from the `∵` operator work.
- **`util.kap` regex-literal lines (19–21)** — `trimLeft ⇐ (1⍳⍨@\s≠)⍛↓` and its
  `trimRight`/`trim` dependents use an unsupported `@\s` regex literal inside a
  train. **The JVM oracle also fails these**: `use("util.kap")` in
  kap-jvm-text leaves `trimLeft`/`trimRight`/`trim` unassigned ("Variable not
  assigned"), so there is no parity target to match. The port reports a clean
  parse error (`Operator without left function: ⍨`) for line 19 and skips it;
  all other util.kap exports load. Implementing `@regex` literals would be a
  NEW feature beyond oracle behavior, not a fix. (Verified 2026-08-25, P7c.)

---

## Deliberate extension: `use()` tolerates per-statement errors (03-B4 decision, 2026-08-23)

**Decision: KEEP the tolerant behaviour as an intentional, documented extension** (option b of
analysis 03 §2-B4 / ROADMAP P0.2). The oracle ABORTS a `use()`d file at the first failing
statement (earlier definitions persist); the port logs `warning: use(): statement failed: …`
and continues. Rationale: the vendored stdlib chain (`standard-lib.kap` pulls 13 files, several
with known port gaps) only delivers a usable kernel if one bad file doesn't kill the load; with
abort semantics the whole stdlib startup dies on the first gap. Revisit (flip to abort) after
ROADMAP P1 (parser migration) closes the bulk parse gaps — at that point tolerance hides nothing.

Known residual deltas in this family:
- **B7**: unparenthesized derivation `keys ≠⌸ values` errors ("≠ requires numbers") where the
  oracle groups correctly; the parenthesized form `(≠⌸)` works identically. Parser strand-loop
  item, tracked with ROADMAP P1.
- **`typeof ⌸` error text**: oracle emits `No arguments specified for function`
  (`IllegalContextForFunction`), the port emits `Operator without left function` uniformly for
  incomplete operator applications. Error-class parity holds; exact text is an ERRORS.md item
  (ROADMAP §0.3).
- **Quad-constant class name**: `typeof ⎕A` → oracle `kap:array`, port `kap:string`. The
  port models char vectors as `APLValue::Str` (a flat string type) rather than a
  character **array**, so `typeof "abc"` also reports `kap:string` where the oracle
  reports `kap:array`. This is the **`Str`-vs-char-array string-modeling gap** (a P2/P5
  string-representation item), NOT a P6 gap. P6's concrete deliverables — native
  `⎕A ⎕a ⎕d` constants (value + shape + indexing, oracle-exact) and `declare(:const …)`
  read-only enforcement (error text `Assignment to constant variable: <ns>:<name>`,
  oracle-exact) — are COMPLETE as of 2026-08-25. The `man` namespace leak that surfaced
  during P6 (the REPL's auto `use()` of stdlib left `current=man` from fhelp.kap, so the
  const error read `man:x` instead of oracle `default:x`) was fixed by saving/restoring
  the caller's namespace around `eval_string_in_env_tolerant` (Kotlin `use()` scoping).


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
