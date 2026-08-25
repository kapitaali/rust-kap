# PROBLEM.md — OPEN problems (rewritten 2026-08-25)

*Problems 1–3 of the previous revision are CLOSED and moved out (see
`PROGRESS-20260825.md` for their resolutions):*

- **P7c paren-group/nested-fn-group + `declare(:export …)`** — RESOLVED
  (commits 6441a30, a996a24, 15ab3a7).
- **Bound-constant commute `2÷⍨` (bare AND paren forms)** — RESOLVED:
  `(2÷⍨≢) 8 → 1/2`, `f ⇐ 1 2+≢ ⋄ f 5 → (2 3)` oracle-exact (commit 7fc3db2).
- **stat:median last mile (`⇐` RHS `⍛` binding)** — RESOLVED: inline,
  unparenthesised, and `use("stat.kap")` forms all give `5/2` oracle-exact
  (commits 49f1286, 21123ee, 004100f). The `⍛`/`∘` dedicated-token arms exist
  in BOTH parser paths (bind_operators_kotlin ~:748 and parse_apply ~:1824);
  `declare()` returns statement-complete instead of stranding into the next
  ∇ definition.

All gates green throughout: lib 96/0 · curated parity 1/0.
No debug `eprintln!` lines left in the tree (working tree clean at 004100f
plus an uncommitted ROADMAP P7-table status edit).

---

# OPEN-1 — Adjacent symbol literals strand wrongly (blocks stat.kap `classify`, map.kap)

## Symptom (oracle-vs-port, both captured)
```
port : 'a 'b 'c   →  c            (only the LAST symbol survives)
oracle: 'a 'b 'c  →  ⟨default:a default:b default:c⟩   (a proper 3-strand)
port : 'a 1 2     →  (a 1 2)      (symbol + values strands fine)
```
A leading symbol literal followed by MORE symbol literals loses all but the
last one. Value-stranding after one symbol works.

## Where it bites
- `stat.kap:21` `if (∧/ (typeof¨ ⍵) ∊ 'kap:integer 'kap:float 'kap:rational)`
  → port: `parse error at 21:36: expected ')' after if condition`
- `map.kap:13` same shape inside `'kap:map ≡ typeof m or 'kap:IllegalArgumentException int:throwNative …`

## Evidence / ruled out
- NOT the lexer swallowing whitespace: a single symbol parses; symbol-then-
  numbers strand correctly; only symbol-followed-by-symbol collapses.
- Suspect: `is_strand_operand` / the Kotlin-path accumulator treats a second
  adjacent `Symbol` token as an OPERATOR position (or the first symbol as a
  fn atom), so the strand loop never collects them.

## Open question
Which gate misclassifies the SECOND adjacent symbol literal —
`parse_value_kotlin`'s Symbol arm (it pushes ONE symbol per iteration but a
following Symbol hits the operator-without-left-function check?) or the
legacy strand loop's operand predicate?

---

# OPEN-2 — Destructuring assignment inside defsyntax bodies fails (blocks `when`/`unwindProtect`)

## Symptom (minimal repro, port-only; oracle has no equivalent probe because
its own structure.kap loads fine)
```
defsyntax mywhen (… :repeat (entryList wInner) …) {
  …
  (cond fn) ← ↑entryList[i]        ← FAILS HERE
  (⍞cond ⍬) and (res ← ⍞fn ⍬ ⋄ cont ← 0)
  …
}
mywhen { (1){42} }
→ error: destructuring assignment expected 2 values, got 1
```

## Evidence / ruled out
- Top-level destructuring works: `(q0 r0) ← QR a0` shapes elsewhere in the
  stdlib parse (math-kap uses them; those files get past this line).
- `declare (:local a)` + plain assign INSIDE a defsyntax body works
  (util.kap filter repro passes: `filt (1 2 3 4) {0=2|⍵}` → `(2 4)`).
- So the failing piece is specifically DESTRUCTURING where the RHS is an
  INDEXED expression `↑entryList[i]` evaluated inside macro-expansion scope —
  either the `[i]` index suffix binds differently inside the macro body, or
  `↑` (First) returns the element UNWRAPPED so destructure sees 1 value.

## Open question
Does `↑entryList[i]` evaluate to a 2-element array at that point (then the
destructure arm has a bug), or does the index suffix mis-bind (returning the
whole list = 1 value)? Probe `e ← ((1){2})((0){3}) ⋄ (c f) ← ↑e[0]` standalone.

## Why it matters
structure.kap defines `when` and `unwindProtect` for the whole stdlib; until
this lands, every file that CALLS `when {…}` fails
(`unknown function: kap:when` / destructuring error), which is the single
largest source of the 136 startup warnings under the standard-lib autoload.

---

# OPEN-3 — `∙` inner product unregistered (`undefined symbol: ∙`)

```
port : 1 2 3 +∙× 1 2 3  →  error: undefined symbol: ∙
oracle: 1 2 3 +∙× 1 2 3  →  ⊢ 14
```
Kotlin anchor: `engine.kt` registers the inner-product operator (grep
`registerNativeOperator` for the bullet glyph); semantics = reduce of the
LEFT fn over the outer product built by the RIGHT fn (`+.×` = matrix product).
Needs TWO-GATE registration (`is_primitive_name` + `is_primitive_op`) plus an
evaluator arm that builds the outer product then reduces. math-kap.kap uses
`+∙×` (QR decomposition, lines 20–21).

Note: `.×` (ASCII dot) is NOT the syntax — the oracle rejects `+.` with
"Member dereference without argument". The glyph is U+2219 `∙`.

---

# OPEN-4 — `throw` / native-exception surface unimplemented

```
port : throw "x"  →  error: undefined symbol: throw
oracle: throw "x" →  Error at: 1:1: throw: x
```
Blocks:
- `stat.kap:20` `1≡≢⍴⍵ or throw "classify is only valid…"` (the whole
  `classify` export),
- `util.kap` filter's argument-validation branch,
- `map.kap` `int:throwNative` calls (same family).

Kotlin anchor: `throw` is a keyword-form (parser.kt processThrow /
ThrowException); `int:throwNative` wraps a native exception class symbol.
Open question: how much of the catch side (`catch`/`int:unwindProtect`)
exists already — `int:unwindProtect` IS implemented (structure.kap's
defsyntax expands to it); verify `catch` before scoping.

---

# OPEN-5 — map-type surface (map.kap): member-dereference `⍺.(⍵)` + `(@.≠)⍛⊂`

```
use("map.kap") → parse error at 13:6: unexpected token in primary
map.kap:13:   {⍺.(⍵)}/ m , (@.≠)⍛⊂ p
```
Two constructs, neither probed standalone yet (probe FIRST, don't assume):
1. `⍺.(⍵)` dynamic member dereference (Kotlin `MemberDeref`…) — also seen in
   the oracle's own error text "Member dereference without argument", so the
   construct exists upstream; check `parser.kt` for the `.` postfix rule on
   symbols.
2. `(@.≠)⍛⊂` — derived-op paren group as LEFT of reverse-compose. The P1
   compose work made `(fn)⍛fn` work generally; whether `@.≠` (bitwise-not
   derived op, itself P4 scope) parses inside the paren is unknown.
Per ROADMAP P7: consult the Kotlin map module before scoping; may be its own
mini-phase.

---

# OPEN-6 — startup autoload noise (cosmetic but masks real output)

Every REPL launch evaluates `use("standard-lib.kap")` (kap-cli/src/main.rs:73–76
unless `--no-standard-lib`), which currently emits **136 warning lines**
(OPEN-1..OPEN-5 cascading through the chain). Two consequences:
1. Probes must grep carefully or pass `--no-standard-lib`;
2. It hides NEW regressions inside the noise floor.

This ties into ROADMAP §P0.2 (`use()` abort-vs-tolerate policy — still
undecided): Kotlin aborts the included file at the FIRST failing statement;
the port warns-and-continues by design (`eval_string_in_env_tolerant`,
evaluator.rs:10156). Whichever policy wins, the stdlib chain should load
CLEAN once OPEN-1..OPEN-5 close; revisit the count after they do.

---

# Pre-existing, documented elsewhere (NOT re-opened here)
- `⟨⟩` vs `()` display glyphs and box frames — P8 renderer decision, value-
  exact everywhere; tracked in ROADMAP §P8.
- `util.kap:19–21` `@\s` regex literals — beyond-oracle scope, documented in
  KNOWN-NONCONFORMANCE.md.
- dfn `{…}` block interiors still parse via legacy `parse_block` (P1 known
  divergence; no current symptom).
