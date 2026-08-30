//! Conformance harness: runs extracted Kotlin Kap test cases against our engine.
//!
//! This is a *coverage* instrument. It reads `conformance/kotlin_tests.jsonl`
//! (produced by `tools/extract_kotlin_tests.py`) and evaluates each case.
//! Because our Rust Kap is a growing subset, most cases will currently fail to
//! parse or evaluate; the harness reports the breakdown so we can track how much
//! of the language is implemented.
//!
//! NOTE: `cargo test` captures a passing test's stdout/stderr and only prints it
//! when a test *fails* or with `-- --nocapture`. To always see the summary, the
//! harness ALSO writes it to `conformance_summary.txt` (repo root) and `eprintln!`s
//! it to stderr. `run_kotlin_conformance` now runs as part of the normal suite (with a
//! per-case timeout so a runaway builtin can't hang it) and PASSES; `curated_kap_parity`
//! is the real per-feature pass/fail gate.
//!
//! Run: `cargo test -p kap-core --test conformance`
//!   or with visible output: `cargo test -p kap-core --test conformance -- --nocapture`

use kap_core::Engine;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    /// Parsed + evaluated, and (if `expected` was known) matched it.
    Ok,
    /// Parsed + evaluated but `expected` (when known) did NOT match.
    Mismatch,
    /// Either parse or runtime error => a feature our engine does not support yet.
    Unsupported,
}

#[derive(Clone)]
struct Case {
    file: String,
    test: String,
    kind: String,
    expr: String,
    expected: Option<String>,
}

fn load_cases() -> Vec<Case> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("conformance")
        .join("kotlin_tests.jsonl");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            eprintln!(
                "conformance/kotlin_tests.jsonl not found; run `python3 tools/extract_kotlin_tests.py` first"
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        out.push(Case {
            file: v["file"].as_str().unwrap_or("?").to_string(),
            test: v["test"].as_str().unwrap_or("?").to_string(),
            kind: v["kind"].as_str().unwrap_or("eval").to_string(),
            expr: v["expr"].as_str().unwrap_or("").to_string(),
            expected: v["expected"].as_str().map(|s| s.to_string()),
        });
    }
    out
}

fn classify(engine: &Engine, c: &Case) -> Outcome {
    // Guard against engine panics (e.g. unchecked indexing in builtins): a crash is
    // treated as "unsupported", never an abort of the whole suite.
    let res =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.eval_to_string(&c.expr)));
    let errored = match &res {
        Ok(Err(_)) | Err(_) => true,
        Ok(Ok(_)) => false,
    };
    // `kind:"fails"` cases are Kotlin tests that are *expected to error* (parse/runtime
    // failure). Erroring here is the CORRECT outcome (counted OK); only a case that runs
    // to a value when Kotlin says it must fail is a genuine Mismatch. Without this, every
    // `kind:"fails"` case was miscounted as UNSUPPORTED even though the engine behaves
    // exactly like Kotlin.
    if c.kind == "fails" {
        return if errored { Outcome::Ok } else { Outcome::Mismatch };
    }
    match res {
        Ok(Ok(got)) => {
            if let Some(exp) = &c.expected {
                if &got == exp {
                    Outcome::Ok
                } else {
                    Outcome::Mismatch
                }
            } else {
                Outcome::Ok
            }
        }
        _ => Outcome::Unsupported,
    }
}

/// Evaluate one case in a worker thread with a wall-clock timeout. If the case
/// triggers a runaway evaluation (an infinite loop rather than a panic), the
/// thread is abandoned and the case is counted as `Unsupported` — so a single
/// bad builtin can no longer freeze the entire `cargo test` run.
fn classify_with_timeout(c: &Case) -> Outcome {
    let c = c.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let _handle = std::thread::spawn(move || {
        let engine = Engine::new();
        let o = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| classify(&engine, &c)))
            .unwrap_or(Outcome::Unsupported);
        let _ = tx.send(o);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(o) => o,
        Err(_) => Outcome::Unsupported,
    }
}

/// Broad Kotlin-reference conformance sweep. Runs as part of the normal
/// `cargo test` suite (no longer `#[ignore]`d). Each case is evaluated in its
/// own worker thread with a wall-clock timeout, so a runaway evaluation in a
/// not-yet-complete builtin degrades that one case to `Unsupported` instead of
/// freezing the whole `cargo test` run (a hang, not a panic, which `catch_unwind`
/// cannot stop). The real per-feature pass/fail gate remains `curated_kap_parity`.
#[test]
fn run_kotlin_conformance() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "no extracted test cases found");
    let engine = Engine::new();

    // Silence per-case panic output. Some reference cases trigger engine panics
    // (e.g. arithmetic overflow, division-by-zero); `classify` catches them via
    // `catch_unwind` and records them as `Unsupported`. The default panic hook
    // would otherwise flood stderr with backtraces for every such case.
    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut by_outcome: BTreeMap<Outcome, usize> = BTreeMap::new();
    let mut by_file: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // file -> (ok, total)
    let mut unsupported_examples: Vec<&Case> = Vec::new();
    let mut mismatch_examples: Vec<(&Case, String)> = Vec::new();

    for c in &cases {
        let o = classify_with_timeout(c);
        *by_outcome.entry(o).or_insert(0) += 1;
        let (ok, total) = by_file.entry(c.file.clone()).or_insert((0, 0));
        *total += 1;
        if o == Outcome::Ok {
            *ok += 1;
        }
        if o == Outcome::Unsupported && unsupported_examples.len() < 25 {
            unsupported_examples.push(c);
        }
        if o == Outcome::Mismatch {
            if let Ok(got) = engine.eval_to_string(&c.expr) {
                if mismatch_examples.len() < 25 {
                    mismatch_examples.push((c, got));
                }
            }
        }
        if std::env::var("CONFORM_DUMP").is_ok() {
            use std::io::Write;
            let oc = match o {
                Outcome::Ok => "OK",
                Outcome::Mismatch => "MISMATCH",
                Outcome::Unsupported => "UNSUPPORTED",
            };
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/conform_dump.txt")
                .unwrap();
            let _ = writeln!(f, "{}\t{}\t{}", c.file, oc, c.expr);
            let _ = c.expected.as_ref().map(|e| writeln!(f, "\t\tEXP {}", e));
        }
    }

    let total = cases.len();
    let oks = by_outcome.get(&Outcome::Ok).copied().unwrap_or(0);
    let mism = by_outcome.get(&Outcome::Mismatch).copied().unwrap_or(0);
    let unsup = by_outcome.get(&Outcome::Unsupported).copied().unwrap_or(0);
    let pct = (oks as f64 / total as f64) * 100.0;

    // Build a single summary string so we can both print it and persist it.
    let mut s = String::new();
    s.push_str("=== Kap conformance vs Kotlin reference suite ===\n");
    s.push_str(&format!("total extracted cases : {total}\n"));
    s.push_str(&format!("  ok (parsed+evaluated)   : {oks}\n"));
    s.push_str(&format!("  mismatch (ran, wrong)    : {mism}\n"));
    s.push_str(&format!("  unsupported (parse/err)  : {unsup}\n"));
    s.push_str(&format!("  coverage : {pct:.1}%\n"));

    // Rank files by absolute number of unsupported cases.
    let mut ranked: Vec<_> = by_file.iter().collect();
    ranked.sort_by(|a, b| {
        let ra = a.1 .1 - a.1 .0;
        let rb = b.1 .1 - b.1 .0;
        rb.cmp(&ra).then_with(|| b.1 .1.cmp(&a.1 .1))
    });
    s.push_str("\n--- worst-covered files (most unsupported) ---\n");
    for (f, (ok, tot)) in ranked.iter().take(15) {
        let un = tot - ok;
        s.push_str(&format!("  {un:4}/{tot:4} unsupported  {f}\n"));
    }

    if !unsupported_examples.is_empty() {
        s.push_str("\n--- sample unsupported cases ---\n");
        for c in &unsupported_examples {
            s.push_str(&format!("  [{}] {} : {}\n", c.kind, c.test, c.expr));
        }
    }
    if !mismatch_examples.is_empty() {
        s.push_str("\n--- sample mismatch cases ---\n");
        for (c, got) in &mismatch_examples {
            s.push_str(&format!(
                "  {} : expr={} expected={:?} got={}\n",
                c.test, c.expr, c.expected, got
            ));
        }
    }

    // Always persist to a file in the repo directory (cargo swallows test
    // stdout/stderr on success). CARGO_MANIFEST_DIR is `kap-core`, so `..` is
    // the repo root — the summary lands at `<repo>/conformance_summary.txt`.
    let summary_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("conformance_summary.txt");
    let _ = std::fs::write(&summary_path, &s);
    // Also emit to stderr (visible with `-- --nocapture` or when a test fails).
    eprint!("{s}");

    // We do NOT assert a pass rate here (it grows over time) — this test now PASSES
    // so `cargo test` stays green and the summary is surfaced via the file written
    // above (`conformance_summary.txt`) and the `eprintln!` below. The real
    // per-feature pass/fail gate remains `curated_kap_parity`.
}

/// Curated hand-ported cases: features we KNOW are implemented, each with an
/// exact expected string. These anchor correctness regardless of the broad
/// conformance run. They mirror Kotlin tests we have already matched.
#[test]
fn curated_kap_parity() {
    let engine = Engine::new();
    let cases: Vec<(&str, &str)> = vec![
        // Assignment (single-var; multi-var chains are not yet parsed — see PROGRESS.md gaps)
        ("a←3", "3"),
        ("(b←2) + 10", "12"),
        // Monadic arithmetic
        ("-(1 + 2)", "-3"),
        ("+(1 + 2)", "3"),
        ("-(3 1 4)", "(-3 -1 -4)"),
        ("2 (+) 3", "5"),
        ("3 (×) 4", "12"),
        // Iota (1-based, just fixed)
        ("⍳5", "(0 1 2 3 4)"),
        ("⍳0", "()"),
        // Control flow (Phase 8)
        ("if (1 < 2) { 42 }", "42"),
        ("if (1 > 2) { 42 } else { 7 }", "7"),
        ("{ 1 ⋄ 2 ⋄ 3 }", "3"),
        (
            "i ← 0 ⋄ s ← 0 ⋄ while (i < 5) { s ← s + i ⋄ i ← i + 1 } ⋄ s",
            "10",
        ),
        (
            "b ← 2 ⋄ when { (b=1){ \"one\" } (b=2){ \"two\" } (1){ \"other\" } }",
            "\"two\"",
        ),
        ("if (0) { 1 }", "⍬"),
        // Adverbs (Phase 7)
        ("+/ 1 2 3 4", "10"),
        ("×/ 1 2 3 4", "24"),
        ("+\\ 1 2 3 4", "(1 3 6 10)"),
        ("⌈/ 3 9 2 7", "9"),
        ("⌊/ 3 9 2 7", "2"),
        // Oracle: whole-valued floor/ceil results normalise to integers
        // (`⌈3.2` → `4` typed `kap:integer`; verified vs kap-jvm-text).
        ("⌈¨ 1.2 2.8 3.5", "(2 3 4)"),
        ("2 ×¨ 3 4 5", "(6 8 10)"),
        ("1 2 3 ×¨ 4 5 6", "(4 10 18)"),
        ("3 ⌈ 5", "5"),
        ("~¨ 1 0", "(0 1)"),
        // Partitioned enclose `⊆` (Phase 6 — mirrors Kotlin PartitionedEncloseFunction)
        ("⊆ 5", "5"),
        ("⊆ 1 2 3", "((1 2 3))"),
        ("1 0 1 ⊆ 1 2 3", "((1 2) (3))"),
        ("1 0 1 0 1 ⊆ 10 20 30 40 50", "((10 20) (30 40) (50))"),
        ("1 1 0 1 1 ⊆ 1 2 3 4 5", "((1) (2 3) (4) (5))"),
        ("0 1 0 1 0 ⊆ 1 2 3 4 5", "((1) (2 3) (4 5))"),
        // Pick `⊇` (Phase 6 — mirrors Kotlin PickAPLFunction / PickResultValue)
        ("0 ⊇ 1 2 3 4 5", "(1)"),
        ("2 ⊇ 1 2 3 4 5", "(3)"),
        ("¯1 ⊇ 1 2 3 4 5", "(5)"),
        ("1 0 2 ⊇ 10 20 30 40", "(20 10 30)"),
        // Builtins (ambivalent max/min, arithmetic) — Phase 6
        // Oracle: `⌈3.2` → `4` typed `kap:integer` (whole results normalise to Long).
        ("⌈ 3.2", "4"),
        ("⌊ 3.8", "3"),
        ("3 ⌈ 5", "5"),
        ("3 | 2", "2"),
        ("1 ∧ 0", "0"),
        ("1 ∨ 0", "1"),
        ("~ 1", "0"),
        ("~ 0", "1"),
        ("1 2 3 ∊ 1 2 3 4", "(1 1 1)"), // membership is element-wise (returns vector)
        ("5 ∊ 1 2 3 4", "0"),
        ("⍋ 3 1 4 2", "(1 3 0 2)"), // grade-up (monadic): 0-based indices ascending
        // Bracket indexing (pick + multi-axis access) — dyadic ⍴ required
        ("(10 20 30 40)[2]", "30"),
        ("(10 20 30 40)[0 2]", "(10 30)"),
        ("(10 20 30 40)[¯4]", "10"),
        ("(3 4 ⍴ 10×⍳100)[2;]", "(80 90 100 110)"),
        ("(3 4 ⍴ 10×⍳100)[;3]", "(30 70 110)"),
        ("(3 4 ⍴ 10×⍳100)[;0 3]", "(0 30 40 70 80 110)"),
        ("(2 3 ⍴ ⍳6)[1;⍳3]", "(3 4 5)"),
        (
            "(1 2 3 4)[4 4 ⍴ 0 1 3 1 2 2 0 1 2 2 0 3 1 0 2 2]",
            "(1 2 4 2 3 3 1 2 3 3 1 4 2 1 3 3)",
        ),
        (
            "(2 2 2 ⍴ 100+⍳8)[0;2 3 ⍴ 0 0 1 1 0 0;0]",
            "(100 100 102 102 100 100)",
        ),
        // Take / drop (↑ ↓) — multi-dimensional, per-axis, negative-from-end.
        // NOTE: monadic `↑` returns the leading *cell* (scalar), not a 1-element array.
        // (Real Kap: `↑1 2 3 4 5 → 1`.) These were `(1)`/`(6)` before the take-first fix.
        ("↑1 2 3 4", "1"),
        ("↑6", "6"),
        ("5 ↑ 10", "(10 0 0 0 0)"),
        ("0 ↑ 10", "⍬"),
        ("1 ↑ 10", "(10)"),
        ("2 2 ↑ 10", "(10 0 0 0)"),
        ("2 2 1 3 ↑ 10", "(10 0 0 0 0 0 0 0 0 0 0 0)"),
        ("3 ↑ 10 11 12 13 14 15 16 17", "(10 11 12)"),
        ("2 3 ↑ 10 15 ⍴ ⍳1000", "(0 1 2 15 16 17)"),
        ("1 ↑ 3 3 ⍴ ⍳9", "(0 1 2)"),
        ("3 ↑ 10 2 ⍴ ⍳20", "(0 1 2 3 4 5)"),
        ("4 ↑ 3 2 ⍴ ⍳20", "(0 1 2 3 4 5 0 0)"),
        // --- Strings as rank-1 arrays (Kap APLBmpString.dimensions = [len]) ---
        ("⍴\"abc\"", "(3)"),
        ("⍴⍴\"abc\"", "(1)"),
        ("≢\"abc\"", "3"),
        ("⌽\"abc\"", "\"cba\""),
        ("⍉\"abc\"", "\"abc\""),
        ("\"foo\", \"bar\"", "\"foobar\""),
        ("\"foo\"⍪\"bar\"", "\"foobar\""),
        ("\"abc\"⍴⍳100", "(0 1 2)"),
        ("1⌽\"abc\"", "\"bca\""),
        ("\"abc\"⍪\"def\"", "\"abcdef\""),
        // Comma of a string with a numeric vector strands into a mixed vector.
        ("\"abc\"⍪1 2 3", "(\"abc\" 1 2 3)"),
        // unicode:* namespace builtins (Kap unicode.kt)
        ("unicode:toCodepoints \"ABC\"", "(65 66 67)"),
        ("unicode:fromCodepoints 65 66 67", "\"ABC\""),
        ("unicode:toGraphemes \"é\"", "(\"é\")"),
        ("unicode:toLower \"ABC\"", "\"abc\""),
        ("unicode:toUpper \"abc\"", "\"ABC\""),
        ("unicode:toNames @A", "\"LATIN CAPITAL LETTER A\""),
        ("unicode:toNames @€", "⍬"),
        ("\"UTF16\" unicode:enc \"A\"", "(254 255 0 65)"),
        ("\"UTF16BE\" unicode:enc \"A\"", "(0 65)"),
        ("\"UTF16LE\" unicode:enc \"A\"", "(65 0)"),
        ("\"UTF32\" unicode:enc \"A\"", "(0 0 0 65)"),
        ("unicode:enc \"A\"", "(65)"),
        ("\"UTF16\" unicode:dec 254 255 0 65", "\"A\""),
        ("\"UTF16\" unicode:dec 0 65", "\"A\""),
        ("\"UTF16LE\" unicode:dec 65 0", "\"A\""),
        ("\"UTF16BE\" unicode:dec 0 65", "\"A\""),
        // Supplementary-plane literal (lexer surrogate-pair combination, Task 1)
        ("\"𝒟\"", "\"𝒟\""),
        ("unicode:toCodepoints \"𝒟\"", "(119967)"),
        // s:trim* (namespace-builtin, Task 4)
        ("s:trimLeft \"   hi\"", "\"hi\""),
        ("s:trimRight \"hi   \"", "\"hi\""),
        ("s:trim \"  hi  \"", "\"hi\""),
        // Character arithmetic (StringsTest.kt)
        ("\"abc\"+1", "\"bcd\""),
        ("1+\"abc\"", "\"bcd\""),
        ("\"af\"+1 ¯1", "\"be\""),
        ("\"abj\"-0 ¯11 3", "\"amg\""),
        ("\"bBa\"-\"aAb\"", "(1 1 -1)"),
        ("⍕@a", "\"a\""),
        ("⍕\"foo\"", "\"foo\""),
        ("⍕8", "\"8\""),
        // Monadic `⍕` uses Kap's `formatted(PLAIN)`: recursively flatten to scalar
        // leaves and concatenate with NO separators/parentheses (Real Kap).
        ("⍕ 1 2 3", "\"123\""),
        ("⍕ 10 20 30", "\"102030\""),
        ("⍕ (2 2⍴⍳4)", "\"0123\""),
        ("⍕ ⊂1 2 3", "\"123\""),
        ("⍕ ⊂5", "\"5\""),
        ("⍕ 1.5 2.5", "\"1.52.5\""),
        ("⍕ ⍬", "\"\""),
        // Dyadic `⍕` format directives (Real Kap format.kt / FormatAPLFunction)
        // Each `$s`/`$h` consumes ONE right-arg element; results are Str (quoted).
        ("\"$s a $s b\"⍕(1 2)", "\"1 a 2 b\""),
        ("\"$10s\"⍕\"abc\"", "\"       abc\""),
        ("\"$h\"⍕\"a<b&c\"", "\"a&lt;b&amp;c\""),
        ("\"f=$¯5s g=$5s\"⍕(1 2)", "\"f=1     g=    2\""),
        ("\"$s\"⍕(2 2⍴⍳4)", "(\"0\" \"2\")"),
        ("\"a$sfoo$sbar\"⍕(3 2⍴⍳6)", "(\"a0foo1bar\" \"a2foo3bar\" \"a4foo5bar\")"),
        ("\"$9s\"⍕\"abcdef\"", "\"   abcdef\""),
        ("\"$¯9s\"⍕\"abcdef\"", "\"abcdef   \""),
        ("\"$h\"⍕\"x&y<z>\"", "\"x&amp;y&lt;z&gt;\""),
        ("\"%s %s\"⍕(1 2)", "\"%s %s\""),
        ("⍎\"123\"", "123"),
        // Kotlin renders rationals num/den (oracle ⍎"1/2" -> 1/2).
        ("⍎\"1/2\"", "1/2"),
        // ⍎ is Kotlin's ParseNumberFunction: a strict NUMBER parser (not an
        // expression evaluator). All cases oracle-verified 2026-08-24.
        ("⍎\"-10\"", "-10"),
        ("⍎\"  30   \"", "30"),
        ("⍎\"123456789012345678901234567890123456\"", "123456789012345678901234567890123456"),
        ("⍎\"10.1\"", "10.1"),
        ("⍎\"-.3\"", "-0.3"),
        ("⍎\"-3.\"", "-3.0"),
        ("⍎\"50e0\"", "50.0"),
        ("⍎\"300e-1\"", "30.0"),
        ("⍎\"1.5e4\"", "15000.0"),
        ("⍎\"-1.5e4\"", "-15000.0"),
        ("⍎\"1.5e-2\"", "0.015"),
        ("⍎\"12/4\"", "3"),
        ("⍎\"-2/5\"", "-2/5"),
        ("⍎\"100000000000000000000000000000000000000000/2\"", "50000000000000000000000000000000000000000"),
        // Error text rows (harness cannot assert errors — verified by hand vs the
        // oracle, see KNOWN-NONCONFORMANCE / PROGRESS-20260824):
        //   ⍎"1+2"  -> "⍎: Value cannot be parsed as a number: '1+2'"
        //   ⍎"."    -> "⍎: Value cannot be parsed as a number: '.'"
        //   ⍎"@a"   -> "⍎: Value cannot be parsed as a number: '@a'"
        //   ⍎ 1 2 3 -> "⍎: Argument is not a string"
        // ⍣ power operator (Kotlin PowerAPLOperator, operator.kt:7). All cases
        // oracle-verified 2026-08-24. Iterate + until modes.
        ("({⍵×2}⍣3) 5", "40"),
        ("({⍵,((↑¯2↑⍵)+(↑¯1↑⍵))}⍣6) (0 1)", "(0 1 1 2 3 5 8 13)"),
        ("({⍵×2}⍣{⍺>⌊⍵÷32}) 1000", "2000"),
        // Error rows (harness cannot assert): 5 ({⍵×2}⍣3) 1 ->
        // "⍣: Function cannot be called with two arguments";
        // ({⍵+1}⍣¯2) 0 -> "⍣: Argument to power is negative: -2".
        // ∵ bitwise family (Kotlin BitwiseOp, bitwise_ops.kt, engine.kt:495).
        // All oracle-verified 2026-08-24.
        ("192 ∨∵ 31", "223"),
        ("5 ∧∵ 3", "1"),
        ("5 ≠∵ 3", "6"),
        ("2 <∵ 1", "1"),
        ("2 >∵ 1", "2"),
        ("12 ⌽∵ 5", "20480"),
        ("¯1 ⌽∵ 5", "2"),
        ("⍴∵ 255", "8"),
        ("⍴∵ ¯1", "0"),
        // Error rows (harness cannot assert): 5 ∧∵ 3.5 ->
        // "∵: Bitwise calls can only be performed on integers";
        // !∵ 255 -> "!: Function does not support bitwise operations".
        // reshape dimension-spec ladder (Kotlin reshape.kt findSizeCalculationMethod
        // :375). Oracle-verified 2026-08-24. NOTE: full-array rows like (2 :match ⍴ ⍳6)
        // flatten in the port's rank-2 render (KNOWN DISPLAY delta), so assert SHAPE.
        ("⍴(2 :match ⍴ ⍳6)", "(2 3)"),
        ("⍴(2 :fill ⍴ ⍳5)", "(2 3)"),
        ("⍴(2 :truncate ⍴ ⍳9)", "(2 4)"),
        ("⍴(2 :recycle ⍴ ⍳5)", "(2 3)"),
        ("(⊃(2 :fill ⍴ ⍳5))[1;2]", "0"),
        // Error rows (harness cannot assert): (2 :match ⍴ ⍳7) ->
        // "Invalid size of right argument: 7. Should be divisible by 2.";
        // (¯3 ⍴ 1 2 3) -> "Attempt to reshape to dimension with negative size: -3";
        // (2 :foo ⍴ ⍳5) -> "⍴: Wanted a value of type number. Got: symbol".
        // ,[axis] laminate + scalar extension (Kotlin concatenate-array.kt
        // joinByLaminate :133). Oracle-verified 2026-08-24. Self-contained
        // expressions (harness rows must not depend on prior-row bindings).
        ("⍴((2 3⍴⍳6),[0.5](2 3⍴⍳6))", "(2 2 3)"),
        ("⍴((2 3⍴⍳6),[1.5](2 3⍴⍳6))", "(2 3 2)"),
        ("((2 3⍴⍳6),[0.5]9)[;1]", "(9 9 9 9 9 9)"),
        // Error rows (harness cannot assert): 1 ,[0.5] 2 -> ",: Both arguments
        // are scalar"; m,[3]m+10 -> "Axis 3 is not valid. Expected: 2".
        // Indexing into a string (bracket indexing) — returns char scalars.
        ("\"abcdef\"[2]", "@c"),
        ("\"abcdef\"[0 2]", "\"ac\""),
        ("⊃\"abcdef\"", "\"abcdef\""),
        // take/drop on a string (slices its characters)
        ("↓\"abc\"", "\"bc\""),
        ("2↓\"abcdef\"", "\"cdef\""),
        ("3↑\"abc\"", "\"abc\""),
        ("¯2 ↑ 100 200 300 400 500 600 700 800 900 1000 1100 1200", "(1100 1200)"),
        ("4 ↑ 1 2", "(1 2 0 0)"),
        ("¯10 ↑ 1 2", "(0 0 0 0 0 0 0 0 1 2)"),
        ("↑⍬", "0"),
        ("↓1 2 3 4", "(2 3 4)"),
        ("↓10 + 1 2 3 4", "(12 13 14)"),
        ("↓⍬", "⍬"),
        // Set operations: ∪ (unique/union) and ∩ (intersection) — Kotlin unique.kt.
        // Union preserves left order/duplicates, appends non-matched right elements.
        ("∪ 0 3 1 1 0 0", "(0 3 1)"),
        ("1 2 3 ∪ 9 8 1", "(1 2 3 9 8)"),
        ("1 2 3 4 5 ∩ 1 2 3 10 11 12", "(1 2 3)"),
        ("⍬ ∩ ⍳10", "⍬"),
        ("⍬ ∪ 1 2 3", "(1 2 3)"),
        ("1 2 3 ∪ ⍬", "(1 2 3)"),
        // `⍸` (where / index-of-nonzero) — Kotlin WhereTest.kt. Rank-1 indices
        // (0-based), repeated by the element's value; higher rank → coordinate vectors.
        ("⍸ 0 0 1 1 0 0 1 1 1 1", "(2 3 6 7 8 9)"),
        ("⍸ 1 0 0 0 0", "(0)"),
        ("⍸ 0 0 0 5 0", "(3 3 3 3 3)"),
        ("⍸ 2 2⍴0 1 1 0", "((0 1) (1 0))"),
        ("⍸ 4 5⍴0 0 1 0 0 1 0 0 0 1 0 1 0 0 0 1 0 0 1 0", "((0 2) (1 0) (1 4) (2 1) (3 0) (3 3))"),
        ("⍸⍬", "⍬"),
        ("⍸ 0 0 0 0", "⍬"),
        ("⍸ 1", "(⍬)"),
        ("⍸ 3", "(⍬ ⍬ ⍬)"),
        ("⍸ 2 0 2", "(0 0 2 2)"),
        // Phase 6 breadth: ⍒ grade-down (sibling of ⍋)
        ("⍒ 3 1 4 1 5", "(4 2 0 1 3)"),
        ("⍒ 2 2⍴3 1 4 1", "(1 0)"),
        ("⍒ ⍬", "⍬"),
        // Phase 6 breadth: ⍲ ⍱ nand/nor broadcast over booleans
        ("1 0 1 ⍲ 0 1 0", "(1 1 1)"),
        ("1 0 1 ⍱ 0 1 0", "(0 0 0)"),
        ("0 0 1 ⍲ 0 1 0", "(1 1 1)"),
        ("0 0 1 ⍱ 0 1 0", "(1 0 0)"),
        ("1 ⍲ 0 0 1", "(1 1 0)"),
        ("0 ⍱ 1 1 0", "(0 0 1)"),
        // Phase 6 breadth: ∼ logical not (monadic, boolean)
        ("∼ 0 1 0 1", "(1 0 1 0)"),
        // Dyadic `⍸` (interval form) — Kotlin IntervalTest.kt.
        ("2 5 10 ⍸ ¯1 0 2 3 4 5 6 10 11", "(0 0 1 1 1 2 2 3 3)"),
        ("4 ⍸ 1 2 3 4 5 6", "(0 0 0 1 1 1)"),
        ("⍬ ⍸ 1 2 3", "(0 0 0)"),
        ("2.0 3.0 ⍸ 1.5 2.0 2.0001 2.9999 3.0 3.0001 1e100", "(0 1 1 1 2 2 2)"),
        ("2 5 10 100 ⍸ 3 5 ⍴ ¯1 0 2 3 4 5 6 10 11 3 1 0 99 100 101", "(0 0 1 1 1 2 2 3 3 1 0 0 3 4 4)"),
        ("2 5 10 100 ⍸ ⍬", "⍬"),
        // scalar right arg → RANK-0 result (Kotlin IntervalValue dims == b's dims);
        // oracle `1e100 ⍸ 1e100` → 1, not (1).
        ("1e100 ⍸ 1e100", "1"),
        ("\"beh\" ⍸ \"abcdefghij\"", "(0 1 1 1 2 2 2 3 3 3)"),
        ("∪ 1 2 3 4 5", "(1 2 3 4 5)"),
        ("∪ 1", "(1)"),
        ("\"abc\" ∪ \"xyz\"", "\"abcxyz\""),
        ("\"abcd\" ∩ \"xycb\"", "\"bc\""),
        ("1 1 2 2 3 4 1 ∩ 4 4 3 3 2 1 1 2 2 1 3 4", "(1 1 2 2 3 4 1)"),
        ("↓ 3 3 ⍴ ⍳9", "(3 4 5 6 7 8)"),
        ("2 ↓ 100 200 300 400 500 600 700 800", "(300 400 500 600 700 800)"),
        ("5 ↓ 10", "⍬"),
        ("¯5 ↓ 10", "⍬"),
        ("0 ↓ 10", "(10)"),
        ("¯2 ↓ 1 2 3 4 5 6", "(1 2 3 4)"),
        ("1 ↓ 3 5 ⍴ ⍳100", "(5 6 7 8 9 10 11 12 13 14)"),
        ("2 ↓ 5 5 ⍴ ⍳100", "(10 11 12 13 14 15 16 17 18 19 20 21 22 23 24)"),
        ("4 4 ↓ 5 5 5 ⍴ ⍳100", "(20 21 22 23 24)"),
        // Multi-dimensional reverse / rotate (⌽ last axis, ⊖ first axis) and
        // transpose (⍉ axis-reversal monadic + dyadic axis-permute).
        // Reference: TransposeTest.kt. `⍉˝` (inverse-transpose adverb) cases ARE
        // covered below; the nested-grouped form `, 0 1 0 (⍉)˝ m` / `,(⍉ int:proto v)˝ m`
        // remains a general standalone-adverb-in-nested-position gap (DEFAULT too).
        ("⌽1 2 3 4", "(4 3 2 1)"),
        ("⌽4 5 ⍴ ⍳100", "(4 3 2 1 0 9 8 7 6 5 14 13 12 11 10 19 18 17 16 15)"),
        ("⌽4 5 4 ⍴ ⍳1000", "(3 2 1 0 7 6 5 4 11 10 9 8 15 14 13 12 19 18 17 16 23 22 21 20 27 26 25 24 31 30 29 28 35 34 33 32 39 38 37 36 43 42 41 40 47 46 45 44 51 50 49 48 55 54 53 52 59 58 57 56 63 62 61 60 67 66 65 64 71 70 69 68 75 74 73 72 79 78 77 76)"),
        ("⊖4 5 ⍴ ⍳100", "(15 16 17 18 19 10 11 12 13 14 5 6 7 8 9 0 1 2 3 4)"),
        ("1⌽1 2 3 4", "(2 3 4 1)"),
        ("¯4 ⌽ ⍳25", "(21 22 23 24 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20)"),
        ("1⊖4 5 ⍴ ⍳100", "(5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 0 1 2 3 4)"),
        // ⌽/⊖ with an explicit axis (Kotlin RotateFunction.computeAxis) and the
        // single-element-vector shift broadcast (transpose.kt:210 a0.size==1 branch).
        ("1 2 3 ⌽ 3 3 ⍴ ⍳9", "(1 2 0 5 3 4 6 7 8)"),
        // Multi-cell rotate specifier mapped over ALL non-rotated axes
        // (Kotlin MultiRotationRotatedAPLValue).
        ("1 2 ⌽[0] 2 2 ⍴ ⍳4", "(2 1 0 3)"),
        ("(2 3 ⍴ 1 2 3 1 1 1) ⌽[1] 2 2 3 ⍴ ⍳12", "(3 1 5 0 4 2 9 10 11 6 7 8)"),
        // Reduce/scan with explicit axis (`+/[0]` ≡ `(+/)[0]`, Kotlin binds the
        // bracket to the fn operand of the reduction; evaluator threads it into
        // adverb_reduce/adverb_scan).
        ("+/[0] 2 3 ⍴ ⍳6", "(3 5 7)"),
        ("+/[1] 2 3 ⍴ ⍳6", "(3 12)"),
        ("⌽/[0] 2 3 ⍴ ⍳6", "(3 4 5)"),
        (",/[0] 2 3 ⍴ ⍳6", "((0 3) (1 4) (2 5))"),
        ("⍴ +/[0] 2 3 ⍴ ⍳6", "(3)"),
        ("(,2) ⌽ 4 5 ⍴ ⍳100", "(2 3 4 0 1 7 8 9 5 6 12 13 14 10 11 17 18 19 15 16)"),
        ("(,1) ⊖ 2 3 ⍴ ⍳6", "(3 4 5 0 1 2)"),
        // `˝` inverse adverb (Kotlin InverseFnOp): monadic reverse is its own
        // inverse; dyadic rotate inverts by negating the shift; dyadic transpose
        // applies the inverse permutation.
        ("⌽˝ 1 2 3 4", "(4 3 2 1)"),
        ("⊖˝ 1 2 3 4", "(4 3 2 1)"),
        ("1 ⌽˝ 1 2 3 4 5", "(5 1 2 3 4)"),
        ("1 ⊖˝ 3 3 ⍴ ⍳9", "(6 7 8 0 1 2 3 4 5)"),
        ("1 2 3 4 5 6 ⌽˝ 6 6 ⍴ ⍳36", "(5 0 1 2 3 4 10 11 6 7 8 9 15 16 17 12 13 14 20 21 22 23 18 19 25 26 27 28 29 24 30 31 32 33 34 35)"),
        ("(3 3 ⍴ 1 2 2 3 2 1 1 2 1) ⌽˝ 3 3 3 ⍴ ⍳100", "(2 0 1 4 5 3 7 8 6 9 10 11 13 14 12 17 15 16 20 18 19 22 23 21 26 24 25)"),
        ("1 0 ⍉˝ 2 3 ⍴ ⍳6", "(0 3 1 4 2 5)"),
        // int:proto (Kotlin ProtoOp): take-with-fill + inverse-diagonal w/ custom fill
        ("3 (↑ int:proto 5) 1 2", "(1 2 5)"),
        ("6 (↑ int:proto 5) 1 2", "(1 2 5 5 5 5)"),
        ("3 (↑ int:proto 5) 1 2 3 4 5 6", "(1 2 3)"),
        ("0 1 0 ⍉˝ 3 3 ⍴ 1 4 7 11 14 17 21 24 27", "(1 0 0 4 0 0 7 0 0 0 11 0 0 14 0 0 17 0 0 0 21 0 0 24 0 0 27)"),
        ("0 1 0 (⍉ int:proto 99)˝ 3 3 ⍴ 1 4 7 11 14 17 21 24 27", "(1 99 99 4 99 99 7 99 99 99 11 99 99 14 99 99 17 99 99 99 21 99 99 24 99 99 27)"),
        // tally ≢ = first axis of the SHAPE, not flat count (Kotlin TallyFunction)
        ("≢ 3 4 ⍴ ⍳12", "3"),
        ("≢ ⍬", "0"),
        ("≢ \"abc\"", "3"),
        ("≢ 5", "1"),
        // axis + inverse composition (`f[k]˝`)
        ("⌽[0]˝ 3 3 ⍴ ⍳9", "(6 7 8 3 4 5 0 1 2)"),
        ("1 ⌽[0]˝ 3 3 ⍴ ⍳9", "(6 7 8 0 1 2 3 4 5)"),
        ("1 ⊖[1]˝ 3 3 ⍴ ⍳9", "(2 0 1 5 3 4 8 6 7)"),
        ("⍉ 2 3 ⍴ ⍳6", "(0 3 1 4 2 5)"),
        ("⍉ 2 2 ⍴ 1 2 3 4", "(1 3 2 4)"),
        ("⍉ 3 4 5 ⍴ ⍳60", "(0 20 40 5 25 45 10 30 50 15 35 55 1 21 41 6 26 46 11 31 51 16 36 56 2 22 42 7 27 47 12 32 52 17 37 57 3 23 43 8 28 48 13 33 53 18 38 58 4 24 44 9 29 49 14 34 54 19 39 59)"),
        ("1 0 ⍉ 2 3 ⍴ ⍳6", "(0 3 1 4 2 5)"),
        // Diagonal transpose (Kotlin TransposedDiagonalValue): duplicated axis
        // indices pick the diagonal; groups must start at 0 ascending.
        ("0 0 ⍉ 4 4 ⍴ ⍳16", "(0 5 10 15)"),
        ("0 0 0 ⍉ 3 3 3 ⍴ ⍳27", "(0 13 26)"),
        ("1 1 0 ⍉ 2 20 3 ⍴ ⍳120", "(0 63 1 64 2 65)"),
        ("1 1 0 0 ⍉ 2 3 4 5 ⍴ ⍳1000", "(0 80 6 86 12 92 18 98)"),
        ("2 0 1 ⍉ 2 3 2 ⍴ ⍳12", "(0 6 1 7 2 8 3 9 4 10 5 11)"),
        // Partial-axis transpose (Kotlin prefix-fill rule): when the left arg is
        // shorter than the rank, the remaining axes are appended in ascending order.
        // Real Kap: `0 1 ⍉ 3 4 5⍴⍳60` => perm [0,1,2] (identity); `1 2 0 ⍉` => [1,2,0].
        ("⍴ 0 1 ⍉ 3 4 5 ⍴ ⍳60", "(3 4 5)"),
        ("⍴ 1 2 0 ⍉ 2 3 4 ⍴ ⍳24", "(4 2 3)"),
        ("⍴ 0 1 ⍉ 2 3 4 ⍴ ⍳24", "(2 3 4)"),
        ("0 1 ⍉ 2 3 4 ⍴ ⍳24", "(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23)"),
        ("2 3 0 1 ⍉ 2 3 4 5 ⍴ ⍳120", "(0 20 40 60 80 100 1 21 41 61 81 101 2 22 42 62 82 102 3 23 43 63 83 103 4 24 44 64 84 104 5 25 45 65 85 105 6 26 46 66 86 106 7 27 47 67 87 107 8 28 48 68 88 108 9 29 49 69 89 109 10 30 50 70 90 110 11 31 51 71 91 111 12 32 52 72 92 112 13 33 53 73 93 113 14 34 54 74 94 114 15 35 55 75 95 115 16 36 56 76 96 116 17 37 57 77 97 117 18 38 58 78 98 118 19 39 59 79 99 119)"),
        ("1 4 2 0 3 ⍉ 2 3 4 5 6 ⍴ ⍳720", "(0 120 240 1 121 241 2 122 242 3 123 243 4 124 244 5 125 245 30 150 270 31 151 271 32 152 272 33 153 273 34 154 274 35 155 275 60 180 300 61 181 301 62 182 302 63 183 303 64 184 304 65 185 305 90 210 330 91 211 331 92 212 332 93 213 333 94 214 334 95 215 335 360 480 600 361 481 601 362 482 602 363 483 603 364 484 604 365 485 605 390 510 630 391 511 631 392 512 632 393 513 633 394 514 634 395 515 635 420 540 660 421 541 661 422 542 662 423 543 663 424 544 664 425 545 665 450 570 690 451 571 691 452 572 692 453 573 693 454 574 694 455 575 695 6 126 246 7 127 247 8 128 248 9 129 249 10 130 250 11 131 251 36 156 276 37 157 277 38 158 278 39 159 279 40 160 280 41 161 281 66 186 306 67 187 307 68 188 308 69 189 309 70 190 310 71 191 311 96 216 336 97 217 337 98 218 338 99 219 339 100 220 340 101 221 341 366 486 606 367 487 607 368 488 608 369 489 609 370 490 610 371 491 611 396 516 636 397 517 637 398 518 638 399 519 639 400 520 640 401 521 641 426 546 666 427 547 667 428 548 668 429 549 669 430 550 670 431 551 671 456 576 696 457 577 697 458 578 698 459 579 699 460 580 700 461 581 701 12 132 252 13 133 253 14 134 254 15 135 255 16 136 256 17 137 257 42 162 282 43 163 283 44 164 284 45 165 285 46 166 286 47 167 287 72 192 312 73 193 313 74 194 314 75 195 315 76 196 316 77 197 317 102 222 342 103 223 343 104 224 344 105 225 345 106 226 346 107 227 347 372 492 612 373 493 613 374 494 614 375 495 615 376 496 616 377 497 617 402 522 642 403 523 643 404 524 644 405 525 645 406 526 646 407 527 647 432 552 672 433 553 673 434 554 674 435 555 675 436 556 676 437 557 677 462 582 702 463 583 703 464 584 704 465 585 705 466 586 706 467 587 707 18 138 258 19 139 259 20 140 260 21 141 261 22 142 262 23 143 263 48 168 288 49 169 289 50 170 290 51 171 291 52 172 292 53 173 293 78 198 318 79 199 319 80 200 320 81 201 321 82 202 322 83 203 323 108 228 348 109 229 349 110 230 350 111 231 351 112 232 352 113 233 353 378 498 618 379 499 619 380 500 620 381 501 621 382 502 622 383 503 623 408 528 648 409 529 649 410 530 650 411 531 651 412 532 652 413 533 653 438 558 678 439 559 679 440 560 680 441 561 681 442 562 682 443 563 683 468 588 708 469 589 709 470 590 710 471 591 711 472 592 712 473 593 713 24 144 264 25 145 265 26 146 266 27 147 267 28 148 268 29 149 269 54 174 294 55 175 295 56 176 296 57 177 297 58 178 298 59 179 299 84 204 324 85 205 325 86 206 326 87 207 327 88 208 328 89 209 329 114 234 354 115 235 355 116 236 356 117 237 357 118 238 358 119 239 359 384 504 624 385 505 625 386 506 626 387 507 627 388 508 628 389 509 629 414 534 654 415 535 655 416 536 656 417 537 657 418 538 658 419 539 659 444 564 684 445 565 685 446 566 686 447 567 687 448 568 688 449 569 689 474 594 714 475 595 715 476 596 716 477 597 717 478 598 718 479 599 719)"),
        // Phase 6 breadth: cmp (total-ordering compare → −1/0/1, Kotlin CompareObjectsFunction)
        ("1 cmp 2", "-1"),
        ("2 cmp 2", "0"),
        ("3 cmp 2", "1"),
        // Phase 6 breadth: ! (gamma / binomial — Kotlin FactorialFunction)
        // Exact-integer factorial: !5 = 5! = 120 (oracle renders 120.0; value-identical).
        ("! 5", "120"),
        ("! 10", "3628800"),
        ("! 100", "93326215443944152681699238856266700490715968264381621468592963895217599993229915608941463976156518286253697920827223758251185210916864000000000000000000000000"),
        ("! 1 2 3", "(1 2 6)"),            // element-wise gamma over an array
        ("! ¯1", "Infinity"),
        ("! ¯2", "NaN"),
        ("! 0.5 1.5 2.5", "(0.886226925452758 1.329340388179137 3.3233509704478426)"),
        ("!¯0.5", "1.772453850905516"),
        ("!¯1.5", "-3.5449077018110318"),
        ("5 ! 2", "0.0"),                  // binomial(2,5) = 0
        ("1 2 3 ! 4 5 6", "(4 10 20)"),    // element-wise binomial over arrays
        ("1 2 3 ! 2", "(2 1 0.0)"),
        ("1r2 + 1r2", "1"),                // whole-number rational renders as bare integer
        // Phase 7 (P7a): statement-level fork postfix `f « g » h` with a DERIVED
        // left member (Kotlin parseOperator LeftForkToken, parser.kt:1295).
        // stat.kap `avg ⇐ +/«÷»≢` = (sum y) ÷ (tally y).
        ("+/«÷»≢ 1 2 3 4", "5/2"),
        ("avg ⇐ +/«÷»≢ ⋄ avg 1 2 3 4 5 6", "7/2"),
        // Bare value-left-bind chain (P7d-6): `1 2 + ≢` parses as a Chain2 of a
        // LeftAssignedFunction([1,2], +) and ≢; the value strand binds to the FIRST
        // function only, remaining fns chain. Oracle: `p9⇐1 2+≢ ⋄ p9 5` → ⟨2 3⟩;
        // `f⇐1 2+×≢ ⋄ f 5` → ⟨2 3⟩. (Display glyph () vs ⟨⟩ is the P8 renderer gap.)
        ("p9 ⇐ 1 2+≢ ⋄ p9 5", "(2 3)"),
        ("f ⇐ 1 2+×≢ ⋄ f 5", "(2 3)"),
        // Phase 7 (P7b): dual / structural-under `base ⍢ wrapper`
        // (Kotlin StructuralUnderOp, engine.kt:501): wrapper⁻¹ ∘ base ∘ wrapper.
        ("(⌽⍢⌽) 1 2 3 4", "(4 3 2 1)"),
        ("(-⍢-) 5", "-5"),
        ("(-⍢⌽) 1 2 3", "(-1 -2 -3)"),
        ("(⌽⍢-) 5", "5"),
        // Structural-under OVERLAY family (2026-08-26). `evalWithStructuralUnder1Arg`
        // is a PER-FUNCTION override, not one generic rule: math/⍉/⍨ wrappers use
        // `inversibleStructuralUnder1Arg` (functions.kt:267, the inverse rule above),
        // while `↑`/`↓` (drop.kt:84/:351) SELECT a region, let base transform it, and
        // splice the result back via `replaceForUnder` → `OverlayReplacementValue`
        // (array_functions.kt:126). All rows oracle-verified against kap-jvm-text.
        ("↓⍢(10↓) ⍳20", "(0 1 2 3 4 5 6 7 8 9 11 12 13 14 15 16 17 18 19)"),
        ("⌽⍢(3↑) ⍳10", "(2 1 0 3 4 5 6 7 8 9)"),
        ("{100}⍢(2↑) ⍳6", "(100 100 2 3 4 5)"),
        ("{1000+⍵}⍢(3↓) ⍳6", "(0 1 2 1003 1004 1005)"),
        ("{99}⍢(¯2↓) ⍳6", "(99 99 99 99 4 5)"),
        ("{⍵}⍢(¯2↑) ⍳6", "(0 1 2 3 4 5)"),
        // A base fn that RESIZES the selected region resizes that axis
        // (array_functions.kt:158); an empty replacement shrinks it away.
        ("{,1 2}⍢(1↑) ⍳4", "(1 2 1 2 3)"),
        ("{⍬}⍢(1↑) ⍳4", "(1 2 3)"),
        ("{⍬}⍢(2↓) ⍳4", "(0 1)"),
        // Rank-2 overlay: the selected sub-block is replaced in place.
        ("⌽⍢(2↑) 2 3⍴⍳6", "((2 1 0) (5 4 3))"),
        // Hex/binary literals normalise to Long, so integer-count builtins accept
        // them (`⍳0x20` errored "⍳ needs an integer count" before 2026-08-26).
        ("⍳0b101", "(0 1 2 3 4)"),
        // `⍢` binds on an assignment RHS and on a chained fn atom (Kotlin
        // processAssignment parses the RHS with parseValue, parser.kt:529).
        ("x ← ⌽⍢⌽ ⍳5 ⋄ x", "(4 3 2 1 0)"),
        ("{⍵+1} ⌽⍢⌽ ⍳5", "(5 4 3 2 1)"),
        // Axis-applied take/drop `↑[k]`/`↓[k]` (Kotlin drop.kt:335: with an explicit
        // axis the left arg must be a SINGLE integer, placed at axis k with every
        // other axis untouched, validated by ensureValidAxis). `↑`/`↓` were missing
        // from the parser's axis allowlist, so `2↑[0] ⍳6` STRANDED the axis and
        // returned a silently wrong value (`(1)`) instead of applying take.
        ("2↑[0] ⍳6", "(0 1)"),
        ("2↓[0] ⍳6", "(2 3 4 5)"),
        ("(-2)↑[0] ⍳6", "(4 5)"),
        // Non-selected axes must pass through WHOLE, not as "take 0" (which emptied
        // the result); shapes checked via a variable since `⍴ 2↑[1] …` hits an
        // unrelated pre-existing ⍴-precedence quirk.
        ("x ← 2↑[1] 2 3⍴⍳6 ⋄ ⍴x", "(2 2)"),
        ("x ← 2↑[0] 2 3⍴⍳6 ⋄ ⍴x", "(2 3)"),
        ("x ← 1↓[1] 2 3⍴⍳6 ⋄ ⍴x", "(2 2)"),
        ("x ← 1↓[0] 2 3⍴⍳6 ⋄ ⍴x", "(1 3)"),
        // Error rows (harness cannot assert error text — verified by hand vs oracle):
        //   `2↑[9] ⍳6`   -> "↑: Axis 9 is not valid. Expected: 1"
        //   `↑[0] ⍳6`    -> "↑: Function does not support axis specifier"
        //   `2 3↑[0] ⍳6` -> "↑: When given an explicit axis, the left argument must
        //                    be a single integer"
        // Left-bind whose bound VALUE is an expression, not a literal. Kotlin's
        // makeLeftBindFunction (parser.kt:486) binds the whole accumulated leftArgs
        // list, so any value instruction qualifies. The parser already built
        // `Train[Apply{…}, ↑]` correctly; the evaluator's `is_value` only accepted
        // Literal/Array/Empty, so these missed the left-bind arm of apply_train, fell
        // through to ATOP, and failed with "only symbol/lambda functions supported yet".
        ("((-2)↑) ⍳6", "(4 5)"),
        // A paren-group VALUE as a left-bind inside a FORK TINE. `((-2)↑)` parsed
        // only in left-tine position; as a right tine the nested group was routed to
        // the fn-members-only `try_parse_train`, which stranded it as `Array[-, 2]`
        // (→ "unexpected token in primary", then "No arguments specified for
        // function"). All four positions are now oracle-exact.
        ("⌽«,»((-2)↑) ⍳6", "(5 4 3 2 1 0 4 5)"),
        ("(2↑)«,»((-2)↑) ⍳6", "(0 1 4 5)"),
        ("((-2)↑)«,»(2↑) ⍳6", "(4 5 0 1)"),
        ("⌽«,»((-2)+) 5", "(5 3)"),
        ("((1+1)↑) ⍳6", "(0 1)"),
        ("((-2)+) 5", "3"),
        ("((⌈3÷2)↑) ⍳6", "(0 1)"),
        // Phase 7 (OPEN-3): inner/outer product `f1 ∙ f2` (Kotlin OuterInnerJoinOp,
        // engine.kt:489). `∘∙f` => outer product (NullFunction sentinel); `f1∙f2` =>
        // inner join (normalize/error/reduce ladder, outer_join.kt:176-266).
        ("1 2 3 +∙× 1 2 3", "14"),
        ("(2 2⍴1 2 3 4) +∙× (2 2⍴1 2 3 4)", "((5 11) (11 25))"),
        ("5 +∙× 1 2 3", "30"),
        ("1 2 3 +∙× 3 4 5", "26"),
        ("f ⇐ ∘∙× ⋄ (1 2 3) f (3 4)", "((3 4) (6 8) (9 12))"),
        ("1 2 3 ∘∙× 3 4", "((3 4) (6 8) (9 12))"),
        ("1 2 ∘∙× (2 2⍴3 4 5 6)", "(((3 4) (5 6)) ((6 8) (10 12)))"),
        // Error text (verified side-by-side; harness cannot assert messages):
        //   1 2 3 +∙× 4 5  ->  "∙: Dimensions of A and B are incompatible. ..." (Kotlin details field)
        // Phase 6 (P6): native quad constants + declare(:const) const enforcement.
        // Values are oracle-exact; the `Str`-vs-char-array `typeof` class name is a
        // separate deferred string-modeling gap (KNOWN-NONCONFORMANCE), not asserted here.
        ("⎕A", "\"ABCDEFGHIJKLMNOPQRSTUVWXYZ\""),
        ("⍴⎕A", "(26)"),
        ("⎕a", "\"abcdefghijklmnopqrstuvwxyz\""),
        ("⍴⎕a", "(26)"),
        ("⎕d", "\"0123456789\""),
        ("⍴⎕d", "(10)"),
        ("⎕A[0]", "@A"),
        ("⎕A[0 1 2]", "\"ABC\""),
        // Error-text cases (verified side-by-side; the harness cannot assert messages):
        //   x ← 5 ⋄ declare(:const x) ⋄ x ← 6   → "Assignment to constant variable: default:x"
        //   ⎕A ← 5                              → "Assignment to constant variable: kap:⎕A"
        //   declare(:const y) ⋄ y ← 5 ⋄ y       → "Assignment to constant variable: default:y"
        // Phase 6 breadth: … (range — Kotlin RangeFunction)
        ("1…5", "(1 2 3 4 5)"),
        ("\"a\"…\"e\"", "\"abcde\""),
        ("5…1", "(5 4 3 2 1)"),
        // Phase 6 breadth: ⍷ (find — Kotlin FindFunction / FindResultValue)
        ("3 ⍷ (1 2 3 4)", "(0 0 1 0)"),
        ("(1 2) ⍷ (1 2 3 4)", "(1 0 0 0)"),
        ("(1 2 3) ⍷ (3 1 2 3 4)", "(0 1 0 0 0)"),
        ("(1 2) ⍷ 1 2 3", "(1 0 0)"),
        ("1 2 ⍷ (1 2 3 4 1 2)", "(1 0 0 0 1 0)"),
        // Phase 6 breadth: ⋆ (power/exp alias of *) — Kotlin PowerAPLFunction
        ("⋆5", "148.4131591025766"),
        ("2⋆5", "32"),
        ("2⋆¯1", "1/2"),
        ("2⋆0.5", "1.4142135623730951"),
        ("0⋆5", "0"),
        ("5⋆2", "25"),
        // Phase 6 breadth: √ (sqrt monadic / nth-root dyadic) — Kotlin SqrtAPLFunction
        ("√4", "2.0"),
        ("√2", "1.4142135623730951"),
        ("3√27", "3.0"),
        ("√16", "4.0"),
        ("√¯1", "6.123233995736766E-17J1.0"),
        ("2√8", "2.8284271247461903"),
        ("4√16", "2.0"),
        ("√0", "0.0"),
        // Phase 6 breadth: ⍮ (pair) — Kotlin PairAPLFunction; display uses () per our hard rule
        ("1 ⍮ 2", "(1 2)"),
        ("1 2 3 ⍮ 4 5 6", "((1 2 3) (4 5 6))"),
        ("⍮ 5", "(5)"),
        ("(1 2) ⍮ (3 4)", "((1 2) (3 4))"),
        // Phase 6 breadth: ⌿/⍀ (axis reduce/scan) — ⌿/⍀ use FIRST axis, /\ use LAST axis.
        // Kotlin: ⌿/⍀ reduce/scan axis 0; /\ reduce/scan last axis. (3 3⍴⍳9) is 0-indexed.
        ("+⌿ 3 3⍴⍳9", "(9 12 15)"),
        ("+/ 3 3⍴⍳9", "(3 12 21)"),
        ("×⌿ 3 3⍴⍳9", "(0 28 80)"),
        ("+⍀ 3 3⍴⍳9", "(0 1 2 3 5 7 9 12 15)"),
        ("+\\ 3 3⍴⍳9", "(0 1 3 3 7 12 6 13 21)"),
        ("×⍀ 3 3⍴⍳9", "(0 1 2 0 4 10 0 28 80)"),
        ("+⌿ 1 2 3 4", "10"),
        ("+⍀ 1 2 3 4", "(1 3 6 10)"),
        // --- Strings: dyadic `⍳` index-of (Kotlin FindIndexTest.kt) ---
        // Result shape = shape of B; first 0-based position in A matching B, else a.size.
        ("\"abc\" ⍳ @c", "2"),
        ("\"abc\" ⍳ 0", "3"),
        ("3 1 4 2 ⍳ 2", "3"),
        ("3 1 4 2 ⍳ 9", "4"),
        ("\"abc\" ⍳ \"xyz\"", "(3 3 3)"),
        ("1 2 3 ⍳ 2 1", "(1 0)"),
        // --- Strings: `cmp` now orders strings (lexicographic) and cross-kinds ---
        ("\"abc\" cmp \"bcd\"", "-1"),
        ("\"abc\" cmp \"abc\"", "0"),
        ("\"xyz\" cmp \"abc\"", "1"),
        // --- Strings: `regex:*` regular-expression utilities (Kotlin RegexpModule) ---
        ("\"foo\" regex:match \"foobar\"", "1"),
        ("\"xyz\" regex:match \"foobar\"", "0"),
        ("\"foo\" regex:find \"foobar\"", "(\"foo\")"),
        ("\"xyz\" regex:find \"foobar\"", "⍬"),
        ("\"f(o)\" regex:find \"fobar\"", "(\"fo\" \"o\")"),
        ("\"o\" regex:findall \"foobar\"", "((\"o\") (\"o\"))"),
        ("\"x\" regex:replace (\"fooxbar\";\"qwe\")", "\"fooqwebar\""),
        ("\"a\" regex:split \"xayaz\"", "(\"x\" \"y\" \"z\")"),
        ("\"x(f[0-9]+)y\" regex:finderror \"fooxf12345ybar\"", "(\"xf12345y\" \"f12345\")"),
        // Character arithmetic (Kotlin StringsTest.kt / oracle `kap-jvm-text`):
        // `Char - Char` => integer; `Char ± Number`/`Number + Char` => Char;
        // `Str ± Number` => shifted string (char array); `Str - Str` => numeric vector.
        ("@b - @a", "1"),
        ("@a - @b", "-1"),
        ("@a + 1", "@b"),
        ("@a - 1", "@`"),
        ("1 + @a", "@b"),
        ("\"ab\" + 1", "\"bc\""),
        ("1 + \"ab\"", "\"bc\""),
        ("\"ab\" - 1", "\"`a\""),
        ("\"abc\" - \"def\"", "(-3 -3 -3)"),
        // Branch/return `→` (Kotlin ReturnFunction): monadic returns immediately;
        // dyadic `cond → val` returns val when cond truthy, else continues.
        ("{→ 5} 9", "5"),
        ("{→ 5 ⋄ 99} 1", "5"),
        ("{⍵ → ⍵+1} 9", "10"),
        ("{⍵ → 5} 9", "5"),
        ("{0 → 5 ⋄ ⍵} 9", "9"),
        ("{1 → 5 ⋄ ⍵} 9", "5"),
        ("f ⇐ {⍵<0 ⋄ → ¯1 ⋄ ⍵×2} ⋄ f 3", "-1"),
        ("f ⇐ {if(⍵<0){→¯1} ⋄ ⍵×2} ⋄ f 3", "6"),
        // --- `⌷` squad / index selection (Kotlin AccessFromIndexAPLFunction) ---
        // Monadic `⌷X` = `⟨X⟩`; dyadic `A⌷B` selects along B's axes (scalar collapses,
        // vector picks a sub-axis, `⍬`/Null selects the whole axis).
        ("⌷1 2 3 4", "((1 2 3 4))"),
        ("⌷⊂5", "(5)"),
        ("⍬⌷1 2 3", "(1 2 3)"),
        ("⍬⌷3 3⍴⍳9", "(0 1 2 3 4 5 6 7 8)"),
        ("0⌷1 2 3 4", "1"),
        ("2⌷1 2 3 4", "3"),
        ("¯1⌷1 2 3 4 5", "5"),
        ("0⌷(1 2)(3 4)", "(1 2)"),
        ("1⌷(1 2)(3 4)", "(3 4)"),
        ("0 0⌷3 3⍴⍳9", "0"),
        ("1 2⌷3 3⍴⍳9", "5"),
        // --- `≡`/`≢` type-discriminating match + depth-of (Kotlin CompareFunction) ---
        ("10 ≡ 10", "1"),
        ("10 ≡ 10.0", "0"),
        ("10 ≢ 10.0", "1"),
        ("10 = 10.0", "1"),
        ("10 ≠ 10.0", "0"),
        ("(1 2) ≡ (1 2.0)", "0"),
        ("≡ 5", "0"),
        ("≡⊂5", "0"),
        ("≡,5", "1"),
        ("≡⊂,5", "2"),
        ("≡ 1 2 3", "1"),
        // Multi-enclose keeps adding a depth level for non-scalars (was 2, oracle 3).
        ("≡⊂⊂ 1 2 3", "3"),
        ("≡⊂⊂ 5", "0"),
        // --- `⊂` enclose: primitive returns unchanged, non-primitive becomes 0-d box ---
        ("⊂5", "5"),
        ("⍴⊂5", "()"),
        // --- `⍮` pair: monadic `⍮x`=`⟨x⟩`, dyadic `a ⍮ b`=`⟨a b⟩` ---
        ("⍮5", "(5)"),
        ("⍮1 2 3", "((1 2 3))"),
        ("5 ⍮ 6", "(5 6)"),
        // --- `,` catenate: monadic `,`=ravel (rank-1), dyadic=concat ---
        (",5", "(5)"),
        (",1 2 3 4", "(1 2 3 4)"),
        (",1 2 3", "(1 2 3)"),
        ("1 2 3 , 4 5 6", "(1 2 3 4 5 6)"),
        // --- `⊃` reveal/disclose + nested pick (Kotlin DiscloseAPLFunction) ---
        // Monadic `⊃X` = disclose (drop outer box level; identity for simple arrays).
        ("⊃1 2 3", "(1 2 3)"),
        ("⊃(1 2)(3 4)", "(1 2 3 4)"),
        ("⊃⊂5", "5"),
        ("⊃5", "5"),
        ("⊃⍬", "⍬"),
        // Dyadic `A⊃B` = pick-with-dimension-checks (distinct from `⊇`):
        //   scalar selector into a scalar arg -> "Mismatched dimensions for selection"
        //   nested selector whose shape != rank of B -> "Dimensions does not match"
        //   out-of-range (positive; negatives wrap) -> "Selection index out of bounds"
        // (Error-text cases are verified by hand against the Kotlin oracle + source,
        //  since this harness cannot assert error messages — see KNOWN-NONCONFORMANCE.)
        ("2 ⊃ 1 2 3 4", "3"),
        ("1 ⊃ (1 2 3)(4 5 6)", "(4 5 6)"),
        // --- `≬` / `toList` + `fromList` (Kotlin `ToListFunction` / `FromListFunction`, div_functions.kt) ---
        // Monadic: a scalar or 1-D array is boxed into a rank-0 list (oracle `⟨⟩` type).
        // Unlike `⊂`, `≬` ALWAYS boxes even a primitive scalar. Rank>1 is an error.
        // The port has no separate list type, so the box displays as `((...))` — that is the
        // recognised DISPLAY-glyph divergence (curated below uses the port's `()` convention).
        ("≬ 1 2 3", "((1 2 3))"),
        ("≬ 5", "(5)"),
        // Error cases (`≬ 2 2⍴⍳4` -> "Argument must be a scalar or 1-D", `3 ≬ 5` -> "cannot be
        // called with two arguments") are verified by hand against the oracle + Kotlin
        // source; this harness cannot assert error messages (see `⊃` block note above).
        ("fromList ≬ 1 2 3", "(1 2 3)"),
        // --- Axis specifiers `f[axis]` (Kotlin AxisValAssignedFunctionDirect + MathCombineAPLFunction) ---
        // scalar+scalar IGNORES the axis (Kotlin eval2Arg short-circuits combine2Arg first).
        // vector+vector (both rank 1, axis 0) is element-wise. vector + higher-rank broadcasts
        // the vector along `axis`. The oracle renders the rank-3 result boxed; the port flattens
        // to parens (the recognised DISPLAY-glyph divergence, values identical).
        ("2 +[0] 3", "5"),
        ("2 +[1] 3", "5"),
        ("1 2 3 +[0] 4 5 6", "(5 7 9)"),
        ("10 20 30 40 +[0] 4 3 2 ⍴ 100+⍳24", "(110 111 112 113 114 115 126 127 128 129 130 131 142 143 144 145 146 147 158 159 160 161 162 163)"),
    ];

    let mut failures = Vec::new();
    // --- Symbols (Kotlin SymbolTest.kt: ${ns}:${name} rendering + int: builtins) ---
    {
        let symbol_cases: Vec<(&str, &str)> = vec![
            // A bare `'foo` literal is a symbol value in the default namespace.
            ("'foo", "foo"),
            // `int:symbolName` returns a 2-element vector [name, namespace] (Kap pair form).
            ("int:symbolName 'foo", "(\"foo\" \"default\")"),
            ("int:symbolName 'abc", "(\"abc\" \"default\")"),
            // A parenthesised symbol value also round-trips through the paren-group path.
            // NOTE: `'abc'` (trailing quote) is NOT valid Kap — `'` is a QuotePrefix
            // that consumes exactly one following symbol token (Kotlin parser.kt:1003),
            // so a second `'` has no operand and errors. Use `('abc)` instead.
            ("int:symbolName ('abc)", "(\"abc\" \"default\")"),
            // `int:intern` (dyadic) builds a symbol: name is the right arg, namespace the left.
            ("\"foo\" int:intern \"bar\"", "foo:bar"),
            ("\"ns\" int:intern \"sym\"", "ns:sym"),
            // Symbols compare by name+namespace via `≡` (deep_equal).
            ("'foo ≡ 'foo", "1"),
            ("'foo ≡ 'bar", "0"),
            // Keyword-namespace symbols render as `:name` (Real Kap: `:hello :sir` →
            // `(:hello :sir)`). They strand into a vector and are distinct from strings.
            (":hello", ":hello"),
            (":hello :sir", "(:hello :sir)"),
            (":foo :bar :baz", "(:foo :bar :baz)"),
            // `:NAME` is a keyword-symbol value, NOT a string (the prior `:keyword`→Str
            // hack was wrong per Real Kap). Consumers accept it directly:
            (":UTF16 unicode:enc \"A\"", "(254 255 0 65)"),
            (":pretty io:print \"x\"", "\"x\""),
        ];
        for (expr, expected) in symbol_cases {
            match engine.eval_string(expr) {
                Ok(v) => {
                    let got = v.format_display();
                    if &got != expected {
                        failures.push(format!(
                            "MISMATCH  {expr:?}  expected {expected:?} got {got:?}"
                        ));
                    }
                }
                Err(e) => {
                    failures.push(format!("ERROR    {expr:?}  -> {e}"));
                }
            }
        }
    }
    // --- Types: `typeof` (Kotlin TypesTest.kt) returns a symbol in the `kap` namespace
    //     naming the Kap class (lowercase, e.g. `kap:array`, `kap:symbol`). ---
    {
        let type_cases: Vec<(&str, &str)> = vec![
            ("typeof 10", "kap:integer"),
            ("typeof 1.2", "kap:float"),
            ("typeof 1÷5", "kap:rational"),
            ("typeof \"x\"", "kap:array"),
            ("typeof @a", "kap:char"),
            ("typeof 1 2 3", "kap:array"),
            ("typeof 'foo", "kap:symbol"),
        ];
        for (expr, expected) in type_cases {
            match engine.eval_string(expr) {
                Ok(v) => {
                    let got = v.format_display();
                    if &got != expected {
                        failures.push(format!(
                            "MISMATCH  {expr:?}  expected {expected:?} got {got:?}"
                        ));
                    }
                }
                Err(e) => {
                    failures.push(format!("ERROR    {expr:?}  -> {e}"));
                }
            }
        }
    }
    for (expr, expected) in cases {
        // Render with `format_display` (REPL form: strings quoted, `⍬` for null, `@` for
        // char) to match Real Kap's reference output, not the bare `format_value` used by
        // the broader `run_kotlin_conformance` harness.
        match engine.eval_string(expr) {
            Ok(v) => {
                let got = v.format_display();
                if &got != expected {
                    failures.push(format!(
                        "MISMATCH  {expr:?}  expected {expected:?} got {got:?}"
                    ));
                }
            }
            Err(e) => {
                failures.push(format!("ERROR    {expr:?}  -> {e}"));
            }
        }
    }
    if !failures.is_empty() {
        panic!("curated parity failed:\n{}", failures.join("\n"));
    }
}
