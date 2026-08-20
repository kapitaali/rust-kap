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
not a value defect. Bracket indexing (`x[sel]`) is a separate path and remains partial.

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

## CRITICAL — `⊃` (first / pick) semantics diverge

| expr | port | oracle |
|------|------|--------|
| `⊃1 2 3` (monadic) | `1` | `⟨1 2 3⟩` (disclose → nested vector) |
| `1 2 3 ⊃ 2` (dyadic) | `2` | **error**: Mismatched dimensions for selection |

Monadic `⊃` should disclose (return the enclosed content as a nested vector),
not the first scalar; dyadic `⊃` should be pick-with-selection and reject
mismatched dimensions. Currently both are off.

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
- **Key / major-cell operators** — `⌺` (stencil) and `⌸` (key) unimplemented.
- **Compose operators** — `∘` / `≬` unimplemented.
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
