# PROGRESS 2026-08-26 (c) — `util.kap` load fix (newline statement-boundary)

## Blocker (resolved)
`use("util")` reported `1/12 statements failed in util.kap: error: ≠ requires numbers`.

### Root cause
`util.kap:21` `trim ⇐ trimRight trimLeft` adds `trim` to `known_functions` mid-file.
At `util.kap:22` `declare(:export (trimLeft trimRight trim))`, the Kotlin-accumulator
loop (parser.rs) routes the *known-function* symbol `trimLeft` through `finish_fn_call`.
`finish_fn_call` called `bind_operators_kotlin`, which **begins with `skip_newlines()`** —
swallowing the newline that terminates line 21. The next statement's `declare(...)`
(token 120) was then mistaken for a right argument of `trimLeft`, producing
`Apply{ fn_expr: trimLeft, right: declare(...) }`. Evaluating it eagerly applies
`trimLeft`'s body `(1⍳⍨@\s≠)⍛↓` with no right arg → `(1⍳⍨@\s≠)` runs → `≠ requires numbers`.

### Fix
`parser.rs:finish_fn_call` — capture `newline_before_right = matches!(peek, Newline)`
BEFORE calling `bind_operators_kotlin`, and fold it into `has_right` so the statement
terminator is respected. `bind_operators_kotlin` no longer swallows a newline that
should end the statement. Fork/operator binding on the SAME line (no intervening
newline) is unaffected because no newline is captured there.

## Verification (oracle = kap-jvm-text)
| probe                              | port (R)        | oracle (O)       | match |
|------------------------------------|-----------------|------------------|-------|
| `use("util")` warnings             | none for util   | (loads clean)    | ✓     |
| `s:trimLeft "  ab c "`             | `"ab c "`       | `"ab c "`        | ✓     |
| `s:trimRight "  ab c  "`           | (⍢ gap, see note)| `"  ab c"`       | —     |
| `3 (+ « × » -) 4` (fork regress.)  | `¯7`            | `-7`             | ✓     |

## Gates
- `cargo test -p kap-core --lib` → **96 passed, 0 failed** (was 1 fork failure during
  the first, too-aggressive boundary-skip attempt; corrected).
- `cargo test -p kap-core --test conformance curated_kap_parity` → **1 passed, 0 failed**.
- `cargo build -p kap-cli` → Finished.

## Known secondary gap (NOT in scope, NOT a regression)
`⍢` (structural under) errors `under not supported for function` for non-self-inverse
wrappers e.g. `trimLeft⍢⌽` / `(-⍢-)`. Confirmed PRE-EXISTING via `git stash` of this
fix (baseline shows the same error). `util.kap` line 19 (`trimLeft`) — the reported
blocker — is fully fixed and oracle-exact. `trimRight`/`trim` (L20-21) depend on the
`⍢` gap and are out of scope for this fix.

## Out-of-scope `use()` warnings (§10, unchanged)
`http.kap`, `output3.kap`, `map.kap`, `fhelp.kap`, `standard-lib.kap` (int:libInitialised)
still emit load warnings — separate bugs, not addressed here.
