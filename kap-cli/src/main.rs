//! kap-cli: the native Kap text client.
//!
//! Phase 0 skeleton. Per strategy D4 (locked):
//!   * `kap file.kap` runs a file, prints result, exits.
//!   * No arg  -> a REPL loop.
//!   * Kap has NO `)`-prefixed session commands. File loading inside Kap is the
//!     `use("/path/file.kap")` parser directive (engine feature, Phase 3+).
//!     Quitting is a host concern (Ctrl-D / Ctrl-C -> process exit).
//!   * Multi-line continuation: a line ending in backtick `` ` `` (reference
//!     §"Line continuation"), or unbalanced `(`/`[`/`{`.

use kap_core::Engine;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let engine = Engine::new();

    match args.get(1) {
        Some(path) => {
            // Phase 0: file execution is a stub (engine.eval_string is unimplemented).
            // Real implementation (Phase 3): read file, engine.eval_file, print result
            // or formattedError.
            eprintln!("file mode not yet implemented: {}", path);
            let _ = engine;
        }
        None => {
            eprintln!("REPL not yet implemented (Phase 5).");
        }
    }
}
