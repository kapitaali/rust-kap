# Getting Started with rust-kap

> **rust-kap** is a Rust rewrite of the **Kap** array language — compatible with the Kotlin `array` reference at `~/Apps/array/array` (2987/0/0 = 100% conformance). The `kap` binary is a text REPL and file runner; it can also speak the **RIDE** binary-framed protocol to the [`stride`](https://github.com/kapitaali/stride) TUI editor.

If you only want to try Kap, you need 3 commands. If you want to extend it or plug it into stride, this file plus `docs/ride.md` and `docs/extensions.md` cover everything copy-paste.

---

## 1. Prerequisites

```bash
rustc --version   # stable, edition 2021 (1.75+ is fine)
cargo --version
git --version
```

Optional — only if you touch JVM primitives (`JvmInstanceValue`) or run the full conformance sweep that needs a JDK:

```bash
export JAVA_HOME=/home/theb/Apps/jdk-current   # JDK 25, javac + libjvm.so
java -version
```

No other tooling is required. The base `kap` binary builds with no JVM.

---

## 2. Clone and build

```bash
git clone https://github.com/kapitaali/rust-kap
cd rust-kap

# debug build — fast, line-tables-only (default dev profile is memory-capped)
cargo build -p kap-cli

# release build — faster interpreter (≈2×), needed for the full 2987-row sweep
cargo build --release -p kap-cli

# binaries
ls -lh target/debug/kap target/release/kap
./target/debug/kap --help 2>&1 | head   # no --help yet; it just starts the REPL
```

For convenience the repo ships a symlink (created by setup):

```bash
ln -sf target/debug/kap ./kap
./kap --help
```

> The workspace is `kap-core` (engine) + `kap-stdlib` (Kap files) + `kap-cli` (binary) + `kap-derive` (proc-macro) + `kap-ext-stats` (example extension). `cargo build` from the root builds all of them.

---

## 3. Run Kap

### 3.1 REPL

```bash
./target/debug/kap                  # REPL
./target/debug/kap --no-standard-lib  # without stdlib (bare engine)
```

```
Kap 0.0.0 — REPL (Ctrl-D to exit)
>>> 1+1
2
>>> 1 2 3 + 4 5 6
(5 7 9)
>>> ⎕A
"ABCDEFGHIJKLMNOPQRSTUVWXYZ"
>>> stats:mean 1 2 3 4
5/2
```

* Variables and defined functions persist across lines (`Session` — see `kap-core/src/session.rs`).
* Multi-line: a line ending in `` ` `` continues, as does an unbalanced `(` `[` `{`. Or press Enter on an empty line to execute the buffer.
* Quit: `Ctrl-D` or `Ctrl-C`.

### 3.2 File mode

```bash
./target/debug/kap file.kap
./target/debug/kap --no-standard-lib --lib-path=/path/to/standard-lib file.kap
```

Reads `file.kap`, evaluates it once, prints the last value with `format_display` (strings quoted, `⍬` for empty). Errors exit 1 and print to stderr.

### 3.3 CLI flags

| Flag | Effect |
|------|--------|
| `--lib-path=PATH` / `-p PATH` | Add a standard-library search dir for `use(...)` (repeatable; tried in order). Mirrors `kap-jvm-text --lib-path`. |
| `--no-standard-lib` | Don't auto-load `use("standard-lib.kap")` at startup. Bare engine — `⎕A`/`when`/`split` are not defined. |
| `--conform-display` | Use oracle-compatible display (`⟨⟩` and `┌→──┐` box frames) instead of house-style `()` — for manual comparison with the Kotlin oracle. |
| `--ride` | RIDE client: connect to a RIDE gateway (stride/RIDE editor) via `RIDE_INIT=CONNECT:host:port`. See `docs/ride.md`. |
| `--serve [port]` | RIDE server: listen on `127.0.0.1:port` (default `4502`) and speak RIDE. For manual testing. |

All flags are parsed before the positional file argument; `--lib-path`/`--conform-display`/`--no-standard-lib` affect both REPL/file **and** RIDE sessions (per-connection `Session`).

---

## 4. Standard library

If `--no-standard-lib` is not given, the session evaluates

```
use("standard-lib.kap")
```

at startup — the same as Kotlin's `repl-builder.kt:loadStdLibrary`. Failures are non-fatal and reported as `warning: standard library failed to load: ...` on stderr. The library lives in `kap-stdlib/` and in the sibling `~/Apps/array/array/standard-lib/`; `--lib-path` lets `use(...)` find it when running from a different CWD.

---

## 5. Test and conformance

```bash
# unit + integration (114 lib + curated + ext)
cargo test -p kap-core --lib
cargo test -p kap-cli
cargo test -p kap-ext-stats

# full 2987-row Kotlin oracle sweep — MUST be --release (debug has a 10s recv_timeout phantom)
export JAVA_HOME=/home/theb/Apps/jdk-current
cargo test --release -p kap-core --test conformance run_kotlin_conformance -- --nocapture
# → 2987 ok / 0 mismatch / 0 unsupported = 100.0%  (conformance_summary.txt)

# curated Kap parity (hand-written oracle checks)
cargo test --release -p kap-core --test conformance curated_kap_parity -- --nocapture
```

The sweep extracts cases via `tools/extract_kotlin_tests.py ~/Apps/array → conformance/kotlin_tests.jsonl` and compares `Session::eval` against the JVM oracle `~/Apps/array/kap-jvm-text/bin/kap-jvm-text`.

---

## 6. Extensions — add a Rust primitive

One crate = one domain. Example `kap-ext-stats` gives `stats:mean`/`median`/`stdev`/`contains` as real Kap names.

**Quick start** (10 minutes):

```bash
cargo new --lib kap-ext-my
# add to Cargo.toml workspace members, then:
```

```toml
# kap-ext-my/Cargo.toml
[dependencies]
kap-core = { path = "../kap-core" }
inventory = "0.3"
```

```rust
// kap-ext-my/src/lib.rs
use kap_core::native::{Args, NativeContext, NativeFn, NativeReg};
use kap_core::native::helpers::{bad_arg, ok};
use kap_core::{APLValue, AplError, AplRef};
use std::rc::Rc;

#[derive(Debug, Default)] pub struct MyHello;
impl NativeFn for MyHello {
    fn name(&self) -> &str { "my:hello" }
    fn doc(&self) -> &str { "my:hello y — greet y" }
    fn call(&self, _ctx: &NativeContext, args: Args) -> Result<AplRef<APLValue>, AplError> {
        let v = args.mono_named("my:hello")?;
        let s = match v.as_ref() { APLValue::Str(s) => s.clone(), APLValue::Char(c) => c.to_string(), _ => return Err(bad_arg("my:hello","requires a string")) };
        ok(APLValue::Str(format!("hello, {s}")))
    }
}
inventory::submit! { NativeReg { name: "my:hello", factory: || Box::new(MyHello) } }
```

```toml
# kap-cli/Cargo.toml
kap-ext-my = { path = "../kap-ext-my" }
```
```rust
// kap-cli/src/main.rs
#[allow(unused_imports)] use kap_ext_my as _; // keep inventory statics linked
```

```bash
cargo build -p kap-cli
printf 'my:hello "world"\n' | ./target/debug/kap --no-standard-lib
# → "hello, world"
KAP_DEBUG_NATIVES=1 ./target/debug/kap --no-standard-lib <<< 'my:hello "x"'
# KAP natives: ["stats:mean", ..., "my:hello"]
```

Full guide with `RankSpec::Scalar`, error handling, and the `#[kap_fn]` macro: **`docs/extensions.md`** (482 lines, beginner-friendly) and the RFC **`docs/native-api-design.html`**.

---

## 7. RIDE with stride — Kap inside a TUI editor

Kap speaks the **RIDE binary-framed protocol** (`[4B BE len][4B "RIDE"][JSON]`) — same as Dyalog RIDE and `rust-apl`. Stride is the gateway server (listens on 4502); Kap is the client (`--ride`). See **`docs/ride.md`** for the full setup, including the `editor.toml` fix and manual `RIDE_INIT` mode.

**Minimal auto-connect** (stride spawns Kap):

```bash
# 1. Point stride at Kap (one-time)
cat ~/Apps/stride/editor.toml
# gateway_executable = "/home/theb/Apps/array/rust-kap/target/debug/kap"
# gateway_args = "--ride"
# gateway_env = ["RIDE_INIT=CONNECT:127.0.0.1:4502"]

# 2. Build both
cargo build -p kap-cli && cargo build -p stride

# 3. Start stride — it listens on 4502 and spawns Kap automatically
cd ~/Apps/stride && cargo run
# inside stride: type `1+1` → Kap evaluates → stride shows `2`
```

No manual `kap --ride` needed in this mode.

**If you see**

```
RIDE_INIT=CONNECT:localhost:4502 ./kap --ride
Kap RIDE client connecting to localhost:4502...
kap --ride: cannot connect to localhost:4502: Connection refused (os error 111)
```

stride isn't running — start it first, or check `ss -tlnp | grep 4502`. `localhost` vs `127.0.0.1` are the same; the binary is `target/debug/kap` (symlink `./kap` is created for you).

Details, troubleshooting, and a Python harness that mimics stride's gateway for development: **`docs/ride.md`**.

---

## 8. Troubleshooting

| Symptom | Fix |
|---------|-----|
| `Variable not assigned: stats:mean` | Extension crate not linked: `kap-cli/Cargo.toml` missing dep or `use kap_ext_stats as _;` missing in `kap-cli/src/main.rs`. Check `KAP_DEBUG_NATIVES=1`. |
| `KAP natives: []` | Same linking cause; `cargo test -p kap-core` doesn't link `kap-ext-*` — only `kap-cli` does. |
| `cannot bind port 4502` | Previous `kap --serve` or stride still holding it. `lsof -i :4502` or `ss -tlnp \| grep 4502`, then kill. |
| `standard library failed to load` | Wrong CWD vs `--lib-path`. Run from repo root or pass `--lib-path=/home/theb/Apps/array/array/standard-lib`. |
| `2987` sweep shows `recv_timeout` in debug | Use `--release` — debug has a 10s `recv_timeout` phantom on `renderMapWithHorizontalOverflow` (12.84s debug vs 2.25s release). |

---

## 9. Where to read next

* **`docs/ride.md`** — full RIDE setup with stride, wire format, manual testing, and the `Connection refused` fix.
* **`docs/extensions.md`** — 10-minute “add a Rust primitive” tutorial.
* **`docs/native-api-design.html`** — RFC: trait design, `ScalarImpl`, `RankSpec`, migration plan (16k → 6k LOC).
* **`~/Apps/array/rust-kap` @ `main` (~06085e0 + ride)** — `114 lib + 4 ride` tests green, `2987/0/0` corpus.
