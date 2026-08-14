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

---
