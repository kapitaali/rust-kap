# RIDE Protocol — Kap with stride

> Kap's RIDE gateway lets the [`stride`](https://github.com/kapitaali/stride) TUI editor (and Dyalog RIDE) evaluate Kap expressions over TCP. The wire format is **binary-framed**: `[4 bytes BE total length][4 bytes "RIDE"][JSON payload]` where `total = 8 + len(payload)`. This is the same protocol as `src/cn.js` in the [RIDE repo](https://github.com/Dyalog/ride) and the reference implementation in `rust-apl/src/main.rs`.

You don't need to know the framing to use it — stride and Kap handle it. This doc covers **how to plug them together**, how to verify, and how the protocol works if you need to debug it.

---

## 1. Roles — who is server vs client

| Setup | Who listens | Who connects | Flag |
|-------|-------------|--------------|------|
| **Stride + Kap (default)** | stride gateway server on `127.0.0.1:4502` | Kap client | `kap --ride` + `RIDE_INIT=CONNECT:127.0.0.1:4502` |
| Manual testing / `kap --serve` | Kap server on `127.0.0.1:4502` | any RIDE client (Python, stride as client) | `kap --serve 4502` |

Stride's default is the first row — stride is the server, Kap is the client. This doc uses that as the primary path. `--serve` exists for manual harness testing.

---

## 2. One-time setup

### 2.1 Build both

```bash
git clone https://github.com/kapitaali/rust-kap && cd rust-kap
cargo build -p kap-cli              # → target/debug/kap  (27M debug)
cargo build --release -p kap-cli    # → target/release/kap (8.6M, 2× faster)

git clone https://github.com/kapitaali/stride ~/Apps/stride
cd ~/Apps/stride && cargo build     # → target/debug/stride
```

A convenience symlink is kept at `rust-kap/kap → target/debug/kap` so `RIDE_INIT=... ./kap --ride` works as typed in examples.

### 2.2 Point stride at Kap

`stride/editor.toml` is the gateway config. After the fix in this repo it should read:

```toml
gateway_host = "127.0.0.1"
gateway_port = 4502
gateway_executable = "/home/theb/Apps/array/rust-kap/target/debug/kap"
#gateway_executable = "../rust-apl/target/debug/apl"
gateway_args = "--ride"
gateway_env = ["RIDE_INIT=CONNECT:127.0.0.1:4502"]
apl_version = "GNU APL 2.0 (Rust)"
auto_connect = true
max_results = 500
```

> **Prior bug:** the file had two `gateway_executable` lines — the first was a directory without `/kap`, the second pointed to `rust-apl`. TOML keeps the last, so stride kept spawning `rust-apl` even after you built Kap. The fix is one line: the absolute path to `target/debug/kap` (or `target/release/kap`).

If you edit `editor.toml`, restart stride to pick it up.

---

## 3. Running — the two modes

### 3.1 Auto-connect (recommended)

Stride spawns Kap itself. No manual `kap --ride` needed.

```bash
cd ~/Apps/stride
cargo run                  # or ./target/debug/stride
# stride listens on 127.0.0.1:4502, spawns Kap, TUI opens
# inside stride: type `1+1` → Kap evaluates → stride shows `2`
```

### 3.2 Manual connect

Useful when you want to see Kap's handshake logs or attach a debugger. Start stride first (so 4502 is listening), then in a second terminal:

```bash
RIDE_INIT=CONNECT:127.0.0.1:4502 /home/theb/Apps/array/rust-kap/target/debug/kap --ride
# or: RIDE_INIT=CONNECT:127.0.0.1:4502 ./kap --ride   (symlink)

# expected:
Kap RIDE client connecting to 127.0.0.1:4502...
Connected to RIDE server at 127.0.0.1:4502
Handshake sent. Waiting for commands...
```

`localhost:4502` works the same as `127.0.0.1:4502`.

### 3.3 Kap as server (manual harness)

For testing without stride:

```bash
./target/debug/kap --serve 4502
# Kap server listening on port 4502 (RIDE protocol)
```

Then connect with a Python client (see §5.2) or configure stride to use `gateway_args = "--serve"` (not the default).

---

## 4. Troubleshooting

### `Connection refused (os error 111)`

```
RIDE_INIT=CONNECT:localhost:4502 ./kap --ride
Kap RIDE client connecting to localhost:4502...
kap --ride: cannot connect to localhost:4502: Connection refused (os error 111)
```

**Cause:** nothing is listening on 4502. Kap `--ride` is a *client* — stride must be running first as the server.

**Fix:**

```bash
ss -tlnp | grep 4502   # should show stride after `cargo run`
ps aux | grep stride
# if empty: cd ~/Apps/stride && cargo run
```

Then retry the `RIDE_INIT=... ./kap --ride` (or just let stride auto-spawn Kap).

### `No such file or directory: ./kap`

The binary is `target/debug/kap` (or `target/release/kap`). The repo keeps a symlink `rust-kap/kap → target/debug/kap` for the `./kap` shorthand. If missing:

```bash
ln -sf target/debug/kap /home/theb/Apps/array/rust-kap/kap
```

Use the absolute path if you prefer: `/home/theb/Apps/array/rust-kap/target/debug/kap --ride`.

### Stride still spawns `rust-apl` not Kap

`editor.toml` has two `gateway_executable` lines — TOML keeps the last. Ensure only one active line points to Kap, as in §2.2.

### `cannot bind port 4502`

A previous `kap --serve` or stride still holds it:

```bash
ss -tlnp | grep 4502
lsof -i :4502
kill <pid>
```

### Kap evaluates but stride shows blank or duplicated lines

Check `gateway_port` matches `RIDE_INIT` port (both `4502`). With manual mode, Kap needs `RIDE_INIT` exactly `CONNECT:host:port`. See `kap-cli/src/ride.rs::parse_ride_init`.

---

## 5. How the protocol works (for developers)

### 5.1 Framing

Every message — including the handshake — is a frame:

```
[4 bytes BE total_len][4 bytes "RIDE"][payload bytes]
total_len = 8 + len(payload)
```

`payload` is either a handshake string (`"SupportedProtocols=2"`, `"UsingProtocol=2"`) or a JSON array `["Name", {...}]`. See `kap-cli/src/ride.rs::frame`.

```rust
pub fn frame(payload: &str) -> Vec<u8> {
    let total = (8 + payload.len()) as u32;
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.extend_from_slice(&total.to_be_bytes());
    buf.extend_from_slice(b"RIDE");
    buf.extend_from_slice(payload.as_bytes());
    buf
}
```

### 5.2 Handshake

* Client (Kap `--ride`) opens with two frames:

```
frame("SupportedProtocols=2")
frame("UsingProtocol=2")
```

* Server (stride) replies with two frames (if client is framed) or raw `UsingProtocol=2` (if raw):

```
frame("SupportedProtocols=2")
frame("UsingProtocol=2")
```

Both sides flush per block so coalesced TCP segments are handled via an accumulator. After handshake, client sends `["SetPromptType",{"type":1}]` (ready). See `ride_mode` and `handle_client` in `kap-cli/src/ride.rs` and `rust-apl/src/main.rs` reference.

### 5.3 Session messages

After handshake, every message is a JSON 2-element array `["Name", {...}]`:

| Direction | Name | Purpose |
|-----------|------|---------|
| client → server | `["Identify", {...}]` | interpreter announces itself; server replies `["ReplyIdentify", {...}]` with `arch:"Unicode/64"` and `version` (both non-empty — RIDE indexes `arch[0]` without guards). |
| server → client | `["ReplyIdentify", {...}]` | see `ride_identify()` in `kap-cli/src/ride.rs` — `Vendor:"rust-kap"`, `Language:"Kap"`, `version:"Kap 0.0.0"`, `arch:"Unicode/64"` |
| either | `["Connect", {"remoteId":N}]` | server replies `["ReplyConnect", {"remoteId":N}]` (stride does; Kap stays silent like `rust-apl` — clients ignore unknown replies). |
| server → client | `["Execute", {"text":"1+1","trace":0}]` | stride asks Kap to evaluate. Kap replies in order: `["EchoInput", {"input":"1+1","group":0}]`, `["SetPromptType",{"type":0}]` (busy), `["AppendSessionOutput", {"result":"2\n","type":2,"group":0}]` or `"type":5` on error, `["InterpreterStatus", {...}]`, `["SetPromptType",{"type":1}]` (ready). |
| client → server | `["AppendSessionOutput", ...]` | Kap's result. `type` 2 = value, 5 = error, 4 = system-command. `result` is `render_value` (`format_display` or `format_conform` if `--conform-display`), with trailing `\n` only if peer sent `Identify` (`peer_identified`). Stride strips `\r\n` per row. |
| client → server | `["EchoInput", ...]` | echo of the executed line (group 0). |
| server → client | `["Subscribe", {"status":["statusfields"]}]` | stride subscribes; Kap replies `["InterpreterStatus", {...}]` (fixed `IO:1` — Kap has no `⎕IO` tray yet). |
| server → client | `["GetWindowLayout"]`, `["GetSyntaxInformation"]`, `["GetLog"]`, `["SetPW"]`, `["GetLanguageBar"]`, `["GetKeyboardLayout"]`, `["GetConfiguration"]` | Kap replies with empty/minimal stubs (`entries:[]`, `keyMappings:{}`, `configurations:[]`) like `rust-apl` — stride carries on. |
| either | `["Exit", {"code":0}]` / `["Disconnect"]` | ends the session (returns `false` in `handle_command`). |

Kap's `handle_command` in `kap-cli/src/ride.rs` is a direct port of `rust-apl/src/main.rs::handle_command` with `Environment` replaced by `Session` and `render_session_value` replaced by `render_value` (`format_display`/`format_conform`). Each RIDE connection gets a fresh `Session` (so `a←5` then `a+1` works within that connection; a new connection starts empty). Standard library is loaded per-session unless `--no-standard-lib` is given; `--lib-path` and `--conform-display` are forwarded to the RIDE `Session`.

### 5.4 Manual harness (Python)

The repo's integration test used this harness — useful for developing without the TUI:

```python
import socket, struct, json

def frame(p): return struct.pack('>I', 8+len(p.encode()))+b'RIDE'+p.encode()
def read_frame(s):
    h=s.recv(8)
    flen=struct.unpack('>I', h[:4])[0]
    assert h[4:8]==b'RIDE'
    plen=flen-8
    payload=b''
    while len(payload)<plen: payload+=s.recv(plen-len(payload))
    return payload.decode()

# Kap as server
s=socket.create_connection(("127.0.0.1",4502))
s.sendall(frame("SupportedProtocols=2"))
print(read_frame(s))  # UsingProtocol=2
s.sendall(frame(json.dumps(["Identify",{"identity":1}])))
print(read_frame(s))  # ReplyIdentify
print(read_frame(s))  # SetPromptType
s.sendall(frame(json.dumps(["Execute",{"text":"1 2 3 + 4 5 6","trace":0}])))
while True:
    p=read_frame(s)
    print(p)
    if "SetPromptType" in p and '"type": 1' in p: break
# → AppendSessionOutput result "(5 7 9)\n" type 2
```

The same logic with `RIDE_INIT` drives Kap as a client (see `kap-cli/src/ride.rs::ride_mode` and the `test_ride_client.py` harness in CI).

---

## 6. Debugging

| What | How |
|------|-----|
| List natives inside Kap | `KAP_DEBUG_NATIVES=1 ./target/debug/kap --no-standard-lib <<< '1+1'` → `KAP natives: ["stats:mean", ...]` |
| Stride gateway logs | `STRIDE_DEBUG=1 cargo run` → `/tmp/stride_debug.log` (`debug_log` in `stride/src/gateway.rs`) |
| Kap RIDE logs | stderr: `ride: write failed`, `ride: ignoring unhandled message`, `Handshake sent. Waiting for commands...` |
| Port / process | `ss -tlnp \| grep 4502`, `lsof -i :4502`, `ps aux \| grep -E "kap|stride"` |
| Unit tests | `cargo test -p kap-cli` — 4 ride tests: `ride_init_parses_host_port`, `ride_init_rejects_garbage`, `reply_identify_satisfies_ride`, `handshake_frames_round_trip` |

---

## 7. Reference

* Kap RIDE implementation: `kap-cli/src/ride.rs` (462 lines) + `kap-cli/src/main.rs` (`--ride`/`--serve` dispatch, `Session` per connection).
* Upstream reference: `rust-apl/src/main.rs` (`serve_mode`, `ride_mode`, `frame`, `ride_identify`, `handle_command` — 822 lines, credited in file header).
* Stride gateway server: `stride/src/gateway.rs` (737 lines, `GatewayServer`, `perform_handshake`, `handle_interpreter`, `frame`, `normalize_output`).
* RIDE spec: `ride/docs/protocol.md` in the RIDE repo and `stride/META-INF/RIDE_CONFORMANCE_PLAN.md`.
