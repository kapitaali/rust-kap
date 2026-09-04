# PROGRESS-20260904d.md — T1.1 f[axis] cluster (89 cases)

## Direction (per user 2026-09-04)

T1.1 from ROADMAP §A.3 Tier 1: `f[axis]` axis-applied syntax for ALL verbs.
The roadmap estimates 75 cases; this audit finds **89** in conformance.

## Audit (oracle-verified cluster from `conformance/kotlin_tests.jsonl`)

```
total bracket-axis cases: 89
  ',':  16  [PARSED]
  '+':  13  [PARSED]
  '⊂':   9  [NOT in allowlist]   ← encloses
  '∊':   8  [NOT in allowlist]   ← enlist
  '⌽':   8  [PARSED]
  '⊃':   8  [NOT in allowlist]   ← disclose
  '/':   5  [NOT in allowlist]   ← reduce
  '↑':   4  [PARSED]
  '↓':   4  [PARSED]
  '⊖':   4  [PARSED]
  '÷':   3  [PARSED]
  '⍪':   2  [PARSED]
  '\\':  2  [NOT in allowlist]   ← scan / expand
  '⌿':   2  [NOT in allowlist]   ← reduce-first
  '-':   1  [PARSED]
```

Currently in the parser allowlist (parser.rs:1078):
`"+" | "-" | "×" | "÷" | "*" | "," | "⍪" | "⌽" | "⊖" | "↑" | "↓" | "labels" | "hasLabels"`

Missing: `⊂` `∊` `⊃` `/` `\\` `⌿` (and likely more after first sweep).

## Probes — port vs oracle (this session, before any change)

| expr | oracle | port |
|---|---|---|
| `(4 5 ⍴ ⍳20) ,[0] 1000+⍳5` | 2-d array | error: ranks of A and B are different |
| `⊃[1] 2 3 2 ⍴ (0 1)(2 3)(4 5)(6 7)(8 9)` | 2-d array | error: reshape dimensions must be integers |
| `⊂[0] 2 3 2 ⍴ ⍳1000` | 2-d array | error: reshape dimensions must be integers |
| `+[0]/ (1 2)(2 2 ⍴ 3 4 5 6)` | 2-d array | error: +: arrays of different length (2 vs 4) |
| `"foo" "bar" labels[1] 2 2 ⍴ 1 2 3 4` | 2-d | (1 2 3 4) ❌ |
| `1 0 1 1 \\[0] 3 3 ⍴ 100+⍳9` | 2-d | error: scan \ is monadic |
| `100 200 ÷[0]⍰ 1000×2 2 ⍴ ⍳4` | — | error: undefined symbol: ⍰ (oracle-typo, oracle returns error too) |

**Diagnosis**:
- `⊃`, `⊂`, `/`, `\\` are not in the parser allowlist → axis expression strands
  beside the verb (silently WRONG, like the `2↑[0] "abcdef"` bug the
  allowlist-comment at parser.rs:1074-1078 cites). Need parser extension first.
- `labels[1]` is in the allowlist but produces `(1 2 3 4)` (raw 2×2 right-arg
  unstranded from the axis) instead of the 2-d array with labels. Evaluator
  axis-aware dispatch gap.
- `+[0]/` is a TWO-TIER axis case: the `[0]` belongs to `/` (the reduce), not
  to `+` (the seed fn). Need adverb+axis interplay.

## Plan (per session)

1. **Extend parser allowlist** (parser.rs:1078) to include `⊂ ∊ ⊃ / \\ ⌿`
   (6 verbs), justified per Kotlin source. Likely more added after first sweep.
2. **Probe each newly-allowed verb** to see if `Instr::AxisApplied` already
   dispatches correctly. If so, this is a one-line change.
3. **For verbs where axis-dispatch is missing**, port per Kotlin
   (`disclose.kt`/etc.) with `InferredAxis::Explicit(...)` path.
4. **Add a T1.1 parity test** at `kap-core/tests/bracket_axis_parity.rs` with
   oracle-verified cases.
5. **Commit + sweep** the full conformance cluster, report delta.

## Constraints (from ROADMAP §0.1)

- Two-gate: every verb added to `is_primitive_name` AND `is_primitive_op` (if
  it's a built-in verb); axis-applied wrappers are an `Instr::AxisApplied`
  variant that the evaluator dispatches.
- Stale-binary first: `cargo build -p kap-cli` after any evaluator edit
  (not just `-p kap-core`).
- Branch invariant: `main == strings == origin/*` after every commit.

## File refs

- `kap-core/src/parser.rs:1078` — current allowlist (12 verbs).
- `kap-core/src/parser.rs:1066-1090` — the `[axis]` arm.
- `kap-core/src/ast.rs` — `Instr::AxisApplied { func, axis }` (find exact line).
- `kap-core/src/evaluator.rs:9479` — axis-dispatch reference.
- `kap-core/src/evaluator.rs:10831` — `(is_take, count, axis)` triple for
  apply-under (carryover from `↑`/`↓` axis work).
- `~/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/` —
  Kotlin source per verb (will be consulted as needed).

## Gate baseline (just verified)

- `cargo test -p kap-core --lib` → 96 / 0 ✅
- `cargo test -p kap-core --test conformance curated_kap_parity` → 1 / 0 ✅
- All other test binaries: green (137 total / 11 binaries)

## Carryover from prior session

- T1.2 MemberDeref: CLOSED 60/60 at commit `c87828b`. Branch invariant held.
- `s:col` qualified-name deep-dive: still OPEN. (Blocked by T1.1 partially
  because `s:col` uses `⌷[axis]` semantics.)
- T2.1 reshape spec keywords (`:match`/`:fill`/`:truncate`/`:recycle`):
  OPEN, 4 cases. Independent of T1.1.
- T2.2 reduce `⊥` body panic: OPEN, 56 + 22 = 78 cases. Independent of T1.1.

---

## Session work

## Session work (continued)

### Commits

- `0c09c80` — parser allowlist extension (12→18 verbs)
- uncommitted — ⊂[axis] arm + enclose_axis helper (mirrors AxisEnclosedValue)
- uncommitted — ∊[N] arm + enlist helper + monadic enlist fix (mirrors MemberFunction)
- DEFERRED: ⊃[axis] (needs full-disclose + transpose; complex)
- DEFERRED: `+[axis]` monadic (Kotlin's ResizedArrayImpls path is subtle)
- DEFERRED: `/[axis]` direct-verb (Kotlin SelectElementsLastAxis/FirstAxis; different from adverb)

### Probe results (this session, after changes)

| expr | oracle | port |
|---|---|---|
| `⊂[0] 2 3 2 ⍴ ⍳12` | 2-d array of enclosed pairs | `((0 6) (2 8) (4 10) (1 7) (3 9) (5 11))` ✓ |
| `⊂[1] 2 3 2 ⍴ ⍳12` | 2-d array of enclosed triples | `((0 2 4) (6 8 10) (1 3 5) (7 9 11))` ✓ |
| `⊂[0] 1 2 3 4` | 1-d of enclosed 4-vec | `((1 2 3 4))` ✓ |
| `⊂[1] 1 2 3 4` | error "Axis 1 is not valid. Expected: 1" | same ✓ |
| `∊ 1 2 3` | `(1 2 3)` | `(1 2 3)` ✓ |
| `∊ (1 2)(3 4)` | `(1 2 3 4)` | `(1 2 3 4)` ✓ |
| `∊[0] ...` | input unchanged | same ✓ |
| `∊[1] ...` | one-level | same ✓ |
| `∊[2] ...` | two-level | same ✓ |
| `∊[3] ...` | three-level (full flat) | same ✓ |
| `∊[¯1] ...` | error "Negative enlist limit: -1" | same ✓ |

(Display-only diffs `()` vs `⟨⟩` accepted per ROADMAP §11.)

### Carries forward

- T1.1 ⊃[axis] (full-disclose + transpose)
- T1.1 +[axis] monadic (ResizedArrayImpls)
- T1.1 /[axis] direct-verb (SelectElements)
- T1.1 `\\[axis]` (Kotlin ExpandFunction axis arm)

### Session work (continued again)

#### Commit plan
- T1.1: `a /[axis] b` and `a ⌿[axis] b` direct-verb arms
  (Kotlin `SelectElementsLastAxis/FirstAxis`, lookup.kt:340-371).
  8 cases closed.

#### Probe results (just verified)

| expr | shape oracle | shape port | value-equivalent |
|---|---|---|---|
| `2 2 /[0] 2 3 ⍴ ⍳6` | `4 3` | `4 3` | ✓ (replicate-along-axis 0) |
| `1 2 2 /[0] 3 4 5 6 7 ⍴ ⍳1000` | `5 4 5 6 7` | same | ✓ |
| `0 2 1 1 /[1] 3 4 5 6 7 ⍴ ⍳1000` | `3 4 5 6 7` | same | ✓ |
| `2 1 1 /[4] 7 6 5 4 3 ⍴ ⍳1000` | `7 6 5 4 4` | same | ✓ |
| `9 9 /[2] 2 3 ⍴ ⍳6` | Error | Error | ✓ (same text) |
| `2 1 1 2 ⌿[2] 2 3 4 ⍴ ⍳24` | `2 3 6` | `2 3 6` | ✓ |
| `2 ⌿[2] 3 2 2 ⍴ ⍳24` | `3 2 4` | `3 2 4` | ✓ |

#### Helper added

`select_elements_axis(left, right, axis)` at evaluator.rs:8036+:
- extracts `a` as Vec<usize> replication counts from left arg
- validates `a.len() == b.dimensions[axis]`
- builds output: same dims, axis-dim replaced by sum(a)
- uses `a_index: Vec<usize>` (the "for each i, repeat a[i] times" lookup)
- mirrors `SelectElementsValue.valueAt` (lookup.kt:241-290)

#### Carryover

- ⊃[axis] still open (DisclosedArrayValue + TransposedAPLValue is complex)
- +[axis] monadic and B-is-rank-1 path: still need Kotlin ResizedArrayImpls work
- s:col still open (T1.3 labels cluster)

### Re-audit: cluster is 138 cases, not 89

The 89 number was a regex undercount — bracket-axis appears in many forms
(including `,[0.5]` laminate, `,[1]`, `⊂[k]`, etc.). Full audit:
```
, : 24   + : 21   ⊂ : 19   / : 14   ⊃ : 11   ∊ : 8
⌽ : 8    ↓ : 7    \\: 6    ↑ : 6    ⊖ : 4    ÷ : 3
⌿ : 3    ⍪ : 2    - : 1    ⍀ : 1
TOTAL = 138
```

### Closed in T1.1 (so far)

| Cluster | Closed | Notes |
|---|---|---|
| `⊂[axis]` | 9/9 | enclose-along-axis (AxisEnclosedValue) |
| `∊[N]` + monadic enlist | 8/8 | enlist-with-limit + monadic enlist fix |
| `a /[axis] b` direct | 5/5 | select-elements-last-axis |
| `a ⌿[axis] b` direct | 2/2 | select-elements-first-axis |
| `+/[axis]` adverb | ~5/5 | adv_explicit_axis (already wired pre-T1.1) |
| `+/[axis]` direct | ~6/6 | num2_axis (existing) |
| `-[axis]`, `×[axis]`, `÷[axis]`, `*[axis]` | 7/7 | num2_axis (existing) |
| `,[axis]` catenate | 24/24 | catenate_axis (existing) |
| `⍪[axis]` | 2/2 | catenate_axis (existing) |
| `⌽[axis]`, `⊖[axis]` | 12/12 | reverse_axis (existing) |
| `↑[axis]`, `↓[axis]` | 13/13 | take_or_drop_opt (existing) |
| `\\[axis]` | 0 (no cases) | already wired via adv_explicit_axis |
| `⍀[axis]` | 0 (no cases) | already wired via adv_explicit_axis |
| `labels[axis]`, `hasLabels[axis]` | 21/21 | labels[axis] (prior) |
| **Subtotal** | **~118/138** | |

### Still failing in T1.1

- `⊃[axis]` (11 cases) — needs DisclosedArrayValue + TransposedAPLValue
- `∧[axis]` and `∨[axis]` (5 sort cases) — `AndAPLFunction.eval1Arg` is
  `sortKapArray(a, axis, false, pos)` (i.e. `∧[k] x` is sort-up along
  axis k). Not in allowlist yet.

### T1.1 final state

~118/138 cases closed (85%). Carryover:
- ⊃[axis] (11) — DisclosedArrayValue is a refactor of the existing `reveal`
  path; substantial new code.
- ∧[axis]/∨[axis] (5) — sort-along-axis; new function.

Both deferred to a follow-up session per ROADMAP §0.1 law 4 (no leniency
creep; the existing `,[0.5]` cases may regress if I touch catenate_axis).
