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

## CRITICAL — `⌷` (squad / index selection) is mis-dispatched

`⌷` is wired to `disclose` in `eval_apply`
(`"⌷" | "reveal" | "disclose" => self.disclose(right_val)`, evaluator.rs:1178).
In Real Kap, `⌷` is **index selection** (squad), a completely different
function. Consequence: ALL `⌷`-based selection silently returns the wrong thing.

| expr | port | oracle |
|------|------|--------|
| `2 ⌷ 1 2 3 4` | `(1 2 3 4)` | `3` |
| `¯1 ⌷ 1 2 3 4 5` | `(1 2 3 4 5)` | `5` |
| `⌷ 1 2 3 4` (monadic) | `(1 2 3 4)` | `⟨⟨1 2 3 4⟩⟩` |
| `⍴(0 1 400)(1 ¯2)(0 1)⌷3 3 3⍴⍳100` | `(27)` | **error**: Index out of bounds (400 > axis size 3) |

Bracket indexing is only partially present: `(1 2 3)[0]` → `1` works, but
stranded index on a bare vector (`1 2 3[0]`) errors with a wrong message, and
out-of-bounds does **not** raise the oracle's "Index out of bounds" error.
This is the single largest mismatch/unsupported driver (LookupTest,
indexLookup*, MultiAxis* families).

**Fix direction**: implement a real `squad`/`index_select` (Kotlin
`IndexAPLFunction` / `PickResultValue`), separate from `disclose` (`⊃`).

---

## CRITICAL — `≡` / `≢` (match) use wrong semantics

Dyadic `≡` is implemented as plain `deep_equal → 1/0`
(`match_or_depth`, evaluator.rs:4230). Real Kap `≡` is **match**: it returns a
numeric *depth* when the arguments have identical type/depth/structure, else
`0`; and it is **type-strict** (a `Long` never equals a `Double`).

| expr | port | oracle | note |
|------|------|--------|------|
| `10≡10` | `1` | `1` | OK |
| `@a≡@a` | `1` | `1` | OK |
| `10≡10.0` | `1` | `0` | **WRONG** (long ≠ double) |
| `10≢10` | `1` | `0` | **WRONG** |
| `0≢0⌷0 1` | `2` | `0` | **WRONG** (depends on `⌷` bug too) |

Monadic `≡` (nesting depth, `depth_of`) is implemented but **not yet
cross-verified** against the oracle.

**Fix direction**: replace the `deep_equal` 1/0 logic with Kap's match
algorithm (compare element type/strictness + depth, return common depth or 0).

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
