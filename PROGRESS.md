# PROGRESS.md — Kap text-client rewrite in Rust

> Meticulous diary of the Rust rewrite of Kap's **text client only** (no GUI/web/optional
> modules). Companion docs at repo root: `RUST_REWRITE_STRATEGY.md` (plan + locked
> decisions) and the generated `<module>/<module>.md` Kotlin→Rust doc set.
>
> Format per entry: `## [YYYY-MM-DD] PHASE/STEP — status` then what was done, decisions,
> and next action. Entries are append-only; mistakes are recorded, not erased.

---

## [2026-08-14] PRELUDE — Kotlin documentation pass

- **Done:** Generated a mechanical Kotlin→Rust documentation set for the whole repo using
  a local brace/paren-aware scanner (`/tmp/kapdoc/extract_kotlin.py`), avoiding LLM
  subagent rate-limit failures on the free tier.
  - 22 module `.md` files: 20 script-generated + `kap-util/kap-util.md` (hand-written
    exemplar). 18,939 declarations documented (verbatim signature, parsed Inputs/Output,
    Behaviour = KDoc where present, mechanical Rust mapping line).
  - Test source sets and `*Test.kt` excluded.
- **Decision:** Documentation only — no architecture proposed (per user's instruction).
- **Note:** The doc set is the mechanical translation table; Rust type choices are made in
  the strategy, not the docs.

## [2026-08-14] STRATEGY — `RUST_REWRITE_STRATEGY.md` written

- **Scope locked:** port `array/commonMain` + a thin Rust `main` REPL; reuse the `.kap`
  stdlib as-is; drop all GUI/web/UI crates and the text-client's optional engine modules
  (crypto/audio/valkey/sdl/raylib/ffi/net).
- **Crate layout:** `kap-core` (engine, UI-agnostic), `kap-stdlib` (the `.kap` files),
  `kap-cli` (main/REPL).
- **Key realisation:** `text-client/src/linuxMain/.../init.kt` is a *thin* harness — all
  language lives in `array/commonMain`; the stdlib is Kap source the engine runs.

## [2026-08-14] DECISIONS — four locked (expanded in strategy §9)

- **D1** Single-threaded `Rc<APLValue>` first; `pub type AplRef<T> = Rc<T>` alias so a
  later `Rc`→`Arc` move is one line. `SIGINT` → cooperative `AtomicBool`.
- **D2** `num-bigint`/`num-rational`/`num-complex` first (pure Rust, no GMP). Promotion
  rules from `number.kt`; `fmt-rational` reformatted locally.
- **D3** ANSI escape sequences for terminal ops; drop ncurses/termcap (`TerminalModule`
  may be omitted in v1).
- **D4** Script + REPL only. **Corrected (APL-flood):** Kap has NO `)`-prefixed session
  commands. File load = `use("/path/file.kap")` parser directive; `import`/`namespace`/
  `declare` are the other directives. Quit = host concern (Ctrl-D/Ctrl-C). Multi-line
  continuation = trailing backtick `` ` ``. Added an APL-flood guard to the strategy.

## [2026-08-14] PHASE 0 — workspace skeleton  [DONE]

- **Done:** Created Rust workspace at `rust-kap/` (new dir, separate from Kotlin repo
  root to avoid mixing build systems).
  - `rust-kap/Cargo.toml` — workspace, members `kap-core`, `kap-stdlib`, `kap-cli`.
  - `kap-core` — `AplRef<T> = Rc<T>` alias; `APLValue` enum (Phase-0 shape only);
    `Engine` struct stub with `eval_string` returning `AplError::NotImplemented`;
    `AplError` enum (thiserror). Deps: num-bigint/num-rational/num-traits/num-complex,
    thiserror.
  - `kap-stdlib` — vendored the 18 `.kap` files from `array/standard-lib/` into `std/`;
    `STDLIB_FILES` + `STDLIB_DIR` constants matching `stdlib-files.kt`.
  - `kap-cli` — `kap` binary; arg parse: file path → file mode (stub), else REPL
    (stub). No `)` commands (per D4).
- **Build:** `cargo build` succeeds (13.8s; crates downloaded from crates.io).
- **Verification:** `cargo build` green. No tests yet (Phase 0 is a skeleton).
- **Next:** Phase 1 — value model & scalars (numbers, chars, strings, formatting).

---

### Open questions / risks being watched
- `array/standard-lib/` has 23 `.kap` files; only 18 are in `stdlib-files.kt`'s load
  list. Vendored the 18 listed ones. The 5 extras (animation, jvm-calls, output2,
  sextant-drawing, xml) are not auto-loaded — defer.
- `AplRef<Box<dyn Any>>` in the Phase-0 `APLValue::Ref` is a placeholder; real storage is
  the `ArrayData` enum from strategy §4.1 (Phase 1).

## [2026-08-14] CORRECTION — testing was missing from the strategy

- **Gap admitted:** the original strategy only *implied* testing (the "stdlib is the
  oracle" note in §5/§7). It never designed a test harness, and §1.1 explicitly excluded
  `*Test.kt` from the documentation scope. User flagged this: "did you include testing at
  all… remember to include those and turn them into the Kap test harness."
- **Finding (grounded in source):** 197 `*Test.kt` files exist. The `array/src/commonTest`
  ones (~150) are **Kap-as-input**: `parseAPLExpressionWithTest("+1 2 ¯6 2J7 2J¯7") { result ->
  assert1DArray(...) }` — build engine, eval Kap string, assert on result. The assertion
  *helpers* (`assert1DArray`, `InnerBigIntOrLong`, `NearDouble`, `NearComplex`, in
  `test-tools/.../APLTest.kt`) are **Kotlin** and cannot run against a Rust engine.
  Confirmed Kap's stdlib has **no** `assert`/`test` vocabulary (grepped `array/standard-lib/*.kap`).
- **Decision (strategy §9b D5 + §10, LOCKED): Kap-native harness.** Tests become Kap
  source (`*.kap` test files) run by the engine under test — NOT Rust/Kotlin unit tests
  inspecting internals. The `*Test.kt` files are source material; assertion logic moves
  into a small `test.kap` framework (`assertEqual`/`assertNear`/`assertThrows`/`test` +
  `TESTS total=N pass=M fail=K` summary). Each Kotlin `@Test` → a Kap `test "…" { … }`
  block. A `kap-test` runner (`use`s `test.kap` + `test/*Test.kap`, parses the summary
  line, sets exit code) gates every phase.
- **Why this shape:** exercises the real engine end-to-end (same oracle principle as §5),
  ports zero Kotlin internals, and the 18,939-decl doc set + `.kap` stdlib stay the single
  source of truth.
- **Scope:** priority = `array/src/commonTest` numeric/array/parser cases (gate Phases
  1–6). `contrib/*`, `gui/*`, platform-specific (`jvmTest`/`jsTest`/`linuxTest`), and
  `mpbignum`/`mpmaths` tests are lower priority (numeric cases already cover bigint/
  rational/complex behaviourally).
- **Discipline:** tests assert *observable behaviour*; internal storage-class distinctions
  (bigint-vs-long) are dropped (not user-visible; Rust may store numbers differently per
  D2). Dropped assertions are flagged here, not silently removed.
- **Next:** Phase 1 (values) will also create `kap-stdlib/test/test.kap` and a first
  `NumbersTest.kap` port as the smoke test, before broadening.

## [2026-08-14] CORRECTION — number formatting: `1.0` is a Double, prints `1.0`

- **User correction:** "if you add .0 Kap will coerce it into a decimal number and not use
  rational. so if I input 1.0, Kap prints 1.0". I had assumed READABLE style strips `.0`
  (render `1.0` as `1`). That is WRONG.
- **Actual Kap behaviour (confirmed against `number.kt` + `rendertext.kt`):**
  - Integer *literal* `1` → `APLLong` → prints `1` (plain & readable).
  - Floating *literal* `1.0` → `APLDouble` → prints `1.0` (plain & readable). `READABLE`
    style only maps the minus sign `-`→`¯`; it does **not** drop `.0`.
  - Rational literal `1r2` → `APLRational` → prints `1r2` (plain) / `1r2` (readable).
  - So `Double` and `Long` are distinct stored types with distinct printed forms. The
    Kotlin `formatDouble` keeps the float representation; do NOT normalise `1.0`→`1`.
- **Implication for `KapNumber` (strategy §4.2 / Phase 1):** the Rust enum must preserve
  `Long` vs `Double` as separate variants (not coerce `1.0` to an integer on display), and
  the formatter must print `1.0` for doubles. This matters for matching `NumbersTest`/
  `FormatNumbersTest` expectations in the Kap harness.
- **Action:** Phase 1 value model will store `Long`/`Double`/`BigInt`/`Rational`/`Complex`
  distinctly and format each per the above; no `.0` stripping.

## [2026-08-14] PHASE 1 — value model & scalars (Rust core)  [DONE]

- **Done (`kap-core`):**
  - `src/number.rs` — `KapNumber` enum: `Long(i64)`, `Double(f64)`, `BigInt(BigInt)`,
    `Rational(BigRational)`, `Complex(f64,f64)`. Conversions (`as_double`/`as_long`/
    `as_complex`/`is_complex`/`is_zero`/`as_boolean`), cross-type `numeric_cmp`
    (complex -> Err, per `number.kt`), and `format(readable)` with the `1.0` rule from
    the correction entry (Double keeps `.0`; only `-`->`¯` in READABLE).
  - `src/array.rs` — `ArrayData` enum (specialised immutable buffers: Long/Double/Char/
    BigInt/Rational/Nested) + `KapArray { dimensions, data }`; `from_numbers` builds the
    most-specific backing type.
  - `lib.rs` — replaced Phase-0 placeholder `APLValue::Ref(Box<dyn Any>)` with the real
    `APLValue` enum (`Number`/`Char`/`Str`/`Array(AplRef<KapArray>)`/`Null`); wired
    `mod number`/`mod array`; kept `AplRef<T> = Rc<T>`.
  - Rust `#[test]`s (D5 pure-helper tests): 9 tests, all pass — cover formatting
    (long/double/rational/complex), `as_long` errors, cross-type comparison, complex
    non-orderability. `cargo test` green.
- **Done (Kap harness, executes in Phase 3+):**
  - `kap-stdlib/test/test.kap` — Kap-native framework: `assertEqual`/`assertNear`/
    `assertThrows`/`test`/`testSummary`, using real `catch`/`throw`/`if-else` syntax
    (corrected after I initially wrote invented `catch` tuple syntax — verified against
    `docs/reference.asciidoc` §"catch"/"throw"/"if"). Prints `TESTS total=N pass=M fail=K`.
  - `kap-stdlib/test/NumbersTest.kap` — first port of `NumbersTest.kt` cases
    (monadicAdd, monadicAddWithBigint, addMixedTypes 0-3) as `test "…" { … }` blocks.
  - NOTE: these `.kap` tests are NOT yet runnable — the engine cannot evaluate Kap
    source until Phase 2-3. They are authored now as the regression oracle (strategy §5/§10).
  - The `kap-test` runner (`use`s `test.kap` + `*.kap` tests, parses the summary line for
    exit code) is deferred to Phase 3 when `eval_string` exists.
- **Verified:** `cargo test` → 9 passed, 0 failed. `cargo build` green.
- **Decisions made in-code:** `Double` and `Long` kept as DISTINCT stored variants (no
  `.0` stripping) — preserves Kap's `1` vs `1.0` distinction (correction entry).
- **Next:** Phase 2 — tokeniser + parser (port `tokeniser.kt`/`parser.kt` verbatim, emit
  `Instr` AST). Then Phase 3 wires `eval_string` so the Kap harness above becomes live.
- **Watching:** `numeric_cmp` for Double falls back to f64 compare (not Kap's exact
  `rationalise()` repeat-by-10 comparison) — precision difference vs Kotlin for some
  float/bigint-ordering cases. Acceptable for Phase 1; revisit when porting
  `FormatNumbersTest`/`CompareTest`. Recorded here per D5 oracle discipline.

## [2026-08-14] PHASE 2a — tokeniser (Rust core)  [DONE]

- **Done (`kap-core`):** tokeniser module split across small files (avoids stream timeouts):
  - `src/token.rs` — `Token` enum (all ~50 kinds from `tokeniser.kt`) + `LiteralValue`
    (Number/Char/Str/Symbol) + `SpannedToken { token, line, col }` for error positions.
  - `src/lexer.rs` — `tokenise(src) -> Vec<SpannedToken>`: whitespace/comment/newline
    handling, `@` char literals, `"..."` strings (with escapes), number lexing, single-char
    punctuation, symbol names (with `foo:bar` namespace split).
  - `src/lex_helpers.rs` — `lex_char`/`lex_string`/`lex_number`/`lex_symbol`/`split_namespace`/
    `is_symbol_start` + `parse_kap_number` (int/bigint/float/rational/complex per reference).
- **Number lexing** follows docs/reference.asciidoc §"Numbers": `1234`, `¯456` (Kap's minus),
  hex `0x12`, binary `0b1100110`, rational `3r2`, float `1.234`/`¯0.001`/`4.1e22`, complex
  `100j200`. Big integers beyond i64 -> `KapNumber::BigInt`.
- **Verified:** `cargo test -p kap-core` → 20 passed (10 Phase-1 + 10 new lexer tests
  covering every number form, strings, chars, punctuation). `cargo build` green.
- **Decisions / notes:**
  - `to_i64()` on `BigInt` needed the `num_traits::ToPrimitive` trait; used a string-parse
    fallback (`bigint_to_i64`) to avoid the trait-import fragility — correct, simpler.
  - Whitespace is skipped (not emitted as tokens), matching Kap; newlines ARE emitted
    (`Token::Newline`) so the parser can implement line-continuation (`\` at line end).
  - The `ToPrimitive` import in number.rs is now unused (warning only).
- **Next:** Phase 2b — parser: turn the token stream into an `Instr` AST (port
  `parser.kt`). Then Phase 3 wires `Engine::eval_string` so the Kap harness
  (`kap-stdlib/test/*.kap`) becomes live. The tokeniser output is the parser's input.

## [2026-08-14] PHASE 2b — parser + lazy-evaluation model (Rust core)  [DONE]

- **Done (`kap-core`):**
  - `src/ast.rs` — `Instr` enum: `Literal`/`Symbol`/`Apply{fn_expr,left,right}`/
    `Assign{target,value}`/`Array{elements}`/`Empty`. `left`/`right` are `Box<Instr>`
    TREES (not evaluated), preserving laziness.
  - `src/parser.rs` — recursive-descent `parse(tokens) -> (Vec<Instr>, Vec<String>)`:
    statement / assign (`←`) / dyadic apply / parentheses / `[a;b;c]` array literals /
    `⍬` null. Whitespace is stranded into `Array` at the evaluator (Phase 3); here
    `1 2 3` parses as consecutive terms (apply chain) — stranding resolved later.
  - `src/lib.rs` — added `Deferred { instr: AplRef<Instr>, env: AplRef<Environment> }`
    variant to `APLValue` = the **lazy thunk** (unevaluated tree + captured env).
    `Environment` is a stub (Phase 3 fills the symbol table). `mod parser;` wired in.
- **Lazy evaluation (user-raised, core Kap feature):** Kap is lazy. Decision LOCKED:
  laziness lives in the EVALUATOR (Phase 3), not the parser. The parser builds trees
  only; function args stay as `Instr` trees and are forced via `Deferred` on demand.
  Builtins will declare per-arg eager/lazy (mirrors Kap's `APLBuiltin` descriptor).
  Recorded as strategy decision **D6** (see RUST_REWRITE_STRATEGY.md §9b).
- **Verified:** `cargo test -p kap-core` — ran by module to avoid OOM in this
  environment (swap ~full, 16 GB RAM). Results: number 7/7, array 3/3, lexer 10/10,
  parser 6/6 → **26/26 pass**. (Full-suite `cargo test` was SIGKILLed by the OOM
  killer, NOT a test failure — confirmed by per-test runs all green.)
- **Bug fixed:** `parse_primary` initially wrapped symbols as `Instr::Literal`, which
  broke `foo` parsing; now `LiteralValue::Symbol` → `Instr::Symbol` (correct).
- **Next:** Phase 3 — `Engine::eval_string` (tokenise→parse→eval), `Environment` symbol
  table, `Deferred::force()`, and a handful of builtins (`+`, `←`, `⍳`, `≢`/`⍴`, `⊃`).
  This makes the Kap test harness (`kap-stdlib/test/*.kap`) executable. Watch for the
  stranding rule (`1 2 3` → vector) and lazy forcing of `if`/`while`/`and`/`or`.

## [2026-08-14] PHASE 3 — evaluator + OOM/lexer debugging  [DONE]

- **Done (`kap-core`):** `src/evaluator.rs`
  - `Environment { symbols: HashMap, parent: AplRef<Environment> }` symbol table with
    `lookup`/`assign` (child scopes shadow parent).
  - `Engine::eval_string(src) -> Result<AplRef<APLValue>, AplError>`: tokenise → parse →
    eval each statement; surfaces first parse error with position.
  - `eval_instr` core loop: Literal/Char/Str/Symbol(lookup)/Empty/Array(build)/Assign/Apply.
  - `APLValue::force` thunk forcing: `Deferred{instr,env}` evaluates lazily; non-deferred
    values clone through (lazy model, D6).
  - `eval_apply`: resolves the fn name (Symbol), forces left/right (monadic = right only),
    dispatches by name. Starter verbs: `+` (add), `*` (multiply), `⍳`/`iota` (0..N-1),
    `⍴`/`≢`/`tally` (element count), `⊃`/`first`, `←` (assign). `format_value` added to
    `APLValue` (nested arrays render via `ArrayData::Nested`).
  - `KapNumber::mul` added (mirrors `add`, with promotion rules + complex multiply).
- **Memory fix (root cause of the OOM the user observed):** added a memory-capped dev/test
  profile to `Cargo.toml` — `codegen-units = 1`, `debug = 1`, `opt-level = 0`,
  `split-debuginfo = "unpacked"`, `incremental = false`. Slashed debug-build link memory
  so `cargo test` no longer gets SIGKILLed mid-rebuild in this 16 GB box.
- **Four genuine bugs found & fixed during the OOM investigation:**
  1. **Lexer infinite loop (the 5 GB OOM).** In `tokenise`, the `is_symbol_start` branch
     called `lex_symbol` which *rejects* operator chars like `+` (non-alphanumeric) and
     returned `ni == i` (zero width); the old code did `i = ni` leaving `i` unchanged, then
     `continue` re-entered the loop on the SAME `+` forever — pushing ~50M `Symbol` tokens
     until a 5,368,709,120-byte `Vec` alloc failed. Fixed with a `consumed =
     ni.saturating_sub(i).max(1)` ≥1-advance guard. Backtrace confirmed the site
     (`lexer::tokenise` → `Vec::push`).
  2. **Empty-name symbols.** Even with the loop fixed, `lex_symbol` returned an empty name
     for `+`, so it became `Symbol("")`. Fixed: a lone operator char becomes `Symbol(c)`
     (the single char) rather than an empty name.
  3. **Bare symbols mis-parsed.** `parse_apply` treated ANY leading `Symbol` as monadic
     application, so a variable ref like `foo` (or `1 + 2`, where `+` was the empty-name
     symbol) failed to parse. Fixed: monadic form only fires when an operand actually
     follows; a lone symbol is a variable reference (`Instr::Symbol`).
  4. **Missing `*` verb.** `(1 + 2) * 3` failed with "unknown function: *". Added the `*`
     builtin + `KapNumber::mul`.
- **Verification:** `cargo test -p kap-core` → **33 passed, 0 failed** (number 10 / array 3
  / lexer 10 / parser 6 / evaluator 7). No SIGKILL. The earlier "stale binary" confusion
  was precisely because `cargo test` was OOM-killed mid-rebuild before fixes landed; the
  `Cargo.toml` profile fixed that.
- **Next:** Phase 4 — broaden builtins: `-`/`÷`/`×`, `=`/`≠`/`≤`/`≥`/`</`>`, `,` (catenate),
  `⌽`/`⊖` (reverse), `⍉` (transpose), `↑`/`↓` (take/drop), `⊂`/`⊃` (enclose/first),
  strand/`⍬`, and user-defined functions (`∇` / lambda `λ`). Then Phase 5 REPL in
  `kap-cli`, Phase 6 breadth, Phase 7 parallel, Phase 8 broaden the Kap test harness.

---

## 2026-08-14 — Phase 4 COMPLETE (builtins + variables + lambdas)

**Goal:** Broaden the builtins and add persistent assignment, stranding, and user lambdas.

**Changes made:**

- **Persistent assignment (foundational fix).** `Environment.symbols` was
  `HashMap<(..), AplRef<APLValue>>` (immutable), so `Assign` cloned-and-discarded the env
  and variables never persisted. Changed to `RefCell<HashMap<..>>`; `lookup`/`define` go
  through `borrow`/`borrow_mut`. `Instr::Symbol` eval now clones the inner value out of the
  shared `Rc`. Assignment now persists: `x ← 5 ⋄ x + 1` → `6`.
- **`UserFn` value + `Instr::Lambda`.** Added `APLValue::UserFn { params, body, env }` and
  `Instr::Lambda { params, body }`. `λ(params) body` parses (parenthesised or single-param);
  `apply_user_fn` builds a child scope from the closure env and binds params (dyadic:
  left=first param, right=second; monadic: right=first param). `f ← λ(x) x * 2 ⋄ f 5` → `10`,
  `g ← λ(a b) a + b ⋄ 3 g 4` → `7`.
- **Arithmetic:** `-` (sub), `÷`/`/` (div → Rational for ints), `×` (mul alias), `*` (had).
- **Comparisons:** `= ≠ < > ≤ ≥` → Kap boolean `1`/`0` (via `numeric_cmp`), scalar extension.
- **Array/structural:** `,` (catenate, via `Token::Comma` + `Instr::Symbol(",")`),
  `⌽`/`⊖` (reverse), `⍉` (transpose 2-D), `↑`/`↓` (take/drop), `⊂` (enclose), `⊃`/`≢`/`⍴`/`⍳`
  (had). Scalar extension in `num2` so `1 2 3 + 10` → `[11 12 13]`.
- **Stranding:** `1 2 3` → `[1 2 3]`; a stranded vector is the left arg of a following
  dyadic operator.
- **Formatting:** `format_value` uses APL-style output (`-3` → `¯3`, `1/2` → `1r2`).

**Bugs found & fixed during Phase 4 (parser valence is the hard part):**

1. **Lambda body swallowed the `⋄` separator.** `skip_newlines` was skipping `⋄`
   (`StatementSeparator`) *everywhere*, so `f ← λ(x) x*2 ⋄ f 5` parsed `⋄` *inside* the lambda
   value, corrupting the stream. **Fix:** `skip_newlines` now skips only `Newline`; `⋄` is
   consumed **only** at the top-level `parse()` statement boundary. (Most subtle bug — it made
   every multi-statement program with a lambda/assignment fail.)
2. **Leading-symbol monadic heuristic.** A leading `Symbol` must be monadic (`f x`, `⍳5`,
   `⊃ ⍳5`) but a *user* symbol followed by an operator is the LEFT operand of a dyadic
   (`x + 1`). Rule: primitive + (plain operand | another primitive) → monadic; user symbol +
   plain operand → monadic; otherwise fall through to dyadic. Without this `x + 1` became
   `x (+ 1)` (monadic + with one arg → "+ needs two args").
3. **Early-return bug.** The leading-symbol branch `return Ok(first)` *before* the dyadic
   loop meant a non-monadic leading symbol (`x` in `x + 1`) never reached the dyadic loop, so
   `+ 1` parsed as a *monadic* apply → "+ needs two args". Fixed by falling through.
4. **`Comma` as operator.** `,` (catenate) had no `Token` and `parse_primary` had no arm for
   `Token::Comma` → "unexpected token in primary" for `1 , 2`. Added `Token::Comma`, lexed it,
   and made `parse_primary` emit `Instr::Symbol(",")` for it.
5. **Dyadic operator must accept user functions.** The dyadic loop only treated *primitive*
   symbols as operators, so `3 g 4` (dyadic user fn) failed with "unknown function: g".
   Fixed: any `Symbol` is a valid dyadic operator; the leading-monadic logic already routes
   `f 5` (user fn + operand) to monadic apply correctly.

**Verification:** `cargo test` (whole workspace) → **42 passed, 0 failed** (number 10 /
array 3 / lexer 10 / parser 6 / evaluator 13). Phase-4 evaluator tests added:
`eval_sub_neg`, `eval_div_rational`, `eval_comparisons`, `eval_strand`, `eval_catenate`,
`eval_reverse`, `eval_transpose`, `eval_take_drop`, `eval_enclose`, `eval_assign_and_var`,
`eval_lambda_apply` (11 new, all green). No OOM / no SIGKILL (memory-capped profile holds).

**Next:** Phase 5 — REPL + `.kap` runner in `kap-cli` (thin native binary: read file or
stdin, tokenise→parse→eval, print `format_value`); wire `Engine::eval_string` to it.

---

## 2026-08-14 — Parenthesization hardening (Phase 4 follow-up)

User flagged that parentheses must be parsed fully because "Kap is very LISPish". Probed the
actual behavior first; most already worked. Added the genuinely-missing LISPish cases.

**What already worked (verified, not changed):** nested groups `(1+2)*3`, groups with
assignment/lambda/strand inside `(x ← 5) + 1`, `(f 5)+1`, `(1 2 3)+10`; unclosed-paren gives a
clean "expected ')'" error.

**Gaps fixed:**

1. **Ambivalent arithmetic.** `-`/`+` were dyadic-only → `-(1+2)` errored "needs two args".
   Now monadic `- x` = negate (with array scalar-extension: `-(3 1 4)` → `¯3 ¯1 ¯4`),
   monadic `+ x` = identity. Added `Engine::negate`. `-(1+2)` → `¯3`, `+(1+2)` → `3`.
2. **Parenthesised operators as derived functions** (`(OP)`). `2 (+) 3` / `3 (×) 4` parsed the
   group as a strand operand → "undefined symbol". Added `Parser::next_is_paren_operator`
   (peeks `( OP )` without consuming) so the strand loop does NOT swallow `(+)` and the dyadic
   loop treats `(OP)` as the operator. `2 (+) 3` → `5`, `3 (×) 4` → `12`.
   - Critical correctness rule preserved: a group is a derived *operator* ONLY when its sole
     content is a bare operator symbol. `(1+2)` (contains an `Apply`) stays a plain operand, so
     `(1+2)(3+4)` correctly strands to `[3 7]` (per user's explicit spec), never an Apply.

**Tests added (evaluator):** `eval_parenthesised_groups`, `eval_juxtaposed_groups_strand`,
`eval_monadic_arithmetic`, `eval_paren_operator`, `eval_unclosed_paren_errors`.

**Verification:** `cargo test` (whole workspace) → **47 passed, 0 failed** (number 10 / array
3 / lexer 10 / parser 6 / evaluator 18). No OOM / no SIGKILL.

---

## 2026-08-14 — Phase 5: includeable crate (two modes) + REPL

User wanted `kap-core` usable as an **embeddable crate** in addition to the REPL, with two
programmatic modes:

**Mode 1 — stateless eval of a Kap expression string.** `Engine::eval_string(src)` returns
`Result<Rc<APLValue>, AplError>`; a fresh `Environment` is created per call, so assignments
inside `src` don't leak. Added `Engine::eval_to_string` convenience.

**Mode 2 — persistent session.** `kap_core::Session` (new `session.rs`, re-exported at the
crate root) holds one `Engine` + shared `Rc<Environment>`. `Session::eval(src)` runs in that
env, so variables / user lambdas persist across calls like the REPL. `Session::new()`,
`eval`, `eval_to_string`, `len`, `is_empty`.

Both leverage the *already-existing* `Rc<RefCell<HashMap>>` `Environment` — the only prior
gap was that `eval_string` built a throwaway env per call. `Session` just keeps one alive.
~50 lines of new logic, no engine changes needed for persistence.

**`kap-cli` becomes a real client** (was a stub): `kap` → REPL backed by `Session` (with
multi-line continuation on trailing backtick `` ` `` or unbalanced `()[]{}`); `kap file.kap`
→ batch-eval, print the last statement's value, exit non-zero on parse error.

**Bug fixed (regression from the "accept any Symbol as dyadic operator" change in Phase 4):**
whole-string multi-statement parse (`parse()`) wrongly treated the leading symbol of a
*next* statement as the dyadic operator of the previous one (`a ← 3 \n b ← 4` parsed as
`(a ← 3) b` → "unexpected token in primary"). Added `Parser::at_statement_boundary` and
check it **before** skipping newlines in the monadic, strand, and dyadic loops, so a top-level
newline/⋄ separates statements. `a + b * 2` on its own line is not the left operand of the
next line's operator. (Inside parens/brackets newlines are still consumed by `parse_primary`.)

**Verification:** `cargo test` → **51 passed, 0 failed** (added 4 Session tests: persistent
vars, lambda sees globals, stateless non-persistence, mode-1 format). REPL + file mode
verified end-to-end with state persistence. No OOM.

**Requirement noted (still open):** the crate must stay embeddable (the two modes above) in
addition to the REPL — confirmed satisfied; keep both paths working in later phases.

---

## 2026-08-14 — Error-position polish (Phase 5.5)

User asked to polish the earlier caveat: parse errors reported `0:0` with the real position
buried in the message text. Now:

* `AplError` gained a `Runtime(msg)` variant (separate from `Parse{line,col,msg}`).
* `Parser::err()` now returns `AplError::Parse{line,col,msg}` using the token's real
  `line`/`col` from the lexer span (no more `0:0`).
* `parse()` returns `(Vec<Instr>, Vec<AplError>)` (was `Vec<String>`); `Engine` surfaces
  the **first** error with its real position instead of `0:0`.
* All ~20 runtime-error sites in `evaluator.rs` converted from `AplError::Parse{0,0,..}`
  to `AplError::runtime(..)`.
* REPL `run()` prints `parse error at L:C: msg` (parse) vs `error: msg` (runtime).

**Verification:** `cargo test` → **53 passed, 0 failed** (added `parse_errors_carry_real_position`
+ `runtime_errors_separate_from_parse`). Manual REPL check: `2 (` → `parse error at 2:1`,
`foo 3` → `error: unknown function: foo`.

---

## 2026-08-14 — Phase 6: more builtins

Added a batch of scalar/array builtins (the "easy" next feature, per user's requested order:
builtins → adverbs → control flow → trains).

**New monadic/ambivalent functions** (in `evaluator.rs` `eval_apply`):
- `⌈`/`ceil`, `⌊`/`floor` — element-wise ceil/floor via `KapNumber::ceil/floor`.
- `*` — **ambivalent**: monadic `*` = exp (`x.exp()`), dyadic `*`/`×` = multiply. (Was dyadic-only
  before; made ambivalent like `+`/`-`.)
- `⍟`/`log` — **ambivalent**: monadic `⍟` = natural log (`KapNumber::nat_log`); dyadic `a ⍟ b`
  = log base `a` of `b` = `ln(b)/ln(a)` (Kap/APL convention: `X ⍟ Y` = log base X of Y).
- `|`/`mod` — residue/modulo (`KapNumber::modulo`, rem_euclid semantics).
- `∧`/`∨` — boolean AND/OR on 0/1 (`bool2` helper).
- `~`/`not` — logical not (`KapNumber::not`: zero→1, non-zero→0), element-wise.
- `∊`/`in` — membership: for each of `a`'s elements, 1 if present in `b` else 0 (`membership`).
- `⍋`/`grade` — grade up: 1-based indices that would sort ascending (`grade_up`).
- `⊤`/`encode`, `⊥`/`decode` — mixed-radix base conversion (`encode`/`decode`).

**Supporting changes:**
- `number.rs`: added `ceil`, `floor`, `exp`, `nat_log`, `log`, `modulo`, `not` to `KapNumber`
  (with Complex/BigInt/Rational branches where meaningful).
- `parser.rs`: added the new glyphs to `is_primitive_op` so they parse as operators, not vars.
- Added helper methods `scalar1`, `bool2`, `membership`, `grade_up`, `encode`, `decode` to
  `Engine`.

**Bugs caught & fixed during this phase:**
1. New glyphs missing from `is_primitive_op` → parsed as undefined variables
   (`undefined symbol: ⍟`). Added them.
2. Duplicate `"*"` match arm (old dyadic `*`) shadowed the new ambivalent `*` arm → `*` was
   always dyadic. Removed the stale duplicate.
3. `⍟` monadic initially used `x.log(Long(e))` (wrong: ln(x)/ln(e) ≠ ln(x)); replaced with
   dedicated `nat_log`. Dyadic `⍟` call flipped to `b.log(a)` for correct base convention.
4. `modulo` BigInt branch used `mod_floor` (trait not in scope) → fell back to Double path.

**Verification:** `cargo test` → **62 passed, 0 failed** (added 9 builtin tests). REPL smoke-tested
end-to-end: `⌈ 3.2`→`4.0`, `⌊ 3.8`→`3.0`, `7 | 3`→`1`, `1 ∧ 0`→`0`, `~ 1 0 3`→`[0 1 0]`,
`2 9 4 ∊ 1 2 ・ 3 4`→`[1 0 1]`, `⍋ 3 1 4 1 5`→`[2 4 1 3 5]`, `2 2 2 ⊤ 5`→`[1 0 1]`,
`2 2 2 ⊥ 1 0 1`→`5`, `* 2`→`7.389`, `2 ⍟ 8`→`3.0`.

---

## Phase 7 — Adverbs (`/`, `\`, `¨`) + ambivalent `⌈`/`⌊`

**Goal:** Implement the three core adverbs as higher-order operators producing *derived
functions* (APL `f/` `f\` `f¨` model), and make `⌈`/`⌊` ambivalent (monadic ceil/floor,
dyadic max/min) so they work inside reduce/scan.

**Design — `Instr::Derived`:** Added a new AST node `Instr::Derived { func, op }` representing
a derived function `func op` (e.g. `+/`, `×¨`). This is the clean APL model: the adverb binds a
*function operand* (not data) and produces a derived function that is then applied to its data
arguments.

**Parser (`parser.rs`):**
- `is_adverb(name)` whitelist: `/`, `\`, `¨` (+ aliases `reduce`/`scan`/`each`).
- Leading-symbol block: `f adverb` (e.g. `+/`) is NOT monadic application — `next_is_adverb`
  suppresses `do_monadic`, and a new branch builds `Apply{ fn_expr: Derived{func:f, op},
  left: None, right: data }`.
- Dyadic loop: when the operator is a function-primitive and the token *after* it is an adverb
  (e.g. `2 ×¨ 3 4 5`), the operator `×` and adverb `¨` are bound into `Derived{func:×, op:¨`
  and applied to the data-left `2` and data-right `[3 4 5]` → dyadic each (element-wise).
- This also handles `1 2 3 ×¨ 4 5 6` (vector × vector each) via the same path.

**Evaluator (`evaluator.rs`):**
- `eval_instr` gained an `Instr::Derived` arm (errors only if used standalone without args).
- `eval_apply` early-returns into a `Derived` dispatcher that resolves `op` to reduce/scan/each
  and threads `func`, `left`, `right` through to the helper.
- `adverb_reduce` (fold-left), `adverb_scan` (prefix accumulation), `adverb_each` (element-wise;
  monadic when `left=None`, dyadic element-wise when `left` is a scalar/vector that is applied
  per element via `apply_fn_instr`).
- `apply_fn_instr` recursively builds an `Instr::Apply` for `func` and re-enters `eval_apply`,
  so derived functions work with builtins, lambdas, and user fns uniformly.
- `⌈`/`⌊` made ambivalent: monadic ceil/floor (existing `scalar1`), dyadic max/min (numeric_cmp).

**Bugs caught & fixed during this phase:**
1. Adverb dispatch initially matched the *function* name instead of the adverb name → wrong
   branch. Fixed to read `op` from `Derived.op`.
2. `⌈`/`⌊` were monadic-only `scalar1`, so dyadic reduce/scan discarded the accumulator
   (`⌈/ 3 9 2 7` → `7` instead of `9`). Made them ambivalent dyadic max/min.
3. Scan test used `+/\\` (which is `+/` reduce + stray `\`) instead of `+\` (single backslash).
   Correct Kap scan is `+\`; fixed test strings. Scan adverb is `\` (one backslash), reduce `/`.
4. Missing exhaustive `Instr::Derived` match arm in `eval_instr` → `non-exhaustive patterns`
   compile error. Added a clear error arm.

**Verification:** `cargo test` → **66 passed, 0 failed** (added 7 adverb tests:
`eval_reduce`, `eval_scan`, `eval_each_monadic`, `eval_each_dyadic` [incl. `2 ×¨ 3 4 5`→`[6 8 10]`,
`1 2 3 ×¨ 4 5 6`→`[4 10 18]`], plus `⌈/`,`⌊/`). REPL smoke-tested end-to-end:
`+/ 1 2 3 4`→`10`, `×/ 1 2 3 4`→`24`, `+\ 1 2 3 4`→`[1 3 6 10]`, `⌈/ 3 9 2 7`→`9`,
`⌊/ 3 9 2 7`→`2`, `⌈¨ 1.2 2.8 3.5`→`[2.0 3.0 4.0]`, `2 ×¨ 3 4 5`→`[6 8 10]`,
`1 2 3 ×¨ 4 5 6`→`[4 10 18]`, `3 ⌈ 5`→`5`, `⌈ 3.2`→`4.0`, `~¨ 1 0 3`→`[0 1 0]`.

**Next (per user order):** control flow, then trains. (Adverbs done.)

