# CODE ANALYSIS 04 — OPEN-1…OPEN-6 (PROBLEM.md, 2026-08-25 revision)

**Session:** 04 (2026-08-26). **Role:** analysis only — nothing edited.
**Baseline verified:** HEAD `41d6ef5`, working tree clean (0 DBG prints in
parser.rs/evaluator.rs), gates green: lib **96 passed / 0 failed**, curated
parity **1/0** — matches PROBLEM.md's stated state.
**Method:** every claim re-probed live on the rebuilt port and/or the oracle;
Kotlin anchors read at the cited file:line. One unresolved detail is flagged
honestly in OPEN-1 rather than papered over.

---

## 0. Cross-cutting discovery — `when` / `unwindProtect` are NATIVE in current Kap

This reframes OPEN-2 and part of OPEN-5's context:

- Fresh-session oracle probes (no stdlib, no files): `when { (2>1){r←"a"} }`
  works outright, and a base-functions load exports `⊢ kap:when`.
- `unwindProtect { io:print "x" } { io:print "y" }` also works on the oracle
  without any `use` (prints `xy`, result `"x"`).
- Consequence demonstrated the hard way: instrumenting a *copy* of
  structure.kap to trace `when` internals on the oracle is a silent no-op —
  the native `when` shadows the stdlib def entirely. Several planned oracle
  probes of the macro body produced no output for exactly this reason.

Implications:
1. The parity target for when/unwindProtect is Kotlin's **native**
   implementation in engine.kt (grep `registerNativeFunction("when"` /
   unwindProtect), NOT structure.kap's defsyntax shims.
2. The macro-machinery bug OPEN-2 exposed is still real and still worth fixing
   — user-defined macros with destructuring loops hit it — but it is not on
   the critical path to oracle-parity `when`.
3. PROGRESS claims that `when` "works via the port's defsyntax expander" and
   oracle claims about stdlib-loaded when should be annotated accordingly.

---

## 1. OPEN-1 — adjacent symbol literals strand wrongly

### Confirmed repro (both engines)

```
port : 'a 'b 'c   →  c            (only last survives)
oracle: 'a 'b 'c  →  ⟨default:a default:b default:c⟩
port : 'a 1 2     →  (a 1 2)      ✓ value-stranding after one symbol works
```

### Two gate omissions located (static analysis)

1. **Kotlin-path accumulator has no QuotePrefix arm.**
   `parse_value_kotlin` (parser.rs:295–607) dispatches Number/Str/Char/
   APLNullSym/Symbol/OpenParen/OpenBrace/FnDefSym/LeftArrow… but there is NO
   `Token::QuotePrefix` arm. `'a` falls into the `_ =>` catch-all
   (parser.rs:590–595) which resets `self.pos = start` and bails to the legacy
   parser for the whole statement. Kotlin ground truth: parser.kt:1003
   `is QuotePrefix -> TokenParseResult.Instr(LiteralSymbol(nameToSymbol(...)))`
   — an ordinary VALUE instruction that accumulates like any operand.
2. **Legacy strand collector doesn't recognize symbol literals either.**
   `is_strand_operand` (parser.rs:2604–2626) lists Number/Char/Str/OpenParen/
   OpenBracket/APLNullSym + bare Symbols-that-aren't-fns; both
   `Token::QuotePrefix` and `Token::Literal(LiteralValue::SymbolValue{..})`
   are absent → the second `'b` breaks the strand loop at parser.rs:2362.
   The dyadic-loop operator classifier (parser.rs:2392–2408) also excludes
   them, so even if stranding worked, `('a 'b) f x` chains would mis-fire.

### Honest gap

Static trace predicts the FIRST literal surviving; the binary emits the LAST
(`→ c`). An additional consumption step exists somewhere between
parse_primary's QuotePrefix arm (parser.rs:4064–4090, looks clean) and the
strand loop that I did not pin down. **One targeted eprintln at three sites**
(kotlin-path `_ =>` fallback, is_strand_operand match arm, dyadic-loop
classifier) run against `'a 'b 'c` will settle it in minutes. Do that before
editing.

### Fix direction (for the editor)

- Add a QuotePrefix arm to `parse_value_kotlin`: consume `'`+Symbol, push
  `Instr::Literal(LiteralValue::SymbolValue{qualified-name})` into
  `left_args`, continue. This alone may fix the default path.
- Add `Token::Literal(LiteralValue::SymbolValue{..}) => true` and
  `Token::QuotePrefix => true` to `is_strand_operand` (and consider the
  dyadic classifier).
- Preserve qualified-literal support: `'kap:integer` must yield symbol
  value `kap:integer` (stat.kap:21 depends on it). parse_primary already
  builds the qualified name (parser.rs:4077–4083) — reuse that logic.
- Regression probes: `'a 'b 'c`, `'a 1 2`, `'kap:x ≡ typeof y`,
  stat.kap line 21 standalone, map.kap line 13 shape.

### Related NEW finding

`e ← ((1){2})((0){3})` fails on the port (`parse error at 1:12`) while
stranding paren groups generally works. Adjacent paren-groups whose contents
are dfns hit the same family of classifiers. Probe after the main fix.

---

## 2. OPEN-2 — destructuring inside defsyntax bodies = disclosure-depth divergence

### Confirmed minimal repro (exact when-shape, port-only failure)

defsyntaxsub wInner + `:repeat (entryList wInner)` + body
`(cond fn) ← ↑entryList[i]` → port: `error: destructuring assignment expected
2 values, got 1`. Same file runs clean on the oracle.

### Mechanism narrowed by probes

| probe (port) | result |
|---|---|
| `(c f) ← 1 2` | works |
| `(c f) ← ⊂(1 2)`-style explicit enclose | errors "expected 2 values, got 1" |
| `(c f) ← ↑(1 2)` | **errors identically** |
| `↑(1 2)` | returns `1` — monadic First DISCLOSES |
| `q ← ((5)(6 7)) ⋄ q[0]` | `(5 (6 7))` then `⍴ q[0] → ()` — index suffix also collapses one level |

So `↑entryList[i]` yields a SCALAR on the port; `.elements()` on a scalar
wraps to a 1-element vec (lib.rs:286), and DestructAssign (evaluator.rs:674–691)
then reports "got 1". Kotlin take-first semantics (skill:
take_first_semantics.md) keep the leading CELL intact for arrays-of-arrays —
the port discloses one level too far in the nested case.

### Kotlin anchor for what entryList actually is

`syntax/syntax.kt:168–179 RepeatSyntaxRule.processRule` collects each expansion
via `processCustomSyntax` into `MultiResultInstr(instructions)`
(syntax.kt:156–166), which evaluates to `APLArrayImpl(dimensionsOfSize(n),
valueList)` — a flat n-vector of per-entry results. Each wInner expansion
binds `cond thenStatement`, a 2-element strand, so each entry IS a 2-array;
`↑entryList[i]` under Kotlin take-first yields that 2-array intact and the
destructure sees 2 values. The port flattens/discloses instead.

Note: oracle-side verification of these internals is IMPOSSIBLE via edited
stdlib copies because native `when` shadows them (§0). Verify against Kotlin
native when source instead (engine.kt registration), or trust syntax.kt +
take-first docs as the spec.

### Fix direction

1. Align port take-First with Kotlin for vector-of-array cells: leading cell
   returned UNdisclosed (this is the documented rule; the nested case violates
   it). Gate with `references/take_first_semantics.md` case list plus new rows
   `p ← ↑((11)(22)) ; ⍴ p → ⟨⟩ vs (…)`.
2. Separately probe `↑entryList[i]` standalone — the port errored
   "Index list length must be less than or equal to rank" on a related form,
   suggesting index-suffix/rank interplay worth its own case.
3. After fixing, re-run the full when/unwindProtect acceptance matrix from
   PROGRESS-20260825.

---

## 3. OPEN-3 — `∙` inner product unregistered

Confirmed: port `undefined symbol: ∙`; oracle `1 2 3 +∙× 1 2 3 → 14`;
neither glyph nor ASCII `.×` exists (and `.×` is genuinely invalid — `.` is
MemberDereferenceToken, tokeniser.kt:61/:478).

### Kotlin anchors (outer_join.kt — ONE operator covers BOTH products)

- Registration: engine.kt:489 `registerNativeOperator("∙", OuterInnerJoinOp())`.
- Dispatch (:176–183): left fn is NullFunction ⇒ OUTER product
  `∘.f` equivalent; else INNER join. Implementing `∙` therefore delivers
  `A ∘.f B` almost free — check whether the ledger lists `∘.` separately first.
- Outer product (:186–215): result dims = concat(a_dims,b_dims); scalar sides
  disclosed once (aScalar/bScalar); lazy per-cell `fn.eval2Arg`; long/double
  specializations exist (skip for D1 port; generic path suffices).
- Inner join (:221–266):
  - normalize: scalar-or-1-el-vector A with non-scalar B ⇒ constant-broadcast
    along B's first axis, and vice versa (:232–241);
  - error when last axis of A ≠ first axis of B:
    `"Dimensions of A and B are incompatible. Dimensions of A: ${a}, Dimensions of B: ${b}"`
    with detail "The size of the last axis of A has to be the same as the
    first axis of B." (:244–251);
  - rank-1 × rank-1 fast path: `reduce(fn0, fn1.eval2Arg(A,B))` (:252–254);
  - higher rank: InnerJoinResult frame-of-cells (:256).

### Implementation breakdown (for the editor)

1. Two-gate register `∙` (+ alias scan: grep Kotlin for any other spellings —
   none found; do NOT add `.×`).
2. Parser: `f₁ ∙ f₂` binds two function operands (same shape as `-foo+` OpCall
   handling; the existing two-fn-operand machinery covers it — verify the
   bind path accepts a NON-adverb infix operator here).
3. Evaluator: NullFunction check needs a sentinel for "no left fn" — the port
   can model `f ∙ g` vs `∙ g` at parse level (build different Instr variants)
   rather than porting NullFunction.
4. Port the normalize/error/reduce ladder verbatim (texts above); add curated
   rows: `1 2 3 +∙× 1 2 3 → 14`, matrix product 2×2 case, outer `∘.×` case,
   dimension-mismatch error text.
5. math-kap.kap QR (lines 20–21) becomes the acceptance file.

---

## 4. OPEN-4 — throw/catch: native FUNCTION family, anchors found

Confirmed: port `throw` undefined; oracle `throw "x"` →
`Error at: 1:1: throw: x`.

### Anchors

- engine.kt:411 `registerNativeFunction("throw", ThrowFunction())` — it is a
  plain function, not keyword-form (PROBLEM.md guessed parser.kt processThrow;
  correct location is div_functions.kt).
- div_functions.kt:254–269 ThrowFunctionImpl:
  - monadic: throws TagCatch(ThrowableTag(symbol `error` in core namespace,
    data=msg));
  - DYADIC also exists: `tag throw data` → ThrowableTag(a, b).
- Catch side: div_functions.kt:270+ CatchFunctionDescriptor — `catch` is an
  OPERATOR: `fn catch handlers` where handlers is an even-length vector or a
  2-column matrix of (tag, lambda); tag matched via
  `compareEqualsTotalOrdering`; handler invoked as `handler(data, tag)`;
  invalid-shape error "Invalid dimensions of catch argument"; non-lambda
  handler error "The handler is not callable…".
- Port already has `int:unwindProtect` (evaluator.rs:1845+) and
  `int:throwNative` (evaluator.rs:1898+); adding monadic+dyadic `throw` and
  the `catch` operator completes the family.

Captured oracle probes (shape evidence): `({⍵+1} catch ("error" {"caught:",⍵})) 5`
evaluates (result `⍬` — handler not invoked since no throw);
`({throw "boom"} catch ("boom" {"caught"})) 0` → the throw surfaces as
`Error at: 1:3: throw: boom` — i.e. tag spelling/namespace in the handler row
must match the thrower EXACTLY; treat captured transcripts as authoritative
during implementation, not memory.

### Unblocking priority

stat.kap `classify` needs only MONADIC throw (`or throw "msg"`); util.kap
filter likewise. Ship monadic first, dyadic second, `catch` third.

---

## 5. OPEN-5 — map.kap: bigger than two syntax fixes (mini-phase recommended)

Confirmed: port dies at map.kap:13 (`⍺.(⍵)` member deref) with cascading
undefined-symbol warnings.

Anchors found:
- Dynamic member dereference: Kotlin parser.kt:905–922
  `processMemberDereference` — after `.` accept a Name (fast path,
  MemberDereferenceNameArgumentInstruction) or `(expr)` (full instruction);
  else ParseException "Invalid token after member dereference…". Token:
  MemberDereferenceToken (tokeniser.kt:61; emitted for `.` at :478). Also
  present: processMethodCall (:926+, `m.name(args)` style) — out of scope
  unless map.kap uses it (grep before scoping).
- BUT the decisive finding: **the port has ZERO native `map:` namespace**
  (no `map:` arms in evaluator.rs) while map.kap fundamentally requires
  `m map:with`, `m map:get`, and map-typed VALUES (`'kap:map ≡ typeof m`
  guards). And the oracle engine itself no longer exposes `map:new` /
  `map:from` — its bundled stdlib is again stale relative to the engine
  (same pattern as base-functions.kap/quad constants in session 03).

Verdict: hand-porting map.kap piecemeal is wasted motion. Scope a mini-phase:
native map value kind (or Rust-side opaque value), member-deref parse+eval,
`map:` builtin set (with/get/keys/…), THEN load map.kap. Consult the Kotlin
map module (types.kt MapValue + builtins) for exact semantics per ROADMAP P7.

---

## 6. OPEN-6 — autoload noise: 136 warnings confirmed

Live count: 136 `warning: use(): statement failed:` lines on a bare REPL
launch; visible cascade includes io.kap 21:23 and undefined `code` (the io.kap
line-4 chain from earlier sessions), i.e. several failures are DOWNSTREAM of
OPEN-1/OPEN-4 shapes, not independent.

Recommendations:
1. Short term: collapse per-statement spam into one summary line per file
   (`use(): N/M statements failed in X`) — preserves diagnostics, kills the
   noise floor that hides regressions.
2. Re-count after OPEN-1..OPEN-5 close; only then decide ROADMAP §P0.2
   abort-vs-tolerate against real numbers. (Session 03 established the ORACLE
   aborts the included file at the FIRST failing statement; the port's
   tolerate-mode remains a deliberate divergence until that decision.)

---

## 7. Recommended action order

| # | item | effort | unlocks |
|---|------|--------|---------|
| 1 | OPEN-1: run the 3-site trace probe, then add QuotePrefix arms (kotlin path + is_strand_operand) | small | stat.kap:21, map.kap:13 shapes, `'kap:*` literals everywhere |
| 2 | OPEN-3: implement `∙` inner+outer product per §3 ladder | medium | math-kap QR; free `∘.f` coverage |
| 3 | OPEN-4: monadic `throw` (dyadic next; `catch` last) | small→medium | classify, filter validation |
| 4 | OPEN-2: take-First disclosure fix + index-suffix probe | medium | user macros w/ destructuring loops; alignment with native-when semantics |
| 5 | OPEN-6: warning summarization | trivial | usable REPL output during all of the above |
| 6 | OPEN-5: map mini-phase (deref + map kind + `map:` builtins) | large | map.kap |

Gates after every step: `cargo test -p kap-core --lib`,
`cargo test -p kap-core --test conformance curated_kap_parity`; side-by-side
probe tables from captured output only (process law 0.1 #5/#8).

## Appendix — probe log (condensed, all fresh this session)

| # | input (engine) | result |
|---|----------------|--------|
| P1 | `'a 'b 'c` (port / oracle) | `c` / `⟨default:a default:b default:c⟩` |
| P2 | `'a 1 2` (both) | strands fine on port too |
| P3 | `e ← ((1){2})((0){3})` (port) | parse error at 1:12 (NEW gap) |
| P4 | `(c f) ← ↑e[0]` standalone (port) | destructure "got 1" family |
| P5 | `(c f) ← ↑(1 2)` (port / oracle) | port errors; oracle errors too but with DIFFERENT text ("expected a rank-1 array of 2, got dimensions: []") — port message text also diverges |
| P6 | `q ← ((5)(6 7)) ⋄ ⍴q[0]` (port / oracle) | `()` collapsed / `⟨1⟩` kept |
| P7 | when-body destructure repro file (port / oracle) | port: "expected 2 values, got 1"; oracle: clean |
| P8 | `1 2 3 +∙× 1 2 3` (port / oracle) | undefined symbol / `14` |
| P9 | `throw "x"` (port / oracle) | undefined symbol / `Error: throw: x` |
| P10 | catch forms (oracle) | evaluate; tag must match exactly incl. namespace |
| P11 | `m.a` member deref (port) | parse error at `.` |
| P12 | use(map.kap) (port / oracle) | port parse-error cascade; oracle loads (but its own map:new/map:from absent — stale stdlib again) |
| P13 | autoload warning count (port) | 136 |
| P14 | native when/unwindProtect (oracle, NO files) | both work; `⊢ kap:when` export seen |
| P15 | gates | lib 96/0, curated 1/0 |
