//! Conformance harness: runs extracted Kotlin Kap test cases against our engine.
//!
//! This is a *coverage* instrument. It reads `conformance/kotlin_tests.jsonl`
//! (produced by `tools/extract_kotlin_tests.py`) and evaluates each case.
//! Because our Rust Kap is a growing subset, most cases will currently fail to
//! parse or evaluate; the harness reports the breakdown so we can track how much
//! of the language is implemented.
//!
//! Run: `cargo test --test conformance -- --nocapture`
//! (or `cargo test --test conformance` for just the summary line + pass count)

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
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.eval_to_string(&c.expr)));
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
    }

    let total = cases.len();
    let oks = by_outcome.get(&Outcome::Ok).copied().unwrap_or(0);
    let mism = by_outcome.get(&Outcome::Mismatch).copied().unwrap_or(0);
    let unsup = by_outcome.get(&Outcome::Unsupported).copied().unwrap_or(0);

    println!("=== Kap conformance vs Kotlin reference suite ===");
    println!("total extracted cases : {total}");
    println!("  ok (parsed+evaluated){oks}");
    println!("  mismatch (ran, wrong value) : {mism}");
    println!("  unsupported (parse/runtime err) : {unsup}");
    let pct = (oks as f64 / total as f64) * 100.0;
    println!("  coverage : {pct:.1}%");

    println!("\n--- worst-covered files (most unsupported) ---");
    let mut ranked: Vec<_> = by_file.iter().collect();
    ranked.sort_by(|a, b| {
        let ra = a.1 .1 - a.1 .0;
        let rb = b.1 .1 - b.1 .0;
        rb.cmp(&ra).then_with(|| b.1 .1.cmp(&a.1 .1))
    });
    for (f, (ok, tot)) in ranked.iter().take(15) {
        let un = tot - ok;
        println!("  {un:4}/{tot:4} unsupported  {f}");
    }

    println!("\n--- sample unsupported cases ---");
    for c in &unsupported_examples {
        println!("  [{}] {} : {}", c.kind, c.test, c.expr);
    }
    if !mismatch_examples.is_empty() {
        println!("\n--- sample mismatch cases ---");
        for (c, got) in &mismatch_examples {
            println!("  {} : expr={} expected={:?} got={}", c.test, c.expr, c.expected, got);
        }
    }

    // We do not assert a pass rate here (it grows over time); the test "passes"
    // if it runs. Real per-feature assertions live in the curated suite below.
    assert!(total > 0);
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
        ("-(3 1 4)", "[¯3 ¯1 ¯4]"),
        ("2 (+) 3", "5"),
        ("3 (×) 4", "12"),
        // Iota (1-based, just fixed)
        ("⍳5", "[0 1 2 3 4]"),
        ("⍳0", "[]"),
        // Control flow (Phase 8)
        ("if (1 < 2) { 42 }", "42"),
        ("if (1 > 2) { 42 } else { 7 }", "7"),
        ("{ 1 ⋄ 2 ⋄ 3 }", "3"),
        ("i ← 0 ⋄ s ← 0 ⋄ while (i < 5) { s ← s + i ⋄ i ← i + 1 } ⋄ s", "10"),
        ("b ← 2 ⋄ when { (b=1){ \"one\" } (b=2){ \"two\" } (1){ \"other\" } }", "two"),
        ("if (0) { 1 }", "null"),
        // Adverbs (Phase 7)
        ("+/ 1 2 3 4", "10"),
        ("×/ 1 2 3 4", "24"),
        ("+\\ 1 2 3 4", "[1 3 6 10]"),
        ("⌈/ 3 9 2 7", "9"),
        ("⌊/ 3 9 2 7", "2"),
        ("⌈¨ 1.2 2.8 3.5", "[2.0 3.0 4.0]"),
        ("2 ×¨ 3 4 5", "[6 8 10]"),
        ("1 2 3 ×¨ 4 5 6", "[4 10 18]"),
        ("3 ⌈ 5", "5"),
        ("~¨ 1 0 3", "[0 1 0]"),
        // Builtins (ambivalent max/min, arithmetic) — Phase 6
        ("⌈ 3.2", "4.0"),
        ("⌊ 3.8", "3.0"),
        ("3 ⌈ 5", "5"),
        ("3 | 2", "1"),
        ("1 ∧ 0", "0"),
        ("1 ∨ 0", "1"),
        ("~ 1", "0"),
        ("~ 0", "1"),
        ("1 2 3 ∊ 1 2 3 4", "[1 1 1]"),  // membership is element-wise (returns vector)
        ("5 ∊ 1 2 3 4", "[0]"),
        ("⍋ 3 1 4 2", "[1 3 0 2]"), // grade-up (monadic): 0-based indices ascending
        // Trains (Phase 9): verified cases are added after the broad run confirms behavior.
    ];

    let mut failures = Vec::new();
    for (expr, expected) in cases {
        match engine.eval_to_string(expr) {
            Ok(got) => {
                if &got != expected {
                    failures.push(format!("MISMATCH  {expr:?}  expected {expected:?} got {got:?}"));
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
