# Parser Migration Map — Kotlin `parseValueInner` → Rust `parse_value_kotlin`

**Goal:** Faithful translation of Kotlin's single-pass accumulator loop. Every recurring parser burn of the last weeks traces to the port guessing where Kotlin *accumulates*.

**Kotlin anchor:** `parser.kt::parseValueInner` (:875–1030), `processFn` (:432–497), `parseOperator` (:1273–1319), `makeResultList` (:206–212).

---

## Kotlin accumulator loop structure

```
parseValueInner():
  leftArgs = []
  loop:
    token = nextToken()
    if token in END_EXPR_TOKEN_LIST:
      return makeResultList(leftArgs)
    res = dispatch(token):
      Name →
        if customSyntax: processCustomSyntax()
        elif FnDefArrow: processShortFormFn()
        else:
          fn = lookupFunction(symbol)
          if fn != null: processFn(fn, leftArgs, pos)
          elif getOperator(symbol) != null: throw InvalidOperatorArgument
          else: addLeftArg(makeVariableRef(symbol))
      OpenParen →
        group = parseExprToplevel(CloseParen)
        if FnParseResult: processFn(group.fn, leftArgs, pos)
        else: addLeftArg(group.instr)
      OpenFnDef → processFn(parseFnDefinition(), leftArgs, pos)
      ApplyToken → processFn(parseApplyDefinition(), leftArgs, pos)
      MethodCallToken → processFn(processMethodCall(), leftArgs, pos)
      ParsedLong/Double/Complex/BigInt/Rational/Character → addLeftArg(Literal...)
      LeftArrow → return processAssignment(pos, leftArgs)
      DynassignToken → return processDynamicAssignment(pos, leftArgs)
      FnDefSym → return processFunctionDefinition(pos, leftArgs)
      APLNullSym → addLeftArg(LiteralAPLNullValue)
      NilToken → addLeftArg(EmptyValueMarker)
      StringToken → addLeftArg(LiteralStringValue)
      QuotePrefix → addLeftArg(LiteralSymbol)
      LambdaToken → processFn(processLambda(), leftArgs, pos)
      NamespaceToken → processNamespace()
      ImportToken → processImport()
      DefsyntaxSubToken → processDefsyntaxSub()
      DefsyntaxToken → processDefsyntax()
      IncludeToken → processInclude()
      IncludeIfToken → processConditionalInclude()
      DeclareToken → processDeclare()
      OpenBracket → processIndex(pos)  // adjusts leftArgs
      MemberDereferenceToken → processMemberDereference(pos)  // adjusts leftArgs
      IfToken → processIf(pos)
      WhileToken → processWhile(pos)
      else → throw UnexpectedToken
    when (res):
      Instr → addLeftArg(res.instr)
      ResHolder → return res.holder
      Empty → continue
```

## `processFn(fn, leftArgs, pos)` — the core dispatch

```
parseOperator(fn)  // fold operators/adverbs onto fn
tokenAfter = nextToken()
if FunctionCallOpenParen:
  parse `;`-separated list → FunctionCall1Arg(fn, list)  // monadic
elif LeftArrow:
  processModifiedAssignment(fn, leftArgs, pos)
else:
  pushBack(tokenAfter)
  holder = parseValue()
  if Empty:
    if leftArgs empty: return FnParseResult(fn)  // bare function
    else: return FnParseResult(makeLeftBindFunction(leftArgs, fn))
  elif Instr:
    if leftArgs empty: return InstrParseResult(FunctionCall1Arg(fn, holder.instr))  // monadic
    else: return InstrParseResult(FunctionCall2Arg(fn, makeResultList(leftArgs), holder.instr))  // dyadic
  elif FnParseResult:
    if leftArgs empty: return FnParseResult(Chain2(fn, holder.fn))  // compose
    else: return FnParseResult(Chain2(makeLeftBindFunction(leftArgs, fn), holder.fn))
```

## `parseOperator(fn)` — operator folding

```
loop:
  axis = parseAxis()  // optional [axis]
  if axis: currentFn = AxisValAssignedFunction(currentFn, axis)
  token = nextToken()
  if Name:
    op = getOperator(symbol)
    if op == null: break
    currentFn = op.parseAndCombineFunctions(this, currentFn, ...)
  elif LeftForkToken:
    midExpr = parseExprToplevel(RightForkToken)
    rightArg = parseFunctionForOperatorRightArg(this)
    currentFn = Chain3(currentFn, midExpr.fn, rightArg)
  else: break
pushBack(token)
return currentFn
```

## `makeResultList(leftArgs)` — strand collection

```
if leftArgs.empty: null
elif leftArgs.size == 1: leftArgs.first()
else: Literal1DArray.make(leftArgs)
```

## `END_EXPR_TOKEN_LIST`

CloseParen, EndOfFile, StatementSeparator, CloseFnDef, CloseBracket, ListSeparator, Newline, RightForkToken, AndToken, OrToken, FunctionCallCloseParen

---

## Rust port current state

The Rust port (`parse_value_kotlin` + `finish_fn_call` + `bind_operators_kotlin`) is **structurally close** to Kotlin. Key differences:

| Kotlin | Rust | Status |
|--------|------|--------|
| `parseValueInner` accumulator loop | `parse_value_kotlin` loop | ✅ Aligned |
| `processFn(fn, leftArgs, pos)` | `finish_fn_call(fn, &mut left_args)` | ✅ Aligned |
| `parseOperator(fn)` | `bind_operators_kotlin(fn)` | ✅ Aligned |
| `makeResultList(leftArgs)` | `make_result_list(&left_args)` | ✅ Aligned |
| `END_EXPR_TOKEN_LIST` | `kotlin_close_stack` + token match | ✅ Aligned |
| `addLeftArg` | `left_args.push()` | ✅ Aligned |
| `Name` dispatch (fn/op/var) | Symbol dispatch in `parse_value_kotlin` | ✅ Aligned |
| `OpenParen` group → `processFn` if fn | `OpenParen` group → `finish_fn_call` if fn | ✅ Aligned |
| `LambdaToken` → `processFn` | `LambdaToken` → `finish_fn_call` | ✅ Aligned |
| `ApplyToken` → `processFn` | `ApplyToken` → `finish_fn_call` | ✅ Aligned |
| `OpenFnDef` → `processFn` | `OpenFnDef` → `parse_fn_def` (returns directly) | ⚠️ Diverged — ∇ definition not routed through `finish_fn_call` |
| `LeftArrow` → `processAssignment` | `LeftArrow` handled in `parse_value_kotlin` main loop | ✅ Aligned (since P1-M8) |
| `FnDefSym` → `processFunctionDefinition` | `FnDefSym` → `parse_fn_def` | ✅ Aligned |
| `OpenBracket` → `processIndex` (adjusts leftArgs) | `OpenBracket` handled via `parse_index_suffix` in main loop | ✅ Aligned (since P1-M8) |
| `MemberDereferenceToken` → `processMemberDereference` | `MemberDereferenceToken` handled via `parse_index_suffix` in main loop | ✅ Aligned (since P1-M8) |
| `IfToken`/`WhileToken` → `processIf`/`processWhile` | `IfToken`/`WhileToken` → `parse_keyword_prefix` | ✅ Aligned |
| `ResHolder` propagation | Direct return via `finish_fn_call` | ✅ Aligned |

---

## Migration plan

### Phase 1: Close structural gaps (high-value, low-risk)

1. **Route `OpenFnDef` (∇) through `finish_fn_call`** — currently `parse_fn_def` returns directly, but Kotlin routes it through `processFn` so that `3 ∇ f` builds a left-bind function. This is needed for trains containing ∇ definitions.

2. **Move `LeftArrow` (assignment) into the main loop** — DONE (P1-M8, 2026-08-28).

3. **Move `OpenBracket` (index) and `MemberDereferenceToken` into the main loop** — DONE (P1-M8, 2026-08-28).

### Phase 2: Feature-flag and validate

4. **Env-var feature flag** — `Parser::new_kotlin_loop()` behind `KAP_KOTLIN_PARSER` (already exists as the default path).

5. **Run both parsers over full conformance corpus** — compare ok-counts.

6. **Flip when new parser ≥ old on ok-count AND matches oracle on every hand-probe**.

### Phase 3: Remove legacy

7. **Delete `parse_expr` / `parse_apply` / `parse_primary` legacy path** — once the new parser is validated.

---

## Non-regression probes (from ROADMAP §P1)

`3 - 4`, `3-4`, `-x`, `2 (+) 3`, `(1+2)(3+4)`, `f ⇐ ×-`, `10 (-,) 20`, `-⍛+`, `2 ×¨ 3 4 5`, `+/ 1 2 3`, `1 2 3 +[0] 4 5 6`, `(≠⌸)`, `data ⌸ fn`, `typeof ⌸`, `foo ⇐ ⌸`, `3 (+ « × » -) 4`, `(10+) 1`, `10 (-⍛+) 100`

All pass after today's P1 fixes.

---

## P1-M8 fix details (2026-08-28)

At the top of the `parse_value_kotlin` loop (parser.rs ~313), before main dispatch:

```rust
if !left_args.is_empty() {
    let next_tok = self.peek().map(|t| &t.token);
    if matches!(next_tok, Some(Token::OpenBracket))
        || matches!(next_tok, Some(Token::MemberDereferenceToken))
    {
        let base = left_args.pop().unwrap();
        let base = self.parse_index_suffix(base)?;
        left_args.push(base);
        continue;
    }
}
```

This routes `[` and `.` after a value through `parse_index_suffix` in the main accumulator loop, matching Kotlin's `processIndex`/`processMemberDereference` left-arg adjustment pattern.

---

## Feature gaps discovered during migration probing

### Indexed assignment (`x[i] ← v`, `x[i] op← v`)

**Status:** Implemented (P1-M9, P1-M10). All probes oracle-exact.

**Oracle behavior:**
- `x ← 1 2 3 ⋄ x[1] ← 99 ⋄ x` → `⟨1 99 3⟩`
- `x ← 1 2 3 ⋄ x[1 2] ← 99 100 ⋄ x` → `⟨1 99 100⟩`
- `x ← 1 2 3 ⋄ x[1] +← 10 ⋄ x` → `⟨1 12 3⟩` (modified assignment)

**Implementation:**
- `Instr::IndexAssign { array, selector, value }` in ast.rs
- Parser: `LeftArrow` arm in `parse_value_kotlin` recognizes `Index` target → `IndexAssign`
- Parser: `finish_fn_call` detects `LeftArrow` after function → modified assignment (`x[i] op← v`)
- Evaluator: `index_assign()` computes flat indices, builds modified array, assigns back to variable

### Member dereference (`x.f`, `x.(expr)`)

**Status:** Parsing works via P1-M8. Assignment to `x.f` would need the same treatment as indexed assignment.

### `f ← {⍵}` (assignment vs definition)

**Status:** Correctly errors on both — `←` is assignment, `⇐` is definition. The probes that use `←` with a function RHS are invalid Kap. This is correct behavior, not a gap.
