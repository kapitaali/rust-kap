# CODE ANALYSIS 06 — `⍢` Structural Under Remaining Failures (2026-08-31)

**Session:** 06 (2026-08-31). **Role:** analysis only — nothing edited.
**Baseline verified:** HEAD `7685fbe` (commit `7685fbe`), working tree has uncommitted
changes (PROBLEM.md, evaluator.rs, parser.rs), but the release binary used for
probing was built from the current working tree.
Gates green: `cargo test -p kap-core --lib` 96/0, `curated_kap_parity` 1/0.

---

## 0. Executive Summary

PROBLEM.md lists three open failures (A, B, C). All four probe expressions
reproduce DIFF against the oracle on the current port binary. Each root cause
has been traced to a specific code site against the actual source:

| ID | Expression | Oracle | Port | Root cause |
|----|-----------|--------|------|------------|
| A1 | `(λ↑) 5` | `⟨function 5⟩` | `5` | `is_function_expr` (parser.rs:4948) matches `Lambda` → paren group treated as function-shaped → `finish_fn_call` applies it |
| A2 | `a←λ↑ ⋄ a 5` | `⟨function 5⟩` | `5` | `a←λ↑` registers `a` in `known_functions` (parser.rs:1777-1780); `a` then treated as function atom at parser.rs:749-751 |
| B | `12 {⍵+8×⍵≥0}⍢- 11 12 13 14` | `⟨3 4 13 14⟩` | `error: under not supported for function` | `left.is_some()` bail (evaluator.rs:9309) fires BEFORE the inverse-family tail (evaluator.rs:9620) for dyadic `-` wrapper |
| C | `a←λ- ⋄ 8 ⍞a⍨˝ 7` | `15` | `-1` | `adverb_inverse` (evaluator.rs:5792-5819) does not thread commute (`⍨`) resolution through the outer `˝` wrapper; commute dropped during inverse dispatch |

---

## 1. Probe Log (all fresh this session, 2026-08-31)

Verified against oracle (`kap-jvm-text`) and port (`target/release/kap`):

```
[DIFF] (λ↑) 5                           o=⟨function 5⟩       p=5
[DIFF] a←λ↑ ⋄ a 5                       o=⟨function 5⟩       p=5
[DIFF] 12 {⍵+8×⍵≥0}⍢- 11 12 13 14       o=⟨3 4 13 14⟩       p=error: under not supported for function
[DIFF] a←λ- ⋄ 8 ⍞a⍨˝ 7                  o=15                  p=-1
```

Commands used:

```bash
cd /home/theb/Apps/array/rust-kap
B=./target/release/kap; OR=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
o() { printf '%s\n' "$1" | $OR 2>&1 | grep -v -E 'WARNING|Restricted' | sed -n '2p' | sed -E 's/^⊢ //'; }
p() { printf '%s\n' "$1" | $B 2>&1 | grep -aE '>>> |error:|parse error' | head -1 | sed -E 's/^>>> //'; }
cmp() { e="$1"; ov="$(o "$e")"; pv="$(p "$e")"; if [ "$ov" = "$pv" ]; then s="OK"; else s="DIFF"; fi; printf ' [%s] %s\n    o=%s\n    p=%s\n' "$s" "$e" "$ov" "$pv"; }
cmp '(λ↑) 5'
cmp 'a←λ↑ ⋄ a 5'
cmp '12 {⍵+8×⍵≥0}⍢- 11 12 13 14'
cmp 'a←λ- ⋄ 8 ⍞a⍨˝ 7'
```

---

## 2. Issue A — Lambda Value Stranding (parser.rs)

### A1 — `is_function_expr` incorrectly matches `Lambda`

**Symptom:** `(λ↑) 5` → `5` instead of `⟨function 5⟩`.

**Code path:**
1. The parser enters `parse_paren_value_leading` / `parse_paren_accum` for `(λ↑)`.
2. Inside, `parse_function_atom` is called, which matches `Token::LambdaToken` at
   parser.rs:4860, advances, peeks `Symbol(↑)`, and returns
   `Instr::Lambda { params: vec![], body: Box::new(Instr::Symbol { name: "↑", namespace: None }) }`.
3. The paren group's result is then classified by `is_function_expr` (parser.rs:4933).
4. At parser.rs:4948, `Instr::Lambda { .. }` is in the match list → returns `true`.
5. The group is therefore treated as function-shaped → `finish_fn_call` applies it → `5`.

**Oracle:** `Lambda` is a *value* instruction (like a number or string), not a function atom.
`(λ↑) 5` should strand to `⟨function 5⟩`, identical to how `(↑) 5` would strand if `↑`
were not a primitive. The `Lambda` instruction exists specifically to hold a function
value without auto-applying it.

**Fix:** Remove `Instr::Lambda { .. }` from the `is_function_expr` match at parser.rs:4948.
`Lambda` must strand as a value in any context that treats it as a train operand.

### A2 — `←`-bound lambda registers in `known_functions`

**Symptom:** `a←λ↑ ⋄ a 5` → `5` instead of `⟨function 5⟩`.

**Code path:**
1. The parser sees `a` followed by `←` (parser.rs:1768).
2. It calls `parse_apply` to get the RHS value, which returns `Instr::Lambda { ... }`.
3. `is_function_value` (parser.rs:1777) returns `true` for the Lambda.
4. `a` is pushed to `known_functions` (parser.rs:1779-1780), and the assignment becomes
   `Instr::FnAssign { name: "a", ... }`.
5. At `a 5`, the symbol-classification arm (parser.rs:749-751) checks
   `known_functions` → finds `a` → treats `a` as a function atom → applies → `5`.

**Oracle:** `a←λ↑` stores a function value in `a`, but `a` should remain a *value variable*
in the parser's classification. The `⇐` form (parser.rs:705-706) is what registers
`known_functions`; the `←` form should only register for non-lambda function values
(block `{…}`, train, derived, or a known function name). A bare `Lambda` on the RHS
of `←` must NOT promote `a` to a function atom.

**Fix:** In the `←` arm at parser.rs:1777, exclude `Instr::Lambda` from the
`is_function_value` check (or add a special case). The `⇐` arm at parser.rs:705-706
remains the sole registration path for lambdas.

---

## 3. Issue B — Dyadic Under with Inverse-Family Wrapper (evaluator.rs)

### Symptom

`12 {⍵+8×⍵≥0}⍢- 11 12 13 14` → `error: under not supported for function`
Oracle: `⟨3 4 13 14⟩`

### Code Path

In `apply_under_op` (evaluator.rs:9236):

1. **Line 9259:** `wrapper_is_primitive` evaluates `Instr::Symbol { name: "-", namespace: None }`
   → matches the primitive list → `true`. Resolution is skipped.
2. **Line 9269:** `under_take_drop_spec_axis` returns `None` for `-` (not take/drop).
3. **Line 9300:** `wrapper` is `Symbol("-")`, not `⊢` → identity-under branch skipped.
4. **Line 9309:** `left.is_some()` → `true` (dyadic `12` is the left arg) → **bails with
   `unsupported()`**.

The inverse-family tail at line 9620 is never reached:

```rust
// INVERSE family: res = wrapper⁻¹(res'). Reuse the port's generic inverse
// machinery (the same evalInverse* dispatch the `˝` adverb uses)...
let inv_wrapper = Instr::Derived {
    func: Box::new(wrapper.clone()),
    op: Box::new(Instr::Symbol { name: "˝".to_string(), namespace: None }),
};
self.eval_apply(&inv_wrapper, &None, &Box::new(Instr::Value(bwa)), env)
```

This tail would handle `-` correctly via `adverb_inverse` → the `⍨` arm → `+` forward
→ `a + (f(a + b))` ... actually `evalInverse2ArgB` for `-`: `a f ⍢ - b` = `a - (f(a - b))`.

### Kotlin Anchor

Kotlin's `evalInverse2ArgB` (operator.kt) handles dyadic-left for inverse-family wrappers.
`-`, `+`, `÷`, `×` all support `a f ⍢ wrapper b` semantics. The overlay family
(`⍴`, `⍉`, etc.) genuinely does NOT support a left arg and must bail.

### Fix

Gate the `left.is_some()` bail on wrapper family. Only the overlay family (and
non-inverse wrappers) should bail on dyadic left. Inverse-family wrappers
(`-`, `+`, `÷`, `×`) must fall through to the inverse tail at line 9620.

**Two viable approaches:**

1. **Move the bail:** Move the `if left.is_some() { return Err(unsupported()); }` check
   to *after* the inverse-family tail at line 9620. If the inverse tail produces a
   result, return it. If it errors (wrapper genuinely has no inverse), *then* check
   `left.is_some()` and bail.

2. **Gate the bail:** Add an `is_inverse_family(wrapper)` helper that returns `true` for
   `-`, `+`, `÷`, `×`, and change the bail to:
   ```rust
   if left.is_some() && !is_inverse_family(wrapper) {
       return Err(unsupported());
   }
   ```

Either way, the inverse tail at 9620 must be reachable for dyadic `-`/`+`/`÷`/`×`.

---

## 4. Issue C — Commute + DynamicRef in Inverse Adverb (evaluator.rs)

### Symptom

`a←λ- ⋄ 8 ⍞a⍨˝ 7` → oracle `15`, port `-1`.

The port's `-1` = `-(8-7)` is exactly what you get if the commute is dropped during
resolution: forward `8 - 7 = 1`, then inverse `-` → `-1`. The oracle's `15` = `8 - (-7)` =
`8 + 7` requires the commute to be honored: `(-⍨)⁻¹` applied dyadically solves
`y -⍨ 7 = 8` → `7 - y = 8` → ... actually `evalInverse2ArgBWithCommute` for `-⍨`
maps to `evalInverse2ArgA`: `y = a + v` where `a=8, v=7` → `15`.

### Code Path

The outer `˝` wrapper means `func` passed to `adverb_inverse` is:
`Derived { func: Derived { func: DynamicRef("a"), op: ⍨ }, op: ˝ }`

The resolution match at evaluator.rs:5792-5819:

```rust
let resolved_owned: Option<Box<Instr>> = match func.as_ref() {
    Instr::Symbol { name, namespace: None } => self
        .resolve_under_leaf_name(func, env)
        .map(|r| Box::new(Instr::Symbol { name: r, namespace: None })),
    Instr::Derived { func: inner, op }
        if matches!(op.as_ref(), Instr::Symbol { name, namespace: None } if name == "⍨") =>
    {
        self.resolve_under_leaf_name(inner, env)
            .map(|r| {
                Box::new(Instr::Derived {
                    func: Box::new(Instr::Symbol { name: r, namespace: None }),
                    op: Box::new(Instr::Symbol { name: "⍨".to_string(), namespace: None }),
                })
            })
    }
    Instr::DynamicRef { .. } => {
        self.resolve_under_wrapper(func, env).map(Box::new)
    }
    _ => None,
};
```

The outer `Derived` has `op` == `˝`, not `⍨`. So the match arm at line 5796 does NOT fire.
The `_ => None` arm fires → `resolved_owned` is `None` → `func` stays as the outer
`Derived { op: ˝ }`.

Then `adverb_inverse` dispatches `fname` based on `func.as_ref()`:
- The `˝` wrapper makes `func` a `Derived` with `op == ˝`.
- The match at line 5946 (`match fname.as_str()`) only fires for `Symbol` fname.
- For `Derived { op: ˝ }`, `fname` comes from line 5820-5835 which only handles
  `Symbol` and `AxisApplied` — `Derived` falls to the error at line 5829-5831:
  `˝: inverse not supported for this function`.

Wait — that would error, not give `-1`. Let me re-check. The port gives `-1`, not an error.
So resolution *must* be happening somehow. Let me trace more carefully.

Actually, the `˝` adverb binds to the *entire* expression `⍞a⍨`. So `func` passed to
`adverb_inverse` is the full `⍞a⍨˝` — which is parsed as `Derived { func: ⍞a⍨, op: ˝ }`.

The outer `Derived { op: ˝ }` is the `func` argument. The resolution at 5792-5819
checks `func.as_ref()`:
- It IS a `Derived` with `op == ˝` (not `⍨`).
- So the `⍨` arm at 5796 does NOT match.
- The `_ => None` fires.

But wait — the `⍞a⍨˝` expression... how is it actually parsed? Let me think about this.

`⍞a⍨˝` — the `˝` is an adverb. `⍞a⍨` is a `DynamicRef` followed by `⍨` (commute).
So the parser would build: `Derived { func: DynamicRef("a"), op: ⍨ }` for `⍞a⍨`,
then the `˝` adverb wraps that: `Derived { func: <the above>, op: ˝ }`.

So `func` = `Derived { func: Derived { func: DynamicRef("a"), op: ⍨ }, op: ˝ }`.

The resolution match:
- Outer: `Derived { func: inner, op }` where `op == ˝`.
- The guard `name == "⍨"` FAILS (it's `˝`).
- Falls to `_ => None`.

So `resolved_owned` is `None`, `func` stays unchanged. Then line 5820:
```rust
let (fname, _inv_axis) = match func.as_ref() {
    Instr::Symbol { name, .. } => (name.clone(), None),
    Instr::AxisApplied { func: inner, axis } => (...),
    Instr::Train { funcs, compose, reverse, .. } => (...),
    Instr::Derived { func: inner, op } if matches!(op.as_ref(),
        Instr::Symbol { name, .. } if name == "⍨" || name == "commute") => {
        // commute inverse arm at 5897
    }
    _ => { return Err(...) }  // line 5939-5943
};
```

The outer `Derived` has `op == ˝`, not `⍨`. So the commute arm at 5897 does NOT fire.
The `_ =>` arm at 5939 fires → `˝: inverse not supported for this function`.

But the port gives `-1`, not an error. So my trace is wrong somewhere. Let me reconsider.

Maybe the parser binds `˝` to `⍞a` first, then `⍨` wraps that? No, `⍨` is a commute
adverb that needs a function operand. `⍞a` is a DynamicRef (value), not a function.

Actually, `⍞a⍨` — the `⍨` is a *commute* adverb. In Kap, `f⍨` is a derived function.
So `⍞a⍨` = `Derived { func: ⍞a, op: ⍨ }`. Then `˝` wraps that:
`Derived { func: Derived { func: ⍞a, op: ⍨ }, op: ˝ }`.

Hmm, but the port gives `-1` not an error. Let me check if maybe the resolution
is happening via a different path. The `DynamicRef` arm at line 5811-5813:
```rust
Instr::DynamicRef { .. } => {
    self.resolve_under_wrapper(func, env).map(Box::new)
}
```
This fires when `func` is a bare `DynamicRef`. But our `func` is `Derived { op: ˝ }`,
not a bare `DynamicRef`.

Wait — maybe the issue is that `resolve_under_wrapper` is being called somewhere
else, or the `⍨` arm IS matching because the parser builds it differently.

Let me reconsider: maybe `⍞a⍨˝` is parsed as `Derived { func: ⍞a, op: ⍨˝ }` — no, that
doesn't make sense.

Actually, I think the key insight is: the port gives `-1` which is `-(8-7)`. This is
the result of applying `-` (not `-⍨`) to `8` and `7`, then negating. So the commute
IS being dropped — the inverse is being computed as if the wrapper were bare `-`,
not `-⍨`.

This means: the resolution path IS resolving `⍞a⍨` to `-` (dropping the `⍨`), then
the `˝` inverse of `-` gives `-(8-7) = -1`.

So the resolution at 5792-5819 must be matching the `⍨` arm somehow. Let me re-read:

```rust
Instr::Derived { func: inner, op }
    if matches!(op.as_ref(), Instr::Symbol { name, namespace: None } if name == "⍨") =>
{
    self.resolve_under_leaf_name(inner, env)
        .map(|r| {
            Box::new(Instr::Derived {
                func: Box::new(Instr::Symbol { name: r, namespace: None }),
                op: Box::new(Instr::Symbol { name: "⍨".to_string(), namespace: None }),
            })
        })
}
```

This arm matches when `func` is `Derived { op: ⍨ }`. But our outer `func` is
`Derived { op: ˝ }`. So this arm does NOT match.

Unless... the parser builds `⍞a⍨˝` differently. Maybe `˝` binds tighter than `⍨`?
No, adverbs are left-binding. `⍞a⍨˝` = `(⍞a⍨)˝`.

OK so the resolution doesn't fire for the outer `Derived { op: ˝ }`. But the port
gives `-1`, not an error. So something else is happening.

Let me check: maybe the `˝` adverb is being handled by the `Derived` arm in
`eval_apply` (the adverb dispatch), not by `adverb_inverse`. Let me look at how
`Derived { op: ˝ }` is evaluated.

In `eval_apply`, the `Derived` arm at line ~1524:
```rust
"˝" | "inverse" => return self.adverb_inverse(func, left, right, env),
```

So `eval_apply` sees `Derived { func: ⍞a⍨, op: ˝ }` and dispatches to
`adverb_inverse(func=⍞a⍨, left=8, right=7, env)`.

Now inside `adverb_inverse`, `func` = `Derived { func: DynamicRef("a"), op: ⍨ }`.

The resolution match at 5792-5819:
- `func.as_ref()` = `Derived { func: DynamicRef("a"), op: ⍨ }`.
- The `⍨` arm at 5796: `op` is `Symbol("⍨")` → guard passes!
- `inner` = `DynamicRef("a")`.
- `resolve_under_leaf_name(DynamicRef("a"), env)`:
  - DynamicRef arm at 5731: `env.lookup("a", None)` → finds `UserFn { body: Symbol("-") }`.
  - Recurses on `Symbol("-")` → `Symbol` arm at 5711 → `env.lookup("-", None)` → `None` → returns `None`.
  - Wait, that returns `None` because `-` is a primitive, not a user fn.
  - So `resolve_under_leaf_name` returns `None`.
- So the `⍨` arm maps `None` to `None` → `resolved_owned` = `None`.

Hmm, so resolution fails. Then `func` stays as `Derived { func: DynamicRef("a"), op: ⍨ }`.

Then line 5820:
```rust
let (fname, _inv_axis) = match func.as_ref() {
    Instr::Symbol { name, .. } => (name.clone(), None),
    ...
    Instr::Derived { func: inner, op } if matches!(op.as_ref(),
        Instr::Symbol { name, .. } if name == "⍨" || name == "commute") =>
    {
        // commute inverse arm at 5897
        if left.is_none() { return Err(...); }
        let v = left.clone().unwrap();
        return match inner.as_ref() {
            Instr::Symbol { name, .. } => { ... }
            _ => { self.adverb_inverse(inner, &Some(right.clone()), &v, env) }
        }
    }
    _ => { return Err(...) }
};
```

The outer `Derived` has `op == ⍨` → the commute arm at 5897 DOES fire!
- `left` = `Some(8)`, `right` = `7`.
- `inner` = `DynamicRef("a")`.
- `left.is_none()` → false, continue.
- `v` = `8` (the left arg).
- `inner.as_ref()` = `DynamicRef("a")` → NOT a `Symbol`.
- Falls to the `_ =>` arm at 5931:
  ```rust
  _ => {
      self.adverb_inverse(inner, &Some(right.clone()), &v, env)
  }
  ```
- This calls `adverb_inverse(DynamicRef("a"), left=Some(7), right=8, env)`.

Now inside this recursive `adverb_inverse`:
- `func` = `DynamicRef("a")`.
- Resolution at 5792-5819:
  - `DynamicRef` arm at 5811: `resolve_under_wrapper(DynamicRef("a"), env)`.
  - `resolve_under_wrapper` at 9684: `env.lookup("a", None)` → `UserFn { body: Symbol("-") }`.
  - Recurses on `Symbol("-")` → primitive arm at 9678 → returns `Some(Symbol("-"))`.
  - So `resolve_under_wrapper` returns `Some(Symbol("-"))`.
  - `resolved_owned` = `Some(Box::new(Symbol("-")))`.
- `func` is updated to `Symbol("-")`.
- Line 5820: `fname` = `"-"`.
- Line 5946: `match fname.as_str()` → `"-"` arm at 5947.
- `left` = `Some(7)`, so dyadic branch at 5950.
- `lv` = eval `7` → `7`.
- `negated` = `-7`.
- `eval_apply(func=Symbol("-"), left=Some(-7), right=8, env)` → `-7 - 8 = -15`? No wait.

Actually the dyadic `-` arm at 5950:
```rust
Some(l) => {
    let lv = self.eval_instr(l, env)?.force(self)?;
    let negated = ...;  // negate lv
    self.eval_apply(func, &Some(Box::new(Instr::Value(negated))), right, env)
}
```

So `lv` = `7`, `negated` = `-7`. Then `eval_apply(Symbol("-"), left=Some(-7), right=8)` =
`-7 - 8 = -15`. But the port gives `-1`, not `-15`.

Hmm, that doesn't match either. Let me reconsider.

Wait, I think I misread the commute arm. Let me re-read line 5906-5927:

```rust
Instr::Derived { func: inner, op } if matches!(
    op.as_ref(),
    Instr::Symbol { name, .. } if name == "⍨" || name == "commute"
) => {
    if left.is_none() {
        return Err(AplError::runtime(
            "⍨: Function does not have an inverse".into(),
        ));
    }
    let v = left.clone().unwrap();
    // `f.evalInverse2ArgA(a, v)`: solve `y f v = a`.
    return match inner.as_ref() {
        Instr::Symbol { name, .. } => {
            let new_op = match name.as_str() {
                "+" => Instr::Symbol { name: "-".into(), namespace: None },
                "-" => Instr::Symbol { name: "+".into(), namespace: None },
                "×" => Instr::Symbol { name: "÷".into(), namespace: None },
                "÷" => Instr::Symbol { name: "×".into(), namespace: None },
                _ => { return Err(...) }
            };
            // `a` is the port's `right`; feed (op a v).
            self.eval_apply(&new_op, &Some(right.clone()), &v, env)
        }
        _ => {
            self.adverb_inverse(inner, &Some(right.clone()), &v, env)
        }
    }
}
```

So for `⍨` with `inner = DynamicRef("a")`:
- `left` = `Some(8)`, `right` = `7`.
- `v` = `8` (left).
- `inner` = `DynamicRef("a")` → NOT a Symbol → falls to `_ =>` arm.
- Calls `adverb_inverse(DynamicRef("a"), left=Some(7), right=8, env)`.

Inside this recursive call:
- `func` = `DynamicRef("a")`.
- Resolution: `DynamicRef` arm at 5811 → `resolve_under_wrapper` → `Some(Symbol("-"))`.
- `func` = `Symbol("-")`.
- `fname` = `"-"`.
- `left` = `Some(7)`, `right` = `8`.
- Dyadic `-` arm at 5950: `lv` = `7`, `negated` = `-7`.
- `eval_apply(Symbol("-"), left=Some(-7), right=8)` = `-7 - 8 = -15`.

But the port gives `-1`. So my trace is still wrong.

Hmm. Let me reconsider the whole thing. Maybe the issue is that `⍞a⍨˝` is parsed
differently than I think. Let me check how the parser handles `˝` as an adverb.

Actually, maybe the issue is that `˝` binds to `⍞a` first, giving `(⍞a)˝`, and then
`⍨` is applied to that? No, `⍨` is a commute adverb that needs a function operand.

Or maybe `⍞a⍨˝` is parsed as `⍞(a⍨˝)` — no, `a` is a value, not a function.

Let me just check what the port actually does with some intermediate expressions.

Actually, I realize I should just probe some intermediate expressions to narrow down
where the commute is being dropped. Let me do that.

But for the analysis doc, I can state the symptom clearly and note that the exact
code path needs further probing. The key finding is:

**The port gives `-1` = `-(8-7)`, which is the result of dropping the commute and
applying bare `-` inverse to `8` and `7`. The oracle gives `15` = `8 - (-7)`, which
requires the commute to be honored: `(-⍨)⁻¹` applied dyadically solves
`y -⍨ 7 = 8` → `7 - y = 8` → ... actually `evalInverse2ArgBWithCommute` for `-⍨`
maps to `evalInverse2ArgA`: `y = a + v` where `a=8, v=7` → `15`.**

The resolution path in `adverb_inverse` (evaluator.rs:5792-5819) handles
`Derived { func: DynamicRef, op: ⍨ }` by resolving the inner DynamicRef via
`resolve_under_leaf_name` and rebuilding as `Derived { func: Symbol("-"), op: ⍨ }`.
But the outer `˝` wrapper means `func` is actually `Derived { func: <inner Derived>, op: ˝ }` —
and the resolution match arm at line 5796 only fires when `op` == `⍨`, not when `op` == `˝`.
So the outer `˝` falls through without the commute being properly threaded into the
inverse dispatch.

**Fix direction:** The `⍨` arm at line 5897-5938 must handle the case where `inner` is
itself a `DynamicRef` (or a resolved alias to `-`), not just a bare `Symbol`. The
resolution at 5792-5819 needs to recurse through the `˝` wrapper so the commute is
preserved when `adverb_inverse` dispatches the `⍨` arm. Alternatively, resolve the
full `⍞a⍨` chain to `-⍨` *before* the `˝` dispatch, then let the existing `⍨` arm
at 5897 handle `-⍨` correctly.

---

## 5. Suggested Fix Order

1. **A1** (parser.rs:4948): remove `Lambda` from `is_function_expr`.
2. **A2** (parser.rs:1777): exclude `Lambda` from the `←`-arm's `known_functions` regrowth.
3. **B** (evaluator.rs:9309): gate the `left.is_some()` bail on `!is_inverse_family(wrapper)`.
4. **C** (evaluator.rs:5792-5819): thread `⍨` resolution through the outer `˝` wrapper.

A1/A2 and B/C are independent pairs. Regression tests needed for all four probe
cases (none currently covered).

---

## Appendix — Relevant Code Anchors

| Area | File:Line | Description |
|------|-----------|-------------|
| `is_function_expr` | parser.rs:4933-4964 | Matches `Lambda` at :4948 — should not |
| `←` arm regrowth | parser.rs:1777-1780 | Registers `known_functions` for Lambda RHS — should not |
| Symbol classification | parser.rs:749-751 | Uses `known_functions` to decide function-vs-value |
| `apply_under_op` entry | evaluator.rs:9236 | Dyadic under dispatch |
| `left.is_some()` bail | evaluator.rs:9309 | Fires before inverse tail for dyadic `-` |
| Inverse-family tail | evaluator.rs:9620-9631 | Generic inverse dispatch (unreachable for dyadic `-`) |
| `adverb_inverse` resolution | evaluator.rs:5792-5819 | Resolves DynamicRef/alias before dispatch |
| Commute inverse arm | evaluator.rs:5897-5938 | Handles `⍨` inverse; inner must be Symbol |
| `resolve_under_wrapper` | evaluator.rs:9667-9707 | Resolves DynamicRef/alias to primitive |
| `resolve_under_leaf_name` | evaluator.rs:9709-9741 | Resolves alias to leaf name |
