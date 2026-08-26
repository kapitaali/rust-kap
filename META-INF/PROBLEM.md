# PROBLEM.md — OPEN problems (rewritten 2026-08-27 at `cd2b6bf`)

Every item below was **re-probed fresh on 2026-08-27** against the port and the
oracle. Nothing here is carried over on trust. Probe form used throughout:

```bash
# port
printf 'EXPR\n' | ./target/debug/kap --no-standard-lib 2>&1 | grep -aE '>>> .|error|parse'
# oracle
printf 'EXPR\n' | ~/Apps/array/kap-jvm-text/bin/kap-jvm-text \
  --lib-path=$HOME/Apps/array/kap-jvm-text/standard-lib 2>/dev/null | grep -aE '⊢ .|Error'
```

State at time of writing: gates GREEN (lib **96/0**, curated **1/0**); working tree
clean; `main == strings == feature/wheres-extra == origin/*` == `cd2b6bf`.

## CLOSED since the previous revision (2026-08-25) — do NOT reopen

Confirmed by fresh probe, not by memory:

| old item | probe | port | oracle |
|---|---|---|---|
| OPEN-1 adjacent symbol literals | `'a 'b 'c` | `(a b c)` | `⟨default:a default:b default:c⟩` |
| OPEN-2 destructuring in defsyntax | `use("structure.kap")` | CLEAN | — |
| OPEN-3 `∙` inner product | `1 2 3 +∙× 1 2 3` | `14` | `14` |
| OPEN-4 `throw` | `throw "x"` | `error: throw: x` | `Error at: 1:1: throw: x` |
| OPEN-6 autoload noise | startup warnings | **6** (was 136) | — |

OPEN-1 and OPEN-4 differ ONLY in display glyphs / error prefix (`⟨⟩` vs `()`,
`default:` qualification, `Error at: L:C:`) — those are the tracked P8 renderer
items, not value defects. `util.kap`, `stat.kap`, `structure.kap`, `io.kap`,
`standard-lib.kap`, `thread.kap` all load **CLEAN**.

---

# OPEN-A — `output3.kap:118`: axis-applied fn inside a fork tine (TWO gaps)

**Highest-value item.** This is the last blocker on `output3.kap`, which improved
90/123 → **53/88** failures at `cd2b6bf` but still stops at the same line.

```kap
kap-stdlib/std/output3.kap:118
    ((⌈arrayMaxWidth[0]÷⍺)↑[¯1+≢⍴⍵])«,»((-⌈arrayMaxWidth[1]÷⍺)↑[¯1+≢⍴⍵]) ⍵
```

## Symptom — two SEPARATE defects, both isolated

```
A1 (parse)      ⌽«,»(2↑[0]) ⍳6
                port : parse error at 1:11: unexpected token in primary
                oracle: ⟨5 4 3 2 1 0 0 1⟩

A2 (evaluator)  (2↑[0])«,»((-2)↑[0]) ⍳6
                port : error: unsupported axis operator: ↑
                oracle: ⟨0 1 4 5⟩
```

## Crucially — the constituents ALREADY WORK

This is what makes it a combination bug, not missing axis support:

```
(2↑[0]) 1 2 3      → port (1 2)   oracle ⟨1 2⟩   ✓
((-2)↑[0]) ⍳6      → port (4 5)   oracle ⟨4 5⟩   ✓
((1+1)↑[0]) ⍳6     → port (0 1)   oracle ⟨0 1⟩   ✓
2↑[¯1+1] 1 2 3     → port (1 2)   oracle ⟨1 2⟩   ✓
(2↑)«,»((-2)↑) ⍳6  → port (0 1 4 5) oracle ⟨0 1 4 5⟩ ✓  (fixed in cd2b6bf)
```

So: axis-applied take works standalone; forks with left-bound tines work; **only
axis + fork-tine together fails.**

## Where to look

- **A1** is a parse gap. Every fork site parses its members with
  `parse_function_atom` (`parser.rs` fork arms at ~:715, ~:783, ~:2173, ~:2287,
  ~:3350). The `f[axis]` → `Instr::AxisApplied` wrap lives on a different path
  (`parser.rs` ~:884 allowlist) and is evidently not reached from the tine route.
  Apply the session's own lesson: **position-dependent failure ⇒ diff the ROUTES**,
  and TRACE which branch the tine takes before editing.
- **A2** is an evaluator gap: `unsupported axis operator: ↑` comes from the
  axis-operator dispatch (`evaluator.rs` ~:1122 `AxisApplied` arm region). The
  two-gate pattern for axis support applies (`parser.rs` allowlist ~:884 AND the
  evaluator dispatch arm) — see the skill's note that a function missing from the
  allowlist STRANDS the axis and returns a silently wrong value.

## Open question

Does the fork-tine route build an `AxisApplied` node at all (then A2 is the only
real gap and A1 is a missing token in a guard list), or does it never reach the axis
wrap? **Dump the built instr at the fork member site first** — do not edit either
guard speculatively. Three speculative parser edits were already burned in this area
this session (`PROGRESS-20260826i.md`).

---

# OPEN-B — `(n↑)`: a VARIABLE as a left-bind value

```
n ← 2 ⋄ (n↑) ⍳6
port : error: unknown function: n
oracle: ⟨0 1⟩
```

Deliberate consequence of `6395485`. `is_value` (evaluator.rs) was widened to
`Literal|Array|Empty|Apply|Index|BooleanOp|Value` but **`Symbol` was intentionally
EXCLUDED**, because a bare symbol in a train is normally a FUNCTION reference and
admitting it breaks plain 2-trains `(f g)`.

So this cannot be fixed by widening `is_value` further — the parser must decide,
at parse time, whether the symbol names a value or a function (Kotlin resolves this
via the environment / `known_functions` seeding). Likely parser-side, in the same
family as the `function_names()`/`operator_names()` sync rule already documented in
the skill.

---

# OPEN-C — `map.kap:13`: member-deref lambda + `@.≠`

```
use("map.kap") → parse error at 13:6  (18/24 statements fail)
kap-stdlib/std/map.kap:13:   {⍺.(⍵)}/ m , (@.≠)⍛⊂ p
```

Both constituents probed standalone:

```
{⍺.(⍵)}        port: parse error at 1:3: unexpected token in primary
               oracle: Error at: 1:1: No arguments specified for function
                       (i.e. the oracle PARSES it, then objects to zero args)

(@.≠)          port: error: ≠ requires numbers
               oracle: Error at: 1:2: No arguments specified for function

@.≠ 1          port: error: ≠ requires numbers      oracle: 1
(@.≠)⍛⊂ 1 2 3  port: error: ≠ requires numbers      oracle: ⟨⟨1 2 3⟩⟩
```

Two distinct problems:
1. **`⍺.(⍵)` member dereference** — the port cannot parse the `.` postfix on `⍺`
   (fails at col 3, i.e. at the `.`). The oracle parses it fine. Kotlin: the
   MemberDereferenceToken `.` postfix rule in `parser.kt`.
2. **`@.` is a CHAR LITERAL, not a dereference.** `@.` is the character `.`, so
   `@.≠` is "char `.` left-bound to `≠`" — a left-bind of a CHAR. The port's
   `≠ requires numbers` shows it is comparing numerically instead of accepting a
   char operand. Note `cmp2_elements` was already fixed for char-vs-char (see
   PROGRESS 2026-08-26); this path evidently does not reach it. **Verify which
   compare path `@.≠ 1` takes before editing** — the operand here is char-vs-NUMBER,
   which the oracle answers `1` (not equal), so the fix is "chars compare unequal to
   numbers", not "chars compare as codepoints".

---

# OPEN-D — structural under: dyadic + pick-based

```
3 {⍵}⍢⌽ 1 2 3
port : error: under not supported for function
oracle: ⟨1 2 3⟩
```

The OVERLAY family for monadic `↑`/`↓` wrappers landed in `2426014`. Still missing:
- **Dyadic under** — Kotlin `evalWithStructuralUnder2Arg` +
  `inversibleStructuralUnder2Arg` via `evalInverse2ArgB` (functions.kt:273);
  overlay 2-arg at drop.kt:96 / :360.
- **Pick-based under** — lookup.kt:232. `overlay_replacement` (evaluator.rs) is
  already written and reusable for it.

Relevant port sites: `apply_under_op` (~:7859 region), `overlay_replacement`,
`under_take_drop_spec`.

---

# OPEN-E — `typeof "hi"` type name

```
typeof "hi"
port : kap:string
oracle: kap:array
```

A Kap string IS a char array, so the oracle reports `kap:array`. The port has a
distinct `Str` representation and names it `kap:string`. Pre-existing and unrelated
to the `≡`/namespace work (`'kap:array ≡ typeof 1 2 3` → `1` in BOTH). Fixing it may
mean either renaming the class for `Str` or unifying the representation — the latter
is a large change; scope before touching.

---

# OPEN-F — `fhelp.kap`: `⟦` unimplemented

```
fhelp.kap → 3/7 statements failed: error: undefined symbol: ⟦
port : ⟦  →  error: undefined symbol: ⟦
oracle: ⟦  →  Error at: 1:1: Unexpected token: FunctionCallOpenParen
```

Note the oracle ALSO rejects a bare `⟦` (it is not a standalone value) — so the
probe above does not prove the glyph is unimplemented, only that bare use fails in
both. `⟦` is a function-call bracket form upstream. **Scope this by reading the
Kotlin tokeniser for `⟦` and by finding fhelp.kap's actual usage line**, not from
the bare-glyph probe.

---

# OPEN-G — `http.kap:21` and `graph.kap:43`

```
http.kap  → 7/14 failed @ 21:23
http.kap:21:   (code;data;headers) ← get url      ← multi-target destructure with ';'

graph.kap → 12/29 failed @ 43:24
graph.kap:43:  matrixToList ⇐ (⍸¨ ⊂[1])           ← axis-applied ⊂ inside a paren group
```

`graph.kap:43` looks like a sibling of **OPEN-A** (axis-applied function inside a
paren/derived group) and may fall out of that fix — retest it immediately after
OPEN-A lands before scoping separately.

Both files were previously logged out-of-scope per ROADMAP §10 (networking /
graphing). Keep them out of scope for correctness work, but the two parse errors
above are ordinary parser gaps worth noting.

---

# Pre-existing, tracked elsewhere (NOT re-opened here)

- **P8 renderer:** `⟨⟩` vs `()`, `default:`-qualified symbol display, box frames for
  rank≥2, and the `Error at: L:C:` error prefix. Value-exact everywhere; ROADMAP §P8.
- **`math:pi`** — `math.kap` 1/9 `Assignment to constant variable: math:pi` is
  **ORACLE-CONSISTENT** (oracle errors identically). Do NOT "fix" it.
- **Nested-vector representation gap** — the port generalises
  `((1 2 3)(4 5 6)(7 8 9))` into a true `(3 3)` array where Real Kap keeps it rank-1;
  documented in KNOWN-NONCONFORMANCE.md.
- **dfn `{…}` block interiors** still parse via legacy `parse_block` (P1 known
  divergence; no current symptom).
- **`run_kotlin_conformance`** stays `#[ignore]`d — it HANGS on incomplete builtins.
  The real gate is `curated_kap_parity`.

# Debug instrumentation left in tree

**None.** Verified: `grep -rn 'KAP_TRACE_FA\|KAP_TRACE_PAREN' kap-core/src/` returns
nothing; `git status --short` clean at `cd2b6bf`.
