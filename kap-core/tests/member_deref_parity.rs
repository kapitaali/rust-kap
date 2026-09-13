//! Member-dereference (`.`) parity tests vs the JVM oracle.
//!
//! Locks in the 14 T1.2 conformance cases (MemberDereferenceTest in
//! `conformance/kotlin_tests.jsonl:545-580` and related). Test the
//! 5-branch dispatch (array, enclosed, Map, List, fallback) plus the
//! implicit-default-namespace rule for bare symbol literals.

use kap_core::Engine;
use kap_core::APLValue;

fn run_case(expr: &str) -> Result<String, String> {
    let engine = Engine::new();
    match engine.eval_string(expr) {
        Ok(v) => Ok(format_value(&v)),
        Err(e) => Err(format!("{:?}", e)),
    }
}

fn format_value(v: &APLValue) -> String {
    match v {
        // `APLValue::Dynamic` is the DynAssign thunk (a call-by-name closure), not a
        // data value; this helper only formats data, so render it opaquely.
        APLValue::Dynamic { .. } => "<dynamic>".to_string(),
        APLValue::Array(a) => {
            if a.dimensions.is_empty() {
                // rank-0 scalar array — disclose
                if let Some(first) = a.elements().first() {
                    return format_value(first);
                }
                "()".to_string()
            } else {
                // rank-1: emit `(e1 e2 ...)` for human parity
                let mut s = String::from("(");
                for (i, e) in a.elements().iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    s.push_str(&format_value(e));
                }
                s.push(')');
                s
            }
        }
        APLValue::Number(n) => match n {
            kap_core::number::KapNumber::Long(l) => l.to_string(),
            kap_core::number::KapNumber::Double(d) => {
                if d.fract() == 0.0 && d.abs() < 1e15 {
                    format!("{}", *d as i64)
                } else {
                    format!("{}", d)
                }
            }
            kap_core::number::KapNumber::Complex(re, im) => {
                if *im == 0.0 {
                    format!("{}", re)
                } else {
                    format!("{}j{}", re, im)
                }
            }
            other => format!("{:?}", other),
        },
        APLValue::Char(c) => c.to_string(),
        APLValue::Str(s) => format!("\"{}\"", s),
        APLValue::Null => "⍬".to_string(),
        APLValue::Nil => "null".to_string(),
        APLValue::Symbol { name, namespace } => match namespace {
            Some(ns) => format!("{}:{}", ns, name),
            None => format!(":{}", name),
        },
        APLValue::List(_) => "list".to_string(),
        APLValue::Map(_) => "map".to_string(),
        APLValue::Stream(s) => s.borrow().display_name().to_string(),
        APLValue::Process(p) => format!("MPProcess[pid={}]", p.borrow().pid),
        APLValue::Timestamp(ms) => kap_core::time::format_timestamp(*ms),
        APLValue::Jvm(h) => h.borrow().display(),
        APLValue::Lock { .. } => "lock".to_string(),
        APLValue::Condvar { .. } => "condvar".to_string(),
        APLValue::TypedInstance { delegate, .. } => format_value(delegate),
        APLValue::Thread { .. } => "[thread]".to_string(),
        APLValue::SuspendedReturn { .. } => "[return]".to_string(),
        APLValue::SqlConn { url, .. } => format!("Connection(url={})", url),
        APLValue::SqlPrepared { sql, .. } => format!("PreparedStatement({})", sql),
        APLValue::ArrowVec { elems, .. } => format!(
            "[{}]",
            elems.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ")
        ),
        APLValue::Deferred { .. } => "<deferred>".to_string(),
        APLValue::UserFn { .. } => "<fn>".to_string(),
        APLValue::Escape { .. } => "<fn>".to_string(),
        APLValue::NonBoundFn { .. } => "<fn>".to_string(),
        APLValue::UserOp { .. } => "<op>".to_string(),
    }
}

/// Oracle target values for the 14 cases. Some are P8 display-only
/// differences (P8 Option A: `()` vs `⟨⟩`/`┌─┐`); these are noted but
/// still pass because the port's `()` rendering is the house style
/// per ROADMAP §11.
const CASES: &[(&str, &str, &str)] = &[
    // (label, expr, expected_value)
    // 545: map + .ns:name (the namespace-default case)
    ("map ns-name foo.default:test", "foo ← map:with 'test 1 'abc 2 ⋄ foo.default:test", "1"),
    // 546: map + .(sym)
    ("map value-form foo.('test)", "foo ← map:with 'test 1 'abc 2 ⋄ foo.('test)", "1"),
    // 550: call + .ns:name
    ("call+ns (findMap 0).default:foo", "findMap ⇐ { map:with 'foo 10 'bar 20 } ⋄ (findMap 0).default:foo", "10"),
    // 551: string key
    ("string key foo.(\"test\")", "foo ← map:with \"test\" 1 \"abc\" 2 ⋄ foo.(\"test\")", "1"),
    // 553: expression key
    ("expr key a.(2+8)", "a ← map:with 10 \"abc\" 20 \"def\" 30 \"ghi\" ⋄ a.(2+8)", "\"abc\""),
    // 554: nested sequence
    ("ns sequence a.:b.:c.:d", "a ← map:with :a 1 :b (map:with :c (map:with :d 10 :e 20) :f 30 :g 40) :h 50 ⋄ a.:b.:c.:d", "10"),
    // 557: nested numeric
    ("nested numeric a.(10).(300)", "a ← map:with 10 (map:with 100 200 300 400) 20 30 ⋄ a.(10).(300)", "400"),
    // 559: ns sym key (was failing before ns-default fix)
    ("ns sym key a.default:map", "a ← map:with 'map 10 'math:sin 20 ⋄ a.default:map", "10"),
    // 560: stranding (P8 display-only diff accepted per ROADMAP §11)
    ("stranding", "a ← map:with 'foo 10 'bar 20 ⋄ a.default:foo a.default:bar", "(10 20)"),
    // 561: simple array
    ("simple array (10 20 30).(0)", "(10 20 30).(0)", "10"),
    // 562: multi-dim
    ("multi-dim (3 4⍴⍳12).(2 3)", "(3 4 ⍴ ⍳12).(2 3)", "11"),
    // 564: negative index
    ("negative (5 4⍴⍳20).(¯2 2)", "(5 4 ⍴ ⍳20).(¯2 2)", "14"),
    // 565: recursive
    ("recursive", "((10 (20 30) 40 50) 60).(0).(1).(1)", "30"),
    // 566: mixed types (map embedded in array)
    ("mixed types", "(10 (map:with 'foo (30 40 50) 'bar (60 70 80)) 20).(1).default:foo.(2)", "50"),
];

#[test]
fn member_deref_14_cases() {
    let mut pass = 0;
    let mut fail = 0;
    for (label, expr, expected) in CASES {
        let got = match run_case(expr) {
            Ok(s) => s,
            Err(e) => format!("ERR:{}", e),
        };
        if got == *expected {
            pass += 1;
        } else {
            fail += 1;
            eprintln!("  ❌ {}\n     expr:     {}\n     expected: {}\n     got:      {}", label, expr, expected, got);
        }
    }
    assert_eq!(fail, 0, "{}/{} cases failed ({} passed)", fail, CASES.len(), pass);
    assert_eq!(pass, CASES.len());
}

/// Namespace-default edge cases — confirm bare-sym == Some("default") sym.
#[test]
fn symbol_namespace_default_implicit() {
    // Bare-name map key + .ns:name form look up correctly.
    let v = run_case("m ← map:with 'foo 42 ⋄ m.default:foo").unwrap();
    assert_eq!(v, "42", "bare 'foo should match default:foo");
    // Bare-name map key + .(bare-sym) form look up correctly.
    let v = run_case("m ← map:with 'foo 42 ⋄ m.('foo)").unwrap();
    assert_eq!(v, "42", "bare 'foo should match .('foo)");
    // String key distinct from symbol key (string keys and symbol keys
    // are type-discriminated by `values_key_equal`).
    let r = run_case("m ← map:with 'foo 42 ⋄ m.(\"foo\")");
    assert!(r.is_err(), "string key should NOT match bare-sym key (got: {:?})", r);
}

/// T1.2-3b: bare-name member deref matches String-typed map keys.
/// Per Kotlin `MemberDereferenceNameArgumentInstruction` (lookup.kt:78-90),
/// a bare-name form like `m.foo` converts the name to a String before
/// lookup, so it finds a String-typed key. The qualified form
/// `m.default:foo` uses the Symbol form (and a Symbol key, not a
/// String key, must be in the map for it to match).
#[test]
fn member_deref_bare_name_matches_string_key() {
    // Bare name finds the String key.
    let v = run_case("(map:with \"foo\" \"a\" \"bar\" \"hello\").foo").unwrap();
    assert_eq!(v, "\"a\"", "bare-name m.foo should find the String-typed key \"foo\"");
    // Parens form with a String literal also finds the String key.
    let v = run_case("(map:with \"foo\" \"a\" \"bar\" \"hello\").(\"foo\")").unwrap();
    assert_eq!(v, "\"a\"", "m.(\"foo\") should find the String-typed key");
    // Qualified form with `default:foo` does NOT match a String-typed key
    // (the Kotlin else arm at lookup.kt:84 uses the Symbol form).
    let r = run_case("(map:with \"foo\" \"a\" \"bar\" \"hello\").default:foo");
    assert!(r.is_err(), "m.default:foo should NOT match a String-typed key (got: {:?})", r);
    // And the reverse: bare name does NOT match a Symbol-typed key.
    let r = run_case("m ← map:with 'foo 42 ⋄ m.foo");
    assert!(r.is_err(), "bare-name m.foo should NOT match a Symbol-typed key (got: {:?})", r);
}

/// Negative indices work.
#[test]
fn member_deref_negative_index() {
    let v = run_case("(10 20 30 40 50).(¯1)").unwrap();
    assert_eq!(v, "50");
    let v = run_case("(10 20 30 40 50).(¯3)").unwrap();
    assert_eq!(v, "30");
}

/// Multi-axis member with rank-equality.
#[test]
fn member_deref_multi_axis() {
    // (3 4 ⍴ ⍳12) has rank 2, member must be length 2.
    let v = run_case("(3 4 ⍴ ⍳12).(2 3)").unwrap();
    assert_eq!(v, "11");
    // (2 3 4 ⍴ ⍳24) has rank 3, member must be length 3.
    let v = run_case("(2 3 4 ⍴ ⍳24).(1 2 3)").unwrap();
    assert_eq!(v, "23");
}

/// Rank-mismatch (wrong length for rank) → error.
#[test]
fn member_deref_rank_mismatch_errors() {
    // (3 4 ⍴ ⍳12) has rank 2, member length 1 is fine (last-axis pick).
    // Member length 3 with rank 2 array → error.
    assert!(run_case("(3 4 ⍴ ⍳12).(1 2 3)").is_err());
}

/// Enclosed (rank-0) member must be 0.
#[test]
fn member_deref_enclosed_must_be_zero() {
    // (⊂5).0 is the only valid form.
    // Both port and oracle reject the non-zero integer form.
    assert!(run_case("(⊂5).1").is_err());
    assert!(run_case("(⊂5).99").is_err());
}
