# ARCHITECTURE — rust-kap port (implementor reference)

How the Rust port of Kap is currently put together. Read this before touching code.
Ground truth for *language behavior* is always the Kotlin source + `kap-jvm-text`
oracle (see `META-INF/ROADMAP.md` §0); this file describes only the *port's own*
structure. Verified against the tree at `b0a000d` (branch `feature/wheres-extra`).

## 1. Workspace layout

Three crates, `edition 2021` (root `Cargo.toml`). Dev profile is memory-capped
(`codegen-units=1`, `debug=1`, no incremental) because full debuginfo OOM-killed
the linker past 16 GB.

| Crate | Role | Entry points |
|---|---|---|
| `kap-core` | The language: value model, lexer, parser, evaluator, stdlib-loader | `Engine`, `Session`, `APLValue`, `Instr` |
| `kap-stdlib` | Vendored `.kap` sources + ordered file list | `STDLIB_FILES`, `STDLIB_DIR` |
| `kap-cli` | REPL + script-file runner | `kap` binary, `astprobe` debug binary |

`kap-core` is UI-agnostic (no terminal knowledge); `kap-cli` depends on it, never
the reverse. No `)`-commands anywhere (D4).

## 2. Pipeline: source → value

```
source &str
  → lexer::tokenise → Vec<SpannedToken>            (token.rs Token/LiteralValue)
  → parser::parse / Parser::parse_statements       (ast.rs Instr)
      → ONE statement at a time, see §5
  → Engine::eval_instr → AplRef<APLValue>          (lib.rs APLValue)
  → format_value / format_display / format_conform (lib.rs APLValue impl)
```

- **Lazy by default (D6):** the parser builds `Instr` trees only, never evaluates.
  Args stay unevaluated until a builtin forces them via `APLValue::force`
  (`Deferred { instr, env }`, call-by-need, not memoised). Eager/lazy is decided
  per-arg-position by the builtin in the evaluator.
- **Two eval modes:** Mode 1 stateless (`Engine::eval_string`, fresh env per call);
  Mode 2 stateful (`Session`, one `Rc<Environment>` survives across calls — the REPL).

## 3. Value model (`lib.rs`, `number.rs`, `array.rs`, `map.rs`)

- `pub type AplRef<T> = Rc<T>` — single-threaded `Rc` everywhere (D1). A future
  `Rc→Arc` migration is one line.
- `KapNumber` (`number.rs`, 1222 lines): `Long(i64) | Double(f64) | BigInt |
  Rational(BigRational) | Complex(f64,f64)`. Variants are never silently merged
  (`1.0` ≠ `1`); promotion follows `number.kt`. `num-bigint`/`num-rational` (D2).
- `APLValue` (`lib.rs:51`): `Number | Char | Str | Array | List | Null | Nil |
  Deferred | UserFn | UserOp | Escape | NonBoundFn | Symbol | Map`.
  - `Null` = `⍬`, a rank-1 empty array. `Nil` = the `null` keyword singleton. Not equal.
  - `List` (`;`-separated) ≠ `Array` (space-stranded); destructuring `(a;b;c)←`
    requires a `List` RHS.
  - `UserFn { params, split, body, env }`: closure; `split` = leading params bound
    to the *left* arg (monadic ⇒ `split==0`).
  - `Escape { target }` = captured `→`; `target` is the enclosing env id (`None` =
    no enclosing function → "Call to return without a function call").
- `KapArray` (`array.rs`, 297 lines): `{ dimensions: Vec<usize>, data: ArrayData,
  labels: Option<Box<DimensionLabels>> }`. `ArrayData`: `Long | Double | Char |
  BigInt | Rational | Nested(Vec<AplRef<APLValue>>)` — specialised buffers, immutable
  after construction. Per-axis labels: `DimensionLabels { labels:
  Vec<Option<Vec<AxisLabel>>> }`, `AxisLabel = Option<String>` (`None` = gap).
- `KapMap` (`map.rs`, 189 lines): insertion-ordered `Vec<(key, value)>`, linear
  scan (maps are small); key equality is value-equal *type-discriminating*.
- **Three renderers** (`lib.rs` `APLValue` impl): `format_value` (plain, `⍕`/internals,
  ASCII minus); `format_display` (REPL: quoted strings, `@c` chars, `⍬`);
  `format_conform` (`--conform-display`: `⟨⟩` vectors, box frames — oracle-compatible).

## 4. Errors (`lib.rs:1025`)

`AplError::{ Parse{line,col,msg}, Runtime(String), Return(value, Option<usize>) }`.
`Return` propagates up the env-parent chain until the frame whose env `id` matches
`target` (mirrors Kotlin `ReturnValue.returnEnvironment`); only `{}`-dfn bodies and
`eval_block` set `is_return_target`. Reaching top level with no catcher ⇒ runtime error.

## 5. Parser (`parser.rs`, 6398 lines)

Faithful port of Kotlin `parser.kt::parseExpr` single-pass accumulator loop (P1
migration complete; Kotlin path is the default).

- **Entry:** free fn `parse(tokens, known_functions, known_ops, macros)` for legacy
  callers; live path is `Parser::parse_statements` — **one statement per call**,
  advancing `pos` past a trailing `⋄`.
- **Core loop:** `parse_value_kotlin` / `parse_value_kotlin_with` accumulates
  `left_args`; hitting a function calls `finish_fn_call(fn, left_args)` —
  non-empty `left_args` ⇒ dyadic (`⍺=strand(left_args)`, single arg unwrapped),
  empty ⇒ monadic. `bind_operators_kotlin` folds postfix adverbs/axis/value-ops
  (`/ ¨ ⍨ ∵ ⌸ ⌻ ˝ ⍤ ⍢ ⍣`, `f[axis]`) onto the function atom per Kotlin `parseOperator`.
- **Struct helpers:** `parse_paren_accum` (paren groups, `ParenHolder` Value/Fn split
  = Kotlin `InstrParseResult`/`FnParseResult`); `parse_function_atom` (train members);
  `try_parse_train` (guarded lookahead, restores `pos`); `parse_primary`
  (literals, blocks, `λ`, `⍞`, `∇`, `defsyntax`); `parse_index_suffix`
  (postfix `a[i]` / `a.member`, binds to preceding primary only); `parse_apply`
  (legacy dyadic path, still used for right-arg parsing in places).
- **Valence disambiguation** (the hard part — three inputs consulted together):
  `is_primitive_op` (**gate 1**, `parser.rs:3325` — bare glyphs only; anything with
  `:` returns false), `known_functions` (seeded per-statement from
  `env.function_names()` — see §7), `known_ops` (from `env.operator_names()`),
  plus structural predicates `is_function_expr` / `is_fn_atom_name` /
  `next_is_function_token`. A bare symbol that is none of these strands as data.
- `Token` (`token.rs`, 91 lines) mirrors `tokeniser.kt`; dedicated variants exist
  for structural operators (`ComposeToken ∘`, `ReverseComposeToken ⍛`, `OverToken ⍥`,
  `LeftForkToken «`, `FunctionCallOpenParen ⟦`, `MemberDereferenceToken .`, …).
  `⍢`/`⍣`/`⍤` lex as plain `Symbol`s and are folded by `fold_value_ops_on_atom`.
- `Instr` (`ast.rs`, 314 lines, ~30 variants) mirrors `instr.kt` coarsely:
  `Apply | Assign | Array | ArrayWithShape` (evaluator-only round-trip, parser never
  emits) `| List | Lambda | NonBoundFn | Derived | Block | If | While | When |
  Train{funcs,reverse,compose} | UserFnDef | FnAssign | Guard | Value |
  DynamicRef(⍞) | UserOpDef | OpCall | InnerProduct(∙) | OverOp(⍥) |
  AxisApplied{f[axis]} | ValueOp{f⍤rank} | Index | IndexAssign | MemberDeref{value_form} |
  BooleanOp(and/or) | DefSyntax/DefSyntaxSub | DestructAssign | MacroExpand | Empty …`
- `astprobe` binary (`kap-cli/src/bin/astprobe.rs`): `astprobe 'expr'` dumps the
  `Instr` tree — the first tool for any "what did the parser build?" question.

## 6. Evaluator (`evaluator.rs`, ~18.7k lines — read in slices, never whole)

`impl Engine` rooted at `eval_string_in_env` (shared core): tokenise → loop
`parse_statements` + `eval_instr`, refreshing `known_functions`/`known_ops`/macros
from the live env before **every** statement (so `∇ foo… ⋄ foo 10` works).

Key dispatch (all under `eval_instr` → `eval_apply`):

| Area | Functions | Notes |
|---|---|---|
| apply | `eval_apply` (:1778), `apply_train` (:4522), `apply_user_fn` (:4763), `apply_user_op` (:5017) | train order: left-bind → atop → fork; `compose`/`reverse` (∘/⍛) override len-2 |
| scalar arith | `apply_op` (:5929), `num2` (:5939), `num2_impl` (:6020), `num2_axis` (:6270) | scalar+scalar short-circuits *before* axis (`2 +[0] 3 → 5`); `num2` recurses into nested cells |
| adverbs | `adverb_reduce` (:12234), `adverb_scan` (:12430), `adverb_each` (:14260), `adverb_inverse` (:8359, +`_with_proto` :9502), `adverb_bitwise` (:14084), `apply_under_op` (:13046, +`resolve_under_wrapper` :13527) | dispatch matches the **adverb name in `op`**, not `func`; `Derived{func,op}` node |
| misc ops | rank `⍤` (`apply_rank_op`), power `⍣` (`apply_power_op`), `∘.` inner/outer (`InnerProduct`), over `⍥` (`OverOp`) | `⍛` = `Train{reverse,compose}`; `∘` = `compose` only — both have **no** inverse |
| two-gate | `is_primitive_name` (**gate 2**, :4732) | every builtin in BOTH gates or it parses-but-never-dispatches (or vice versa) |

Stdlib/file loading: `eval_string_in_env_tolerant` (`use()` path) evaluates in a
per-file **anchor scope** (`acts_as_root`, `home_ns` updated per statement),
aborts the file at the first failing statement with one summary line
(oracle semantics, P0.2), restores the caller's namespace, and guards recursive
`use` via `include_stack`. `lib_paths` (+ CLI `--lib-path`) resolves basenames;
`Session::load_standard_lib` runs `use("standard-lib.kap")` unless
`--no-standard-lib`.

## 7. Scoping (`lib.rs:862`, `evaluator.rs:66`)

Two tables, different jobs:

- **Lexical** (`Environment.symbols`, keyed `(name, Option<ns>)`, parent-chained):
  dfn params `⍺`/`⍵`, block locals, operator operands. `←` updates the nearest
  existing binding (`assign`, set!-like); `declare(:local)` marks
  `unassigned_locals` (read-before-write ⇒ `Variable not assigned`).
- **Module** (`NamespaceRegistry`, shared `Rc`): `symbols` per namespace,
  `exports` (`declare(:export)`), `imports` (`import()`), `constants`
  (`declare(:const)` + native `kap:⎕A/⎕a/⎕d` and `default:null` seeded in
  `new_root`). `current` tracks the effective namespace; `use()`d files get a
  `home_ns` anchor so bodies resolve against their *defining* file.
- **Lookup order** (`Environment::lookup`): lexical chain → `ns:name` direct or
  bare via `resolve_bare` (current → imports → `default`) → home-ns anchor walk.
- **Parser seeding:** `function_names()` returns **only `⇐`-defined names**
  (`function_defs` set — a `←`-bound lambda is a *value* and must strand);
  `operator_names()` returns `UserOp` names (+ registry). Stale seeds ⇒
  misparsed valence; this is why the engine re-seeds per statement.

## 8. The other crates

- **`kap-cli`** (`main.rs`, 193 lines): arg parsing (`--lib-path`/`-p`,
  `--conform-display`, `--no-standard-lib`), stdlib preload (non-fatal warning),
  file mode (eval once, print `format_display`, exit 1 on error), REPL loop
  (backtick + unbalanced-bracket continuation, `>>> `/`··· ` prompts, Ctrl-D/Ctrl-C
  quit — host concern, no Kap quit primitive).
- **`kap-stdlib`** (`src/lib.rs`, 34 lines): `STDLIB_FILES` (19 files in
  `stdlib-files.kt` load order) + `STDLIB_DIR="std"`. Files are vendored Kotlin-
  upstream `.kap` run as-is; each loadable file is a regression test.
- **`encoder.rs`** (394 lines): Kap binary wire format (`0x99'kap'` header,
  dynint64, label flag) — port of `encoder/encoder.kt`. Standalone; not on the
  eval path.

## 9. Tests & conformance

- `cargo test -p kap-core --lib`: 96 unit tests (pure helpers). **Current state:
  91 passed / 5 failed** (`eval_iota`, `eval_format_directives`,
  `eval_parenthesised_groups`, `eval_reverse`, `eval_take_drop` — assertion
  mismatches, under investigation; do not claim green).
- `cargo test -p kap-core --test conformance`: `conformance.rs` (1144 lines)
  drives `conformance/kotlin_tests.jsonl` (extracted Kotlin cases) with
  per-case `catch_unwind` + timeout; outcomes `Ok | Mismatch | Unsupported`.
  The broad sweep is `#[ignore]`d (can hang on incomplete builtins); the live
  gates are `curated_kap_parity` (hand-ported, must pass) and the lib tests.
- 13 focused parity files (`labels_parity`, `member_deref_parity`,
  `encode_decode_parity`, `reshape_spec_keywords`, `monadic_axis_arith`,
  `sort_axis`, `null_propagation`, `return_escape`, …) — per-feature oracle
  comparisons; several are skeletons with placeholder expectations (check before
  trusting).
- Debug aids: `KAP_DEBUG_UNDER=1` eprintln tracing in under/overlay paths;
  `run_conformance_tests.sh`, `conformance_summary.txt`, `tools/` extractor
  scripts at repo root.

## 10. Where to look things up

- Language behavior question → Kotlin `~/Apps/array/array/...` + oracle binary
  (never this file, never the port's current behavior).
- "What did X parse as?" → `astprobe 'X'`; then `ast.rs` variant docs.
- New builtin → both gates (`is_primitive_op` + `is_primitive_name`) + axis
  allowlist if `f[axis]`; evaluator arm; curated row from captured oracle output.
- Valence/stranding bug → §5 gate trio + per-statement seeding in
  `eval_string_in_env`; `next_is_paren_operator`-style lookahead restores `pos`.
- Name-resolution bug → §7 lookup order; check `home_ns` anchor for `use()`d files.
- Process rules (gates-before-commit, PROGRESS-before-continuing, stale-binary
  first) → `META-INF/ROADMAP.md` §0.1. Session history → `META-INF/PROGRESS-*.md`.
