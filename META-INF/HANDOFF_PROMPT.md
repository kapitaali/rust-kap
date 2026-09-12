Good morning. We will resume our rewriting of the Kap array language in Rust. Files at ~/Apps/array/rust-kap; load skills `rust-kap-dev`, `kap-lang`, `array-language-impl` (skill_view) before coding. Read ../RUST_REWRITE_STRATEGY.md, the renewed ARCHITECTURE file, the three last PROGRESS files and ROADMAP.

Do not use background processes. Only foreground, every run timeout <=590s.

**Project:** Rust rewrite of Kap at `~/Apps/array/rust-kap` (binary `kap-cli` → `./target/debug/kap`). Source-of-truth Kotlin at `~/Apps/array/array` — read, never run.

PROGRESS file is `META-INF/PROGRESS-YYYYMMDD.md` (today: PROGRESS-20260912.md, fully documented through end of day). Invariant: keep `main == strings == origin/*` after commits (`git branch -f strings main && git push origin main strings`).

**Gates (run BOTH, real gates):**
- `cargo test -p kap-core --lib` → must be 113 passed, 0 failed (old "92" notes are stale).
- `cargo test -p kap-core --test conformance curated_kap_parity` → must be `1 passed; 0 failed`. IGNORE any FAILED-by-design `run_kotlin_conformance` panic note (it passes now).
- After ANY parser/evaluator edit: `cargo build -p kap-cli` (NOT just `-p kap-core`) before probing — stale-binary trap. If still stale: `touch kap-core/src/*.rs && cargo build -p kap-cli`.

## Where we stand (end of 2026-09-12)

**Sweep: 2936 total : 2884 OK / 0 MISMATCH / 52 UNSUPPORTED = 98.2%.** MISMATCH column is empty for the first time. Branch `feature/wheres-extra`, HEAD `6485415` + today's uncommitted work (DynAssign `dynamicequal`, `,`-str splice, `:fill` reshape, bare-glyph `⌹` aliasing — all in tree, gates green).

## Next: the `⌹` cluster (StandardLibMath, 5 rows) — highest value

`⌹` now resolves (bare single-char glyphs reach `kap:⌹` via core-namespace interning). What remains is a real engine gap inside the stdlib's own `∇ (A) ⌹ (B)` + helpers `norm`/`QR`/`Rinv` (standard-lib/math-kap.kap:6/10/26/41):
- `⌹ 5 5 ⍴ ...` and `⌹3 3⍴...` → `÷: arrays of different length` (scalar-vs-vector agreement bug, likely Rinv/QR)
- `9.0 8.0 7.0 ⌹ 2` → `Infinity` (expect 12.0); `9 8 7 ⌹ 2.0` → 12.0 already ✓
- `(4 4⍴...)⌹...` → STACK OVERFLOW (unbounded recursion in same chain)
- Do NOT enable the StandardLibMath stdlib preload until `⌹` terminates (conformance.rs:197-208 — row 998 overflows the harness worker).

Then: ReduceTest 4 / ComposeTest 4 (untriaged), OutputFormatter 4 (P8 box renderer), `defer` 3 + `?` QuestionMark arm (move before `_` fallback ~parser.rs:1268; `deal()` at evaluator.rs:22258 works standalone).

Key files: `kap-core/src/evaluator.rs` (reshape_proto, collect_elements_splice_str, Dynamic force), `kap-core/src/parser.rs` (is_known_fn bare-glyph arm, DynAssign intercept ~1091), `kap-core/tests/conformance.rs:197-208` (preload guard).
