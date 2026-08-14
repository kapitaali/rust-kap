//! kap-cli: the native Kap text client (REPL + file runner).
//!
//! * `kap`           -> interactive REPL backed by a stateful `kap_core::Session`
//! * `kap file.kap`  -> run the file, print the last result, exit
//!
//! Multi-line continuation: a line ending in a backtick `` ` `` continues, as
//! does an unbalanced count of `(` / `[` / `{`. Send EOF (Ctrl-D) or Ctrl-C to
//! quit. Kap has no `)`-prefixed session commands; quitting is a host concern.

use std::io::{self, BufRead, Write};

use kap_core::Session;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::new();

    match args.get(1) {
        Some(path) => {
            // File mode: read, evaluate once, print the last value.
            match std::fs::read_to_string(path) {
                Ok(src) => match session.eval(&src) {
                    Ok(v) => {
                        let out = v.format_value();
                        if !out.is_empty() {
                            println!("{}", out);
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("cannot read {}: {}", path, e);
                    std::process::exit(1);
                }
            }
        }
        None => repl(&session),
    }
}

fn repl(session: &Session) {
    let stdin = io::stdin();
    let mut buffer = String::new();
    println!("Kap {} — REPL (Ctrl-D to exit)", env!("CARGO_PKG_VERSION"));
    loop {
        print!("{}", if buffer.is_empty() { ">>> " } else { "··· " });
        io::stdout().flush().ok();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                // EOF: flush any pending buffer, then quit.
                if !buffer.is_empty() {
                    run(session, &std::mem::take(&mut buffer));
                }
                break;
            }
            Ok(_) => {
                let line = line.trim_end_matches(['\n', '\r']);
                let ends_backtick = line.ends_with('`');
                let trimmed = if ends_backtick {
                    &line[..line.len() - 1]
                } else {
                    line
                };
                buffer.push_str(trimmed);
                buffer.push('\n');

                // Continue if explicitly escaped or brackets are unbalanced.
                if ends_backtick || has_unbalanced(&buffer) {
                    continue;
                }
                run(session, &std::mem::take(&mut buffer));
            }
            Err(e) => {
                eprintln!("read error: {}", e);
                break;
            }
        }
    }
}

fn run(session: &Session, src: &str) {
    match session.eval(src) {
        Ok(v) => {
            let out = v.format_value();
            if !out.is_empty() {
                println!("{}", out);
            }
        }
        Err(e) => eprintln!("error: {}", e),
    }
}

/// True when `src` has more open than close brackets (needs continuation),
/// or a close precedes its open (malformed but we still wait for more input).
fn has_unbalanced(src: &str) -> bool {
    let mut depth: i32 = 0;
    for c in src.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return true;
        }
    }
    depth > 0
}
