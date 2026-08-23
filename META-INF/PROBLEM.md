# PROBLEM — `unwindProtect` (and `defsyntax` macros generally) never register via `use()`

**Date:** 2026-08-23
**File under investigation:** `kap-core/src/parser.rs` (`parse_defsyntax_directive`) + `kap-core/src/evaluator.rs` (`eval_string_in_env`, `Instr::DefSyntax` eval arm)
**Test input:** `use("structure.kap")` (structure.kap defines `unwindProtect` via `defsyntax`)

---

## Symptom

After `use("structure.kap")`:

```
when { (2>1){ result ← "a" } }        →  "a"        ✓ works
unwindProtect { io:print "x" } { io:print "y" }  →  error: undefined symbol: unwindProtect
```

`when` works but `unwindProtect` does not — even though **both** are defined identically in
`kap-stdlib/std/structure.kap` via `defsyntax`, and the oracle exposes both after the same `use`.

---

## Root cause (narrowed but NOT yet fixed)

The `Parser` never produces / the `Evaluator` never evaluates the `Instr::DefSyntax` that
`structure.kap` should yield. Instrumentation proves the directive is *parsed* but the
resulting instruction never reaches the registration arm.

### Key evidence (from temporary `eprintln!` debug, still present in tree — see "Debug state")

1. The directive parser IS entered and reads the trigger correctly:
   - `DBG-DEFSYNTRY name="defsyntax" ns=None`  ← entered
   - `DBG-TRIGGER name="unwindProtect" ns=None`  ← trigger read (correct, bare)
   - body tokens `int:unwindProtect`, `statement`, `handler` are also consumed
   - same for `when` / `whenInner` / `defsyntaxsub`.

2. Yet `eval_string_in_env` never sees a `DefSyntax` statement:
   - `DBG-STMT tag=DefSyntax` → **0 occurrences** in a `use("structure.kap")` run.
   - `DBG-STMT tag=Apply` appears only for `use` itself, `namespace("kap")`, then the loop
     stops producing statement tags.
   - `DBG-DEFSYNTAX register:` (in the `Instr::DefSyntax` eval arm, evaluator.rs ~635) → **0
     occurrences**.

3. `when` works ONLY because it is **hardcoded** in `parser.rs:310`
   (`"when" => return Ok(Some(self.parse_when()?))` inside `parse_keyword_prefix`). It does
   NOT use the `defsyntax` mechanism at all. `unwindProtect` has no such hardcoded path, so it
   depends entirely on the `defsyntax` macro machinery — which is broken.

### What this means

`parse_defsyntax_directive` (parser.rs:2418) consumes the whole
`defsyntax unwindProtect (...) { ... }` form (trigger + rules + body) but then does NOT
return `Some(Instr::DefSyntax{..})` into `parse_expr` → `parse_statements`. Either:
- it returns `Ok(None)` (falls through to `parse_assign` on an already-consumed stream →
  `parse_statements` gets `None` → loop breaks), OR
- `parse_syntax_rules()` / `parse_block()` inside it returns an `Err` that is swallowed, OR
- `parse_expr` returns the `DefSyntax` but `parse_statements`/`eval_string_in_env` discards it.

The exact return path is the open question. **The parser must be made to emit `Instr::DefSyntax`
for the `defsyntax` form exactly as it does for the `defsyntaxsub` form (which uses the same
code path at parser.rs:2458).**

---

## Secondary issue (will matter once registration works)

Even after a `DefSyntax` registers into `Engine.macros` (an `Rc<RefCell<HashMap>>`), the
`Parser` only sees a **static clone** taken once per statement:

- `eval_string_in_env` (evaluator.rs:422): `let macros = self.macros.borrow().clone();`
- `Parser.macros` (parser.rs:83) is a plain `HashMap`, seeded from that clone (parser.rs:50).

So a `defsyntax` registered by statement N is NOT visible to the parser of statement N+1 within
the same `use` file, and — more importantly — the live `Engine.macros` is never consulted by
`parse_primary` (parser.rs:2119: `self.macros.get(&name)` reads the cloned snapshot, not the
Engine's live map). The fix is to thread the live `Rc<RefCell<HashMap>>` into the Parser instead
of a clone, or re-snapshot `self.macros` immediately before expanding each macro trigger.

---

## What `unwindProtect` should expand to

`structure.kap` line 3-5:
```kap
defsyntax unwindProtect (:function statement :function handler) {
  int:unwindProtect statement handler
}
```
So `unwindProtect { A } { B }` must expand to `int:unwindProtect (λ{ A }) (λ{ B })` — i.e. each
`:function` rule becomes a `Lambda` taking the `{...}` body. `int:unwindProtect` (a native fn)
is already confirmed working in isolation: `int:unwindProtect { io:print "x" } { io:print "y" }`
prints `yx`.

---

## Reproduce

```bash
cd ~/Apps/array/rust-kap
cargo build -p kap-cli
printf 'use("structure.kap")\nwhen { (2>1){ result ← "a" } }\nunwindProtect { io:print "x" } { io:print "y" }\n' \
  | ./target/debug/kap 2>&1 | grep -v '^Kap 0.0.0' | grep -v '^warning:' | grep -v 'standard library failed to load'
# expected: "a" then "yx" ; actual: "a" then error: undefined symbol: unwindProtect
```

## Oracle (ground truth)

```bash
printf 'use("structure.kap")\nunwindProtect { io:print "x" } { io:print "y" }\n' \
  | ~/Apps/array/kap-jvm-text/bin/kap-jvm-text 2>&1 | grep -aE '⊢ '
# prints yx  (both branches run; handler after statement)
```

---

## Debug instrumentation currently in the tree (REMOVE before committing)

- `parser.rs:2423` `eprintln!("DBG-DEFSYNTRY ...")` inside `parse_defsyntax_directive`
- `parser.rs:2446` `eprintln!("DBG-TRIGGER ...")` after reading trigger name
- `parser.rs:2120` `eprintln!("DBG-MACRO hit/miss ...")` in `parse_primary`
- `evaluator.rs:635` `eprintln!("DBG-DEFSYNTAX register: ...")` in `Instr::DefSyntax` arm
- `evaluator.rs:406` `eprintln!("DBG-EVALINENV mark")` at top of `eval_string_in_env`
- `evaluator.rs:431` `eprintln!("DBG-STMT tag=..")` after `parse_statements`

These are temporary and MUST be removed (via `git checkout` or patch) before any commit.

---

## Suggested next diagnostic step (for whoever picks this up)

Add a single `eprintln!` right after the `parse_defsyntax_directive` call site in `parse_expr`
(parser.rs:234) that prints whether it returned `Some`/`None`/`Err`, and the `pos` before/after.
That will definitively show whether the directive parser returns the `DefSyntax` or bails. The
most likely culprit is `parse_syntax_rules()` (parser.rs:2479) mishandling the
`:function statement :function handler` rule list (e.g. `expect_keyword_var` or the `:` keyword
tokenization), causing an `Err` that propagates out of `parse_expr` via `?` — which would
explain why `eval_string_in_env` shows no `DBG-STMT` for these lines (the `?` returns the error
and `use` silently fails that file's remaining statements, OR the error is caught and the file
load aborts after `namespace`).
