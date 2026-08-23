# ERRORS.md — Kotlin exception → exact error text

ROADMAP §0.3: every evaluator/parser error arm must emit these texts **verbatim**.
Source of truth: `~/Apps/array/array/src/commonMain/kotlin/com/dhsdevelopments/kap/common.kt`
(exception classes + messages) and each throw site. Oracle check:
`printf '<expr>\n' | ~/Apps/array/kap-jvm-text/bin/kap-jvm-text 2>&1`.

Status legend: ✓ = port emits verbatim (verified); ~ = class matches, text differs
(see note); ✗ = not yet aligned.

## Operator / function resolution

| Kotlin exception | Text | Trigger | Status |
|---|---|---|---|
| `InvalidOperatorArgument` (common.kt:186) | `Operator without left function: <name>` | operator name in value position; bare op; `foo ⇐ ⌸` | ✓ (B1/B2, commit cbbf292-era) |
| `IllegalContextForFunction` (common.kt:173) | `No arguments specified for function` | partial operator derivation lands in a value context (`typeof ⌸`) | ~ port says `Operator without left function` for this form |
| `InvalidFunctionRedefinition` | `cannot redefine primitive function '<name>'` | `∇ + (x){…}` / primitive `⇐` RHS | ✓ |

## Namespace / assignment

| Kotlin exception | Text | Trigger | Status |
|---|---|---|---|
| constant assignment | `Assignment to constant variable: <ns>:<name>` | assign to `declare(:const …)` or native quad | ✓ (B6) |
| unassigned variable | `Variable not assigned: <ns>:<name>` | read of never-assigned name in Kotlin's model | ~ port: `undefined symbol: <name>` (no ns prefix) |

## Dimensions / arguments

| Kotlin exception | Text | Trigger | Status |
|---|---|---|---|
| `InvalidDimensionsException` | `⫇: Left argument should be rank 1` | group-index rank mismatch on A | ✓ |
| `InvalidDimensionsException` | `⫇: Size of left argument must match the size of the major axis in the right argument` | group-index major-axis length mismatch | ✓ |

## Adding a row

1. Find the Kotlin throw site (`grep -rn "throwAPLException" ~/Apps/array/array/src/commonMain/kotlin/...`),
   copy the message template exactly.
2. Fire the oracle with the trigger expression and paste the `Error at:` line here.
3. Fix the port arm; add the case to this table with status ✓.
