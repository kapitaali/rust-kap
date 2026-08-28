//! kap-cli: the native Kap text client (REPL + file runner).
//!
//! * `kap`           -> interactive REPL backed by a stateful `kap_core::Session`
//! * `kap file.kap`  -> run the file, print the last result, exit
//!
//! Multi-line continuation: a line ending in a backtick `` ` `` continues, as
//! does an unbalanced count of `(` / `[` / `{`. Send EOF (Ctrl-D) or Ctrl-C to
//! quit. Kap has no `)`-prefixed session commands; quitting is a host concern.

use std::io::{self, BufRead, Write};

use kap_core::session::Session;
use kap_core::AplError;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::new();

    // Parse `--lib-path=PATH` / `-p PATH` (the port analog of kap-jvm-text's
    // `--lib-path`): one or more standard-library directories `use(...)` searches
    // first when resolving a file by basename. Stops at the first positional (file)
    // argument, which switches to file mode.
    let mut lib_paths: Vec<String> = Vec::new();
    let mut positional: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--lib-path" {
            if let Some(v) = args.get(i + 1) {
                lib_paths.push(v.clone());
                i += 2;
                continue;
            }
            eprintln!("error: --lib-path requires a path");
            std::process::exit(2);
        } else if let Some(p) = a.strip_prefix("--lib-path=") {
            lib_paths.push(p.to_string());
            i += 1;
            continue;
        } else if a == "-p" {
            if let Some(v) = args.get(i + 1) {
                lib_paths.push(v.clone());
                i += 2;
                continue;
            }
            eprintln!("error: -p requires a path");
            std::process::exit(2);
        } else if a.starts_with("-p") {
            lib_paths.push(a[2..].to_string());
            i += 1;
            continue;
        } else if a == "--conform-display" {
            session.set_conform_display(true);
            i += 1;
            continue;
        } else if a == "--no-standard-lib" {
            // Startup stdlib-suppression flag (handled again below at load time); it
            // is NOT a file argument, so skip it here and do not switch to file mode.
            i += 1;
            continue;
        } else if a.starts_with('-') && a.len() > 1 && a != "-n" && !a.starts_with("--no-") {
            // Unknown flag; ignore (could be a REPL flag passed through) but don't
            // treat as the file. We only switch to file mode on a non-flag arg.
            i += 1;
            continue;
        } else {
            positional = Some(a.clone());
            i += 1;
            break;
        }
    }
    if !lib_paths.is_empty() {
        session.set_lib_paths(&lib_paths);
    }

    // Startup stdlib load (Kotlin repl-builder.kt loadStdLibrary): unless
    // `--no-standard-lib` is given, evaluate `use("standard-lib.kap")` before any
    // user code so `⎕A`, `when`, `split`, … are pre-defined. A failed load is
    // non-fatal for the REPL but reported on stderr.
    let no_stdlib = args.iter().any(|a| a == "--no-standard-lib");
    if !no_stdlib {
        if let Err(e) = session.load_standard_lib() {
            eprintln!("warning: standard library failed to load: {}", e);
        }
    }

    match positional {
        Some(path) => {
            // File mode: read, evaluate once, print the last result (REPL-style
            // display: strings are quoted, matching Real Kap).
            match std::fs::read_to_string(&path) {
                Ok(src) => match session.eval(&src) {
                    Ok(v) => {
                        let out = v.format_display();
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
            let out = if session.is_conform_display() {
                v.format_conform()
            } else {
                v.format_display()
            };
            if !out.is_empty() {
                println!("{}", out);
            }
        }
        Err(e) => match &e {
            AplError::Parse { line, col, msg } => {
                eprintln!("parse error at {}:{}: {}", line, col, msg)
            }
            AplError::Runtime(msg) => eprintln!("error: {}", msg),
            AplError::Return(_) => eprintln!("→: Call to return without a function call"),
        },
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
