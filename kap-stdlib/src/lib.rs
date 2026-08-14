//! kap-stdlib: the Kap standard library source files.
//!
//! These are the **same `.kap` files** the Kotlin engine loads at startup
//! (listed in `array/src/.../stdlib-files.kt`). They are Kap source, not Rust —
//! a semantics-compatible Rust engine runs them as-is (strategy §1.3, §5).
//!
//! Phase 0: the files are vendored verbatim under `std/` and exposed via
//! [`STDLIB_FILES`] (relative paths). The engine will read them at startup.

/// Ordered list of standard-library files the engine loads, matching
/// `stdlib-files.kt`'s `stdlibFilesList`.
pub const STDLIB_FILES: &[&str] = &[
    "std/standard-lib.kap",
    "std/base-functions.kap",
    "std/http.kap",
    "std/io.kap",
    "std/map.kap",
    "std/math.kap",
    "std/math-kap.kap",
    "std/output.kap",
    "std/output3.kap",
    "std/thread.kap",
    "std/regex.kap",
    "std/structure.kap",
    "std/time.kap",
    "std/util.kap",
    "std/stat.kap",
    "std/graph.kap",
    "std/fhelp.kap",
    "std/fhelp-impl.kap",
];

/// Directory (relative to this crate's manifest dir) holding the `.kap` sources.
pub const STDLIB_DIR: &str = "std";
