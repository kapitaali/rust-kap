//! RIDE protocol for Kap — binary-framed gateway compatible with stride and Dyalog RIDE.
//!
//! Wire format: [4 bytes BE total length][4 bytes "RIDE"][JSON payload]
//! Total length = 8 + len(payload). This matches `src/cn.js` in the RIDE repo
//! and the implementation in `rust-apl/src/main.rs` which is the reference.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use kap_core::session::Session;
use kap_core::AplError;

/// Frame a JSON payload for the RIDE protocol.
pub fn frame(payload: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + payload.len());
    let total_len = (8 + payload.len()) as u32;
    buf.extend_from_slice(&total_len.to_be_bytes());
    buf.extend_from_slice(b"RIDE");
    buf.extend_from_slice(payload.as_bytes());
    buf
}

/// Parse RIDE_INIT=CONNECT:host:port (set by the RIDE side when it spawns
/// the interpreter, or by hand for a listening RIDE). Split from the right
/// so IPv6 hosts survive.
pub fn parse_ride_init(ride_init: &str) -> Option<(String, u16)> {
    let rest = ride_init.strip_prefix("CONNECT:")?;
    let (host, port) = rest.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse::<u16>().ok()?))
}

/// Send one framed RIDE message. Returns false when the peer is gone.
fn send_ride(stream: &mut TcpStream, payload: &str) -> bool {
    if let Err(e) = stream.write_all(&frame(payload)) {
        eprintln!("ride: write failed ({e}); closing session");
        return false;
    }
    if let Err(e) = stream.flush() {
        eprintln!("ride: flush failed ({e}); closing session");
        return false;
    }
    true
}

/// Full interpreter description for ReplyIdentify. RIDE reads `arch[0]` and
/// `version` with no guards, so both must be non-empty.
fn ride_identify() -> String {
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()))
        .unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    serde_json::json!(["ReplyIdentify", {
        "apiVersion": 1,
        "Port": 0,
        "IPAddress": "",
        "Vendor": "rust-kap",
        "Language": "Kap",
        "version": format!("Kap {}", env!("CARGO_PKG_VERSION")),
        "Machine": hostname,
        "arch": "Unicode/64",
        "Project": "CLEAR WS",
        "Process": "kap",
        "User": user,
        "pid": std::process::id() as i64,
        "token": "",
        "date": "",
        "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
    }])
    .to_string()
}

fn send_interpreter_status(stream: &mut TcpStream, _session: &Session) -> bool {
    // Kap has no ⎕IO tray yet; report fixed values like rust-apl.
    let msg = serde_json::json!(["InterpreterStatus", {
        "IO": 1, "DQ": 0, "WA": 0, "SI": 0, "TRAP": 0, "ML": 1,
        "NumThreads": 1, "TID": 0, "CompactCount": 0, "GarbageCount": 0,
    }]);
    send_ride(stream, &msg.to_string())
}

fn render_value(session: &Session, v: &kap_core::APLValue) -> String {
    if session.is_conform_display() {
        v.format_conform()
    } else {
        v.format_display()
    }
}

fn format_error(e: &AplError) -> String {
    match e {
        AplError::Parse { line, col, msg } => format!("parse error at {line}:{col}: {msg}"),
        AplError::Runtime(msg) => format!("error: {msg}"),
        AplError::Return(..) => "→: Call to return without a function call".to_string(),
        AplError::Thrown(_, data) => format!("error: throw: {}", data.format_value()),
    }
}

/// Handle one RIDE message from the peer. Returns false when the session must end.
fn handle_command(
    stream: &mut TcpStream,
    session: &Session,
    cmd: &str,
    args: &serde_json::Value,
    peer_identified: &mut bool,
) -> bool {
    fn session_text(result: String, peer_identified: bool) -> String {
        if peer_identified && !result.ends_with('\n') {
            format!("{result}\n")
        } else {
            result
        }
    }

    match cmd {
        "Identify" => {
            *peer_identified = true;
            if !send_ride(stream, &ride_identify()) {
                return false;
            }
            send_ride(
                stream,
                &serde_json::json!(["SetPromptType", {"type": 1}]).to_string(),
            )
        }
        "Connect" => true,
        "GetWindowLayout" => true,
        "GetSyntaxInformation" => true,
        "GetLog" => true,
        "SetPW" => true,
        "GetLanguageBar" => {
            let reply = serde_json::json!(["ReplyGetLanguageBar", {"entries": []}]);
            send_ride(stream, &reply.to_string())
        }
        "GetKeyboardLayout" => {
            let reply = serde_json::json!(["ReplyGetKeyboardLayout", {"keyMappings": {}}]);
            send_ride(stream, &reply.to_string())
        }
        "GetConfiguration" => {
            let names: Vec<String> = args["names"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let cfgs: Vec<serde_json::Value> = names
                .iter()
                .map(|n| serde_json::json!({"name": n, "value": ""}))
                .collect();
            let reply = serde_json::json!(["ReplyGetConfiguration", {"configurations": cfgs}]);
            send_ride(stream, &reply.to_string())
        }
        "Subscribe" => {
            let wants_status = args["status"]
                .as_array()
                .map(|a| a.iter().any(|s| s.as_str() == Some("statusfields")))
                .unwrap_or(false);
            if wants_status {
                send_interpreter_status(stream, session)
            } else {
                true
            }
        }
        "Exit" | "Disconnect" => false,
        "Execute" => {
            let text = args["text"].as_str().unwrap_or("");
            let expr = text.trim();
            if expr.is_empty() {
                return true;
            }
            if !send_ride(
                stream,
                &serde_json::json!(["EchoInput", {"input": text, "group": 0}]).to_string(),
            ) {
                return false;
            }
            if !send_ride(
                stream,
                &serde_json::json!(["SetPromptType", {"type": 0}]).to_string(),
            ) {
                return false;
            }

            // In Kap, `)` or `]` are not system commands; evaluate as normal.
            // We still handle a bare empty to stay silent.

            let evaluated = match session.eval(expr) {
                Ok(v) => {
                    let result = render_value(session, &v);
                    // Kap's REPL suppresses empty strings; RIDE mirrors that.
                    if result.is_empty() {
                        true
                    } else {
                        let output = serde_json::json!(["AppendSessionOutput", {
                            "result": session_text(result, *peer_identified),
                            "group": 0,
                            "type": 2
                        }]);
                        send_ride(stream, &output.to_string())
                    }
                }
                Err(e) => {
                    let msg = session_text(format_error(&e), *peer_identified);
                    let output = serde_json::json!(["AppendSessionOutput", {
                        "result": msg,
                        "group": 0,
                        "type": 5
                    }]);
                    send_ride(stream, &output.to_string())
                }
            };
            if !evaluated {
                return false;
            }
            send_interpreter_status(stream, session);
            send_ride(
                stream,
                &serde_json::json!(["SetPromptType", {"type": 1}]).to_string(),
            )
        }
        "ReplyIdentify" | "ReplyConnect" => true,
        _ => {
            eprintln!("ride: ignoring unhandled message {cmd}");
            true
        }
    }
}

fn handle_client(mut stream: TcpStream, lib_paths: Vec<String>, no_stdlib: bool, conform: bool) {
    let session = Session::new();
    if !lib_paths.is_empty() {
        session.set_lib_paths(&lib_paths);
    }
    if conform {
        session.set_conform_display(true);
    }
    if !no_stdlib {
        if let Err(e) = session.load_standard_lib() {
            eprintln!("warning: standard library failed to load: {}", e);
        }
    }

    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    let mut peer_identified = false;

    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => acc.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
        loop {
            if acc.len() < 8 {
                break;
            }
            let frame_len = u32::from_be_bytes([acc[0], acc[1], acc[2], acc[3]]) as usize;
            if acc.len() < frame_len {
                break;
            }
            let payload = String::from_utf8_lossy(&acc[8..frame_len]).to_string();
            acc.drain(0..frame_len);

            if payload.starts_with('[') {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) {
                    if let Some(arr) = val.as_array() {
                        let cmd = arr[0].as_str().unwrap_or("");
                        let args = arr.get(1).cloned().unwrap_or(serde_json::Value::Null);
                        if !handle_command(&mut stream, &session, cmd, &args, &mut peer_identified)
                        {
                            return;
                        }
                    }
                }
            } else if payload.starts_with("SupportedProtocols=") {
                let _ = stream.write_all(&frame("UsingProtocol=2"));
                let _ = stream.flush();
            }
        }
    }
}

/// Serve mode: listen on a TCP port, speak the RIDE binary-framed protocol.
pub fn serve_mode(port: u16, lib_paths: Vec<String>, no_stdlib: bool, conform: bool) {
    let listener = match TcpListener::bind(format!("127.0.0.1:{port}")) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("kap server: cannot bind port {port}: {e}");
            std::process::exit(1);
        }
    };
    println!("Kap server listening on port {port} (RIDE protocol)");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let lp = lib_paths.clone();
                std::thread::spawn(move || handle_client(stream, lp, no_stdlib, conform));
            }
            Err(e) => eprintln!("Connection failed: {e}"),
        }
    }
}

/// Ride mode: connect to a RIDE peer (stride or RIDE editor) as the interpreter.
pub fn ride_mode(lib_paths: Vec<String>, no_stdlib: bool, conform: bool) {
    let ride_init = std::env::var("RIDE_INIT").unwrap_or_default();
    let Some((host, port)) = parse_ride_init(&ride_init) else {
        eprintln!("kap --ride: RIDE_INIT must be in format CONNECT:host:port");
        eprintln!("  e.g., RIDE_INIT=CONNECT:localhost:4502 kap --ride");
        std::process::exit(1);
    };
    let addr = format!("{host}:{port}");
    println!("Kap RIDE client connecting to {addr}...");

    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kap --ride: cannot connect to {addr}: {e}");
            std::process::exit(1);
        }
    };
    println!("Connected to RIDE server at {addr}");

    for hello in ["SupportedProtocols=2", "UsingProtocol=2"] {
        if stream.write_all(&frame(hello)).is_err() {
            eprintln!("kap --ride: handshake failed (peer went away)");
            std::process::exit(1);
        }
    }
    stream.flush().ok();

    println!("Handshake sent. Waiting for commands...");

    let session = Session::new();
    if !lib_paths.is_empty() {
        session.set_lib_paths(&lib_paths);
    }
    if conform {
        session.set_conform_display(true);
    }
    if !no_stdlib {
        if let Err(e) = session.load_standard_lib() {
            eprintln!("warning: standard library failed to load: {}", e);
        }
    }

    if !send_ride(
        &mut stream,
        &serde_json::json!(["SetPromptType", {"type": 1}]).to_string(),
    ) {
        eprintln!("kap --ride: peer went away during handshake");
        std::process::exit(1);
    }

    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    let mut peer_identified = false;

    loop {
        match stream.read(&mut buf) {
            Ok(0) => {
                println!("RIDE disconnected");
                break;
            }
            Ok(n) => acc.extend_from_slice(&buf[..n]),
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }
        loop {
            if acc.len() < 8 {
                break;
            }
            let frame_len = u32::from_be_bytes([acc[0], acc[1], acc[2], acc[3]]) as usize;
            if frame_len < 8 {
                eprintln!("ride: bad frame length {frame_len}; closing session");
                return;
            }
            if acc.len() < frame_len {
                break;
            }
            if &acc[4..8] != b"RIDE" {
                eprintln!("ride: bad frame magic; closing session");
                return;
            }
            let payload = String::from_utf8_lossy(&acc[8..frame_len]).to_string();
            acc.drain(0..frame_len);

            if payload.starts_with('[') {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) {
                    if let Some(arr) = val.as_array() {
                        let cmd = arr[0].as_str().unwrap_or("");
                        let args = arr.get(1).cloned().unwrap_or(serde_json::Value::Null);
                        if !handle_command(&mut stream, &session, cmd, &args, &mut peer_identified)
                        {
                            println!("RIDE session ended");
                            return;
                        }
                    }
                }
            } else if payload == "SupportedProtocols=2" {
            } else if payload.starts_with("UsingProtocol=") {
            } else {
                eprintln!("ride: ignoring handshake text {payload:?}");
            }
        }
    }
}

#[cfg(test)]
mod ride_tests {
    use super::*;

    #[test]
    fn ride_init_parses_host_port() {
        assert_eq!(
            parse_ride_init("CONNECT:localhost:4502"),
            Some(("localhost".to_string(), 4502))
        );
        assert_eq!(
            parse_ride_init("CONNECT:127.0.0.1:4502"),
            Some(("127.0.0.1".to_string(), 4502))
        );
    }

    #[test]
    fn ride_init_rejects_garbage() {
        assert_eq!(parse_ride_init(""), None);
        assert_eq!(parse_ride_init("SERVE:127.0.0.1:4502"), None);
        assert_eq!(parse_ride_init("CONNECT::4502"), None);
        assert_eq!(parse_ride_init("CONNECT:host:notaport"), None);
    }

    #[test]
    fn reply_identify_satisfies_ride() {
        let v: serde_json::Value = serde_json::from_str(&ride_identify()).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0].as_str().unwrap(), "ReplyIdentify");
        let body = &arr[1];
        assert_eq!(body["apiVersion"].as_i64().unwrap(), 1);
        assert!(!body["arch"].as_str().unwrap().is_empty());
        assert!(!body["version"].as_str().unwrap().is_empty());
    }

    #[test]
    fn handshake_frames_round_trip() {
        for payload in ["SupportedProtocols=2", "UsingProtocol=2"] {
            let f = frame(payload);
            assert_eq!(
                u32::from_be_bytes([f[0], f[1], f[2], f[3]]) as usize,
                f.len()
            );
            assert_eq!(&f[4..8], b"RIDE");
            assert_eq!(&f[8..], payload.as_bytes());
        }
    }
}
