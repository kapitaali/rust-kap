# rust-kap

Rust rewrite of the **Kap** array language — 100% conformant with the Kotlin `array` reference (2987/0/0). Binary `kap` is a text REPL, file runner, and RIDE gateway for the [`stride`](https://github.com/kapitaali/stride) TUI editor.

```bash
cargo build -p kap-cli && ./target/debug/kap
# Kap 0.0.0 — REPL (Ctrl-D to exit)
>>> 1 2 3 + 4 5 6
(5 7 9)
```

## Docs

| Doc | What |
|-----|------|
| [`docs/getting-started.md`](docs/getting-started.md) | Install, build, REPL/file, flags, stdlib, testing, extensions & RIDE overview (start here) |
| [`docs/ride.md`](docs/ride.md) | RIDE protocol with stride — `editor.toml`, `--ride` vs `--serve`, handshake, `Connection refused` fix |
| [`docs/extensions.md`](docs/extensions.md) | Add a Rust primitive in 10 min (`kap-ext-my`, `my:hello`, `inventory::submit!`) |
| [`docs/native-api-design.html`](docs/native-api-design.html) | RFC — trait, `ScalarImpl`, `RankSpec`, migration plan (16k → 6k LOC) |

```bash
# RIDE with stride (auto-connect)
cargo build -p kap-cli && cd ~/Apps/stride && cargo run
# inside stride: 1+1 → 2

# standalone
RIDE_INIT=CONNECT:127.0.0.1:4502 ./target/debug/kap --ride
./target/debug/kap --serve 4502
```

2987-row sweep: `export JAVA_HOME=/home/theb/Apps/jdk-current && cargo test --release -p kap-core --test conformance run_kotlin_conformance` → `2987/0/0`.
