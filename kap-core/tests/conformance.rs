//! Conformance harness: runs extracted Kotlin Kap test cases against our engine.
//!
//! This is a *coverage* instrument. It reads `conformance/kotlin_tests.jsonl`
//! (produced by `tools/extract_kotlin_tests.py`) and evaluates each case.
//! Because our Rust Kap is a growing subset, most cases will currently fail to
//! parse or evaluate; the harness reports the breakdown so we can track how much
//! of the language is implemented.
//!
//! NOTE: `cargo test` captures a passing test's stdout/stderr and only prints it
//! when the test *fails* or with `-- --nocapture`. To always see the summary,
//! the harness ALSO writes it to `/tmp/conform_summary.txt`. The `conformance_probe`
//! test fails on purpose so the summary is surfaced in the terminal output.
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
        let o = classify(&engine, c);
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

    // We do NOT assert a pass rate here (it grows over time). Instead we FAIL this
    // test on purpose so cargo prints the captured summary above — a passing test's
    // output is suppressed by `cargo test`. The real per-feature pass/fail gate is
    // `curated_kap_parity`.
    panic!(
        "conformance summary (written to {}):\n{s}\n\n(remove the final panic in run_kotlin_conformance to stop the intentional failure)",
        summary_path.display()
    );
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
        ("-(1 + 2)", "¯3"),
        ("+(1 + 2)", "3"),
        ("-(3 1 4)", "(¯3 ¯1 ¯4)"),
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
            "two",
        ),
        ("if (0) { 1 }", "null"),
        // Adverbs (Phase 7)
        ("+/ 1 2 3 4", "10"),
        ("×/ 1 2 3 4", "24"),
        ("+\\ 1 2 3 4", "(1 3 6 10)"),
        ("⌈/ 3 9 2 7", "9"),
        ("⌊/ 3 9 2 7", "2"),
        ("⌈¨ 1.2 2.8 3.5", "(2.0 3.0 4.0)"),
        ("2 ×¨ 3 4 5", "(6 8 10)"),
        ("1 2 3 ×¨ 4 5 6", "(4 10 18)"),
        ("3 ⌈ 5", "5"),
        ("~¨ 1 0 3", "(0 1 0)"),
        // Builtins (ambivalent max/min, arithmetic) — Phase 6
        ("⌈ 3.2", "4.0"),
        ("⌊ 3.8", "3.0"),
        ("3 ⌈ 5", "5"),
        ("3 | 2", "1"),
        ("1 ∧ 0", "0"),
        ("1 ∨ 0", "1"),
        ("~ 1", "0"),
        ("~ 0", "1"),
        ("1 2 3 ∊ 1 2 3 4", "(1 1 1)"), // membership is element-wise (returns vector)
        ("5 ∊ 1 2 3 4", "(0)"),
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
        // Printer note: a length-1 result vector renders as `(x)` here (Kap
        // discloses to scalar `x`); empty-from-scalar renders as `null`.
        ("↑1 2 3 4", "(1)"),
        ("↑6", "(6)"),
        ("5 ↑ 10", "(10 0 0 0 0)"),
        ("0 ↑ 10", "null"),
        ("1 ↑ 10", "(10)"),
        ("2 2 ↑ 10", "(10 0 0 0)"),
        ("2 2 1 3 ↑ 10", "(10 0 0 0 0 0 0 0 0 0 0 0)"),
        ("3 ↑ 10 11 12 13 14 15 16 17", "(10 11 12)"),
        ("2 3 ↑ 10 15 ⍴ ⍳1000", "(0 1 2 15 16 17)"),
        ("1 ↑ 3 3 ⍴ ⍳9", "(0 1 2)"),
        ("3 ↑ 10 2 ⍴ ⍳20", "(0 1 2 3 4 5)"),
        ("4 ↑ 3 2 ⍴ ⍳20", "(0 1 2 3 4 5 0 0)"),
        ("¯2 ↑ 100 200 300 400 500 600 700 800 900 1000 1100 1200", "(1100 1200)"),
        ("4 ↑ 1 2", "(1 2 0 0)"),
        ("¯10 ↑ 1 2", "(0 0 0 0 0 0 0 0 1 2)"),
        ("↑⍬", "0"),
        ("↓1 2 3 4", "(2 3 4)"),
        ("↓10 + 1 2 3 4", "(12 13 14)"),
        ("↓⍬", "null"),
        ("↓ 3 3 ⍴ ⍳9", "(3 4 5 6 7 8)"),
        ("2 ↓ 100 200 300 400 500 600 700 800", "(300 400 500 600 700 800)"),
        ("5 ↓ 10", "null"),
        ("¯5 ↓ 10", "null"),
        ("0 ↓ 10", "(10)"),
        ("¯2 ↓ 1 2 3 4 5 6", "(1 2 3 4)"),
        ("1 ↓ 3 5 ⍴ ⍳100", "(5 6 7 8 9 10 11 12 13 14)"),
        ("2 ↓ 5 5 ⍴ ⍳100", "(10 11 12 13 14 15 16 17 18 19 20 21 22 23 24)"),
        ("4 4 ↓ 5 5 5 ⍴ ⍳100", "(20 21 22 23 24)"),
    ];

    let mut failures = Vec::new();
    for (expr, expected) in cases {
        match engine.eval_to_string(expr) {
            Ok(got) => {
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
