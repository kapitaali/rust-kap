// Regression tests for `⊂` enclose with string arguments.
// Root cause: Kotlin `APLString` extends `APLArray` (types.kt:1627),
// NOT `APLSingleValue`, so strings must be enclosed like arrays.
// The port wrongly passed `Str` through as a "primitive".
use kap_core::Engine;

fn run(e: &Engine, src: &str) -> Result<String, String> {
    e.eval_to_string(src).map_err(|err| format!("{:?}", err))
}

#[test]
fn enclose_string_basic() {
    let e = Engine::new();
    // `⊂"abc"` encloses the string (oracle `┌─────┐` = 0-D box).
    assert_eq!(run(&e, "⊂\"abc\"").unwrap(), "(abc)");
    // `⊂"a"` encloses a 1-char string.
    assert_eq!(run(&e, "⊂\"a\"").unwrap(), "(a)");
    // `⊂""` encloses an empty string (oracle `┌─┐`).
    assert_eq!(run(&e, "⊂\"\"").unwrap(), "()");
    // Shape of enclosed string is 0-D (empty dims).
    assert_eq!(run(&e, "⍴⊂\"abc\"").unwrap(), "()");
}

#[test]
fn unique_mask_scalar_enclosed() {
    let e = Engine::new();
    // The testWithScalarEnclosed case (UniqueMaskTest.kt:42).
    // `≠ ⊂"abc"` → enclosed string is 1 element → unique mask is `(1)`.
    assert_eq!(run(&e, "≠ ⊂\"abc\"").unwrap(), "(1)");
    // `≠ ⊂5` → enclosed primitive is 1 element → `(1)`.
    assert_eq!(run(&e, "≠ ⊂5").unwrap(), "(1)");
    // `≠ ⊂"a"` → `(1)`.
    assert_eq!(run(&e, "≠ ⊂\"a\"").unwrap(), "(1)");
}

#[test]
fn enclose_string_depth() {
    let e = Engine::new();
    // `≡⊂"abc"` → depth: `"abc"` is 1, `⊂"abc"` wraps → 2.
    assert_eq!(run(&e, "≡⊂\"abc\"").unwrap(), "2");
    // `≡⊂⊂"abc"` → 3.
    assert_eq!(run(&e, "≡⊂⊂\"abc\"").unwrap(), "3");
}

#[test]
fn enclose_primitive_still_passthrough() {
    let e = Engine::new();
    // Numbers and chars still pass through unchanged.
    assert_eq!(run(&e, "⊂5").unwrap(), "5");
    assert_eq!(run(&e, "⊂@a").unwrap(), "a");
    assert_eq!(run(&e, "⊂⍬").unwrap(), "(null)");
}

#[test]
fn enclose_string_disclose() {
    let e = Engine::new();
    // `⊃⊂"abc"` → disclose of enclosed string = back to the string.
    assert_eq!(run(&e, "⊃⊂\"abc\"").unwrap(), "abc");
}
