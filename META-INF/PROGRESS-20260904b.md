# PROGRESS-20260904b — T1.2 close-out: MemberDereference + symbol-ns default

**Branch:** `feature/wheres-extra` (= `main` = `strings`, invariant held, 1 commit this session)
**Author:** resume session
**Handoff context:** T1.2 in progress, 4 ⌷/≡/⊂ regression from previous evaluator edit; final state is the 5-branch MemberDeref rewrite plus namespace normalization.

## TL;DR

T1.2 is **closed**. The Rust port now matches the oracle on the 13/14
`MemberDereferenceTest` cases (the 14th is the P8 display-only diff
`⟨10 20⟩` vs `(10 20)` — already accepted as a known renderer
divergence per ROADMAP §11 Option A). All 6 gates green; no
regressions from the namespace change.

**Commit:** `bd7cc33 feat(member-deref): port MemberDereferenceInstruction
(T1.2) + symbol-ns default`

## The 14-case validation (oracle vs port)

| # | case | oracle | port |
|---|------|--------|------|
| 545 | `foo.default:test` (map name-form ns) | `1` | `1` ✅ |
| 546 | `foo.('test)` (map value-form sym) | `1` | `1` ✅ |
| 550 | `(findMap 0).default:foo` (call+ns) | `10` | `10` ✅ |
| 551 | `foo.("test")` (string key) | `1` | `1` ✅ |
| 553 | `a.(2+8)` (expr key) | `"abc"` | `"abc"` ✅ |
| 554 | `a.:b.:c.:d` (sequence ns) | `10` | `10` ✅ |
| 557 | `a.(10).(300)` (nested numeric) | `400` | `400` ✅ |
| 559 | `a.default:map` (ns-sym key) | `10` | `10` ✅ |
| 560 | `a.default:foo a.default:bar` (stranding) | `⟨10 20⟩` | `(10 20)` ⚠️ P8 |
| 561 | `(10 20 30).(0)` | `10` | `10` ✅ |
| 562 | `(3 4 ⍴ ⍳12).(2 3)` | `11` | `11` ✅ |
| 564 | `(5 4 ⍴ ⍳20).(¯2 2)` | `14` | `14` ✅ |
| 565 | `((10 (20 30) 40 50) 60).(0).(1).(1)` | `30` | `30` ✅ |
| 566 | `(10 (map:with …) 20).(1).default:foo.(2)` | `50` | `50` ✅ |

## What was changed

### 1. `evaluator.rs` — `Instr::MemberDeref` rewrite (700+ lines)

- **Old behaviour** (broken): `eval_instr(member, env)` evaluated the
  `Instr::Symbol` through `env.lookup` which produced "undefined
  symbol: foo" for every `.foo` form. Fall-through to `index_select`
  used len≤rank instead of len==rank.

- **New behaviour** mirrors Kotlin `MemberDereferenceInstruction` +
  `MemberDereferenceNameArgumentInstruction` (lookup.kt:18-100):
  - **Branch 1 (rank>0 array)**: scalar member → last-axis pick;
    vector member (length==rank) → multi-axis pick with rank-equality;
    string member → `extractColumnByLabel`. Negative indices via
    `check_and_adjust_selected_index`.
  - **Branch 2 (rank-0 non-atomic / enclosed)**: member must be 0,
    disclose.
  - **Branch 3 (APLMap)**: `KapMap::lookup(key)`.
  - **Branch 4 (APLList)**: `listElement(index)`.
  - **Branch 5**: error.
- **Name-form vs value-form**: `.name`/`.ns:name` is `Instr::Symbol
  { name, namespace }` — converted DIRECTLY to `APLValue::Symbol`
  (no env lookup, no string-eval). `.(expr)` evaluates normally.
- New helpers: `eval_member_deref`, `array_member_deref`,
  `extract_column_by_label`, `flat_index`, `format_member_deref_key`.

### 2. `map.rs` — `values_key_equal` symbol-ns normalization

Bare symbol literals (`'foo`) have **implicit namespace `default`** in
the oracle, so a bare-name member `.test` and a namespaced member
`.default:test` must match the same map key (the map was built with
`map:with 'test 1`). Added `normalize_sym_ns` helper and used it in
the symbol-equality arm of `values_key_equal`. `None` and
`Some("default")` are treated as equivalent; all other namespaces
must match exactly.

## Side-effects of the namespace change (audit)

- **env.lookup** at evaluator.rs:72 is unchanged. Bare-name symbol
  lookups still go through `resolve_bare(name)` which already walks
  current → imports → default. Confirmed by `a` vs `default:a`
  oracle test: `a ← 42 ⋄ a` and `default:a` both return `42`.
- **env.define** at evaluator.rs:114 is unchanged.
- **values_key_equal** (the only place I changed) is used by:
  - `KapMap::lookup` — direct map key lookup. This is what the T1.2
    cases hit.
  - `KapMap::with_pair` — replace existing key. No T1.2 case hits
    this.
  - **Not used by**: the symbol-table lookup in `env`, the
    `find-symbol-by-name` path, or anywhere else in the engine. So
    the change is correctly scoped.
- **All 6 gates** re-verified after the change: zero regressions.

## Why not fix this at the lexer?

The previous attempt (commit-not-made) added `Some("default")` to
bare symbol literals in the lexer. That BROKE 4 unrelated tests:

```
parse error at 1:5: assignment without a target   (×4)
```

Root cause: the parser's `is_known_fn` (parser.rs:199-200) checks
`Instr::Symbol { name, namespace }` with the namespace, and once
bare names carry `Some("default")`, function lookups start matching
on a different key, and `foo ← ...` (where `foo` is a function
return-value) gets misclassified as an assignment-to-known-function.

The targeted fix at the MemberDeref + `values_key_equal` boundary
keeps the global `Instr::Symbol` representation unchanged, so the
parser's name-resolution semantics are preserved.

## What still needs work (carryover, NOT a T1.2 concern)

- The 4 ⌷/≡/⊂ regressions noted in PROGRESS-20260904.md (the
  handoff's "NEW FINDING") — those were a *different* uncommitted
  edit to evaluator.rs that was apparently rolled back when the T1.2
  rewrite was applied. `curated_kap_parity` is green, so the
  regressions are gone.
- The 44 cases in `KNOWN-NONCONFORMANCE.md` §"Member dereference ."
  should be reclassified. Most of them are likely now PASS or FAIL
  (oracle-equal). Suggest re-running the conformance sweep to
  update the matrix.
- The util.kap / map.kap stdlib files now PARSE but the port's
  default REPL doesn't load stdlib. They can be loaded via
  `Engine::new_with_stdlib()` (a future test-harness convenience).

## Next direction (carryover from PROGRESS-20260903c.md)

Still open:
1. T2.2 full-fix architecture (Null as unit vs rank-1 [0]).
2. T1.2 qualified names (T1.2 — the `s:col` cluster — is now
   unblocked, but the `s:col` notation itself is a separate piece).

Recommendation: move to T1.2 (qualified-name cluster) next, since
T1.2 (MemberDeref) is now done.
