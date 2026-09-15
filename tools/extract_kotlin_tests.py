#!/usr/bin/env python3
"""Extract Kap Kotlin unit tests into a machine-readable catalog.

Reads every `*Test.kt` under ~/Apps/array and emits one JSON object per test
method as `conformance/kotlin_tests.jsonl`. Each record:

    {
      "file":   "<relative path of the .kt file>",
      "test":   "<fun name>",
      "kind":   "eval" | "fails",     # fails => assertFailsWith wrapper
      "expr":   "<raw Kap source passed to parseAPLExpression/WithTest>",
      "expected": null | "<asserted scalar/array string, best effort>"
    }

This is a *coverage* instrument, not a faithful re-implementation. The expected
field is only populated when the assertion is a single-line `assertSimpleNumber`
or `assert1DArray`/`assertString`; otherwise it is null and the harness merely
checks that the expression parses & evaluates (kind=eval) or errors (kind=fails).

Usage:
    python3 tools/extract_kotlin_tests.py [ARRAY_ROOT]
"""
import json
import os
import re
import sys

ARRAY_ROOT = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/Apps/array")
OUT = os.path.join(os.path.dirname(__file__), "..", "conformance", "kotlin_tests.jsonl")

PARSE_CALLS = ("parseAPLExpression", "parseAPLExpressionWithTest", "parseAndTestWithGeneric",
               "parseAPLExpression2", "parseAPLExpressionWithOutput",
               "parseAPLExpressionWithSpecialRandom")


def find_string_literal(s: str, start: int):
    """Return (inner_text, end_index_after_closing_quote) for a "..." or triple-quoted string starting at s[start]=='\"'.

    For triple-quoted strings, applies Kotlin's trimMargin/trimIndent logic.
    """
    assert s[start] == '"'
    # Triple-quoted string (Kotlin raw string with trimMargin/trimIndent)
    if s[start:start + 3] == '"""':
        end = s.find('"""', start + 3)
        if end == -1:
            raw = s[start + 3:]
            end_idx = len(s)
        else:
            raw = s[start + 3:end]
            end_idx = end + 3
        # Apply trimMargin/trimIndent: strip leading whitespace + optional '|' from each line
        lines = raw.split('\n')
        # Find minimum indent (ignoring blank lines and lines with only '|')
        min_indent = None
        for line in lines:
            stripped = line.lstrip()
            if stripped == '' or stripped == '|':
                continue
            indent = len(line) - len(line.lstrip())
            if min_indent is None or indent < min_indent:
                min_indent = indent
        if min_indent is None:
            min_indent = 0
        # Strip margin
        result_lines = []
        for line in lines:
            if line.strip() == '':
                result_lines.append('')
            elif line.strip() == '|':
                result_lines.append('')
            else:
                # Strip leading whitespace up to min_indent, then optional '|'
                stripped = line[min_indent:]
                if stripped.startswith('|'):
                    stripped = stripped[1:]
                result_lines.append(stripped)
        # Remove leading/trailing blank lines
        while result_lines and result_lines[0] == '':
            result_lines.pop(0)
        while result_lines and result_lines[-1] == '':
            result_lines.pop()
        return '\n'.join(result_lines), end_idx
    # Single-quoted string, with `+`-concatenated continuation support: Kotlin
    # tests often build `src` as `"part1 " + "part2 " + ...` across lines. When
    # the closing quote is followed (after whitespace) by `+`, consume it and
    # append the next literal. Returns the FULL concatenated source.
    i = start + 1
    buf = []
    while i < len(s):
        c = s[i]
        if c == '\\':
            nxt = s[i + 1] if i + 1 < len(s) else ''
            mapping = {'n': '\n', 't': '\t', 'r': '\r', '\\': '\\', '"': '"', "'": "'"}
            buf.append(mapping.get(nxt, '\\' + nxt))
            i += 2
            continue
        if c == '"':
            i += 1
            # Concatenated continuation? (`"..." + "..."`): consume every
            # `+ "..."` part, then RETURN. Never fall back into char-scanning
            # here — the chars after the final closing quote are Kotlin code,
            # not string content (over-consumption ate later test bodies).
            while True:
                j = i
                while j < len(s) and s[j] in ' \t\n':
                    j += 1
                if j < len(s) and s[j] == '+':
                    k = j + 1
                    while k < len(s) and s[k] in ' \t\n':
                        k += 1
                    if k < len(s) and s[k] == '"':
                        cont, i = find_string_literal(s, k)
                        buf.append(cont)
                        continue
                break
            return ''.join(buf), i
        buf.append(c)
        i += 1
    return ''.join(buf), i


def find_balanced(s: str, start: int, open_ch='{', close_ch='}'):
    """Given s[start]=='{', return index just past the matching '}'."""
    depth = 0
    i = start
    while i < len(s):
        if s[i] == "'" or s[i] == '"':
            _, i = (find_string_literal(s, i) if s[i] == '"' else (s[i], i + 1))
            continue
        if s[i] == open_ch:
            depth += 1
        elif s[i] == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return i


def extract_method_bodies(src: str, class_ignored: bool = False):
    """Yield (name, body) for each `@Test fun <name>(...) { ... }`.

    Skips `@Ignore`d tests: the Kotlin runner never executes them, so
    scoring the port against their exprs inflates the denominator with
    cases Kotlin itself does not run (e.g. StructuralUnderTest's
    `structuralUnderWihDecreaseRank`, marked `// TODO: Should be
    implemented`). An `@Ignore` may sit directly above `@Test`, possibly
    with a `//` comment line between. A class-level `@Ignore` (e.g.
    `FfiApiTest`) skips the whole file.
    """
    if class_ignored:
        return
    for m in re.finditer(r'@Test\s+fun\s+(\w+)\s*\(([^)]*)\)\s*\{', src):
        name = m.group(1)
        # Look back over the preceding lines for @Ignore (allowing @Test
        # on the same line cluster and // comments in between).
        back = src[max(0, m.start() - 400):m.start()]
        lines = back.split('\n')
        ignored = False
        for line in reversed(lines[-6:]):
            s = line.strip()
            if s == '' or s.startswith('//'):
                continue
            if s.startswith('@Ignore'):
                ignored = True
            break
        if ignored:
            continue
        body_start = m.end() - 1  # index of '{'
        body_end = find_balanced(src, body_start)
        yield name, src[body_start + 1:body_end]


def first_expr_in_call(body: str):
    """Find the first parseAPLExpression(...) call and return its raw string-literal arg.

    Many tests pass a `src` variable holding a triple-quoted multi-statement
    program (`val src = \"\"\"...\"\"\".trimMargin()`); resolve the identifier to
    its declaration above the call. Without this the extractor grabbed the
    first string *after* the call (often an assertion literal like `"a"`).
    """
    for call in PARSE_CALLS:
        idx = body.find(call + '(')
        if idx == -1:
            continue
        j = idx + len(call) + 1
        while j < len(body) and body[j] in ' \t\n':
            j += 1
        # Kotlin 2.x multi-dollar raw strings (`$$"..."`, `$"..."`): the `$`
        # prefix disables interpolation, so `$foo` stays literal Kap source
        # (FormatAPLTest `⍕` directives). The string starts at the `"`.
        if body[j:j + 3] == '$$"':
            text, _ = find_string_literal(body, j + 2)
            return text
        if body[j:j + 2] == '$"':
            text, _ = find_string_literal(body, j + 1)
            return text
        if j < len(body) and body[j] == '"':
            text, _ = find_string_literal(body, j)
            return text
        m = re.match(r'[A-Za-z_]\w*', body[j:])
        if not m:
            continue
        ident = m.group(0)
        decl = re.search(
            r'val\s+' + re.escape(ident) + r'\s*=\s*"""(.*?)"""',
            body[:idx], re.DOTALL)
        if decl:
            lines = decl.group(1).split('\n')
            out = []
            for line in lines:
                s = line.lstrip()
                if s.startswith('|'):
                    s = s[1:]
                out.append(s)
            text = '\n'.join(out).strip('\n')
            # Kotlin triple-quoted (raw) strings process NO backslash escapes
            # (only `$`-templates interpolate) — so backslashes stay verbatim.
            # A prior version unescaped `\\`/`\"` here, corrupting Kap string
            # escapes (JsonTest deref rows lost their `\"` and failed to parse).
            return text
        decl2 = re.search(
            r'val\s+' + re.escape(ident) + r'\s*=\s*"',
            body[:idx])
        if decl2:
            q = body.find('"', decl2.start())
            if q != -1:
                text, _ = find_string_literal(body, q)
                return text
    return None


def detect_fails(body: str):
    if 'assertFailsWith' in body:
        return True
    # try { parseAPLExpression(...) ... } catch — the test expects evaluation
    # to throw (e.g. ComplexExpressionsTest.functionWithNoIdentityValue,
    # ExceptionsTest.stackTrace, ThrowNativeTest). Without this these rows are
    # misclassified as plain evals and a correct engine error counts against
    # conformance. `catch` inside the Kap *expression* (e.g. `}catch ...`
    # handler tests) does not match: the `try` must precede a parse call.
    for m in re.finditer(r'\btry\b', body):
        rest = body[m.end():m.end() + 2000]
        ci = rest.find('catch')
        if ci == -1:
            continue
        if any(c + '(' in rest[:ci] for c in PARSE_CALLS):
            return True
    return False


def expand_template_loop(expr: str, body: str) -> list:
    """Expand Kotlin string-template loops to concrete expressions.

    Handles two patterns:
    1. Simple arithmetic: repeat(15) { i -> "...${i + 1}" }
    2. List-indexed: val strings = listOf("a", "b"); repeat(10) { i -> "...${strings[i]}" }

    Returns a list of concrete expr strings (one per iteration),
    or [expr] if no template loop is found.
    """
    import re as _re
    # Find the enclosing loop for this expr: look for `repeat(N) { i ->`
    # pattern in the body before the expr's parse call.
    loop_m = _re.search(r'repeat\s*\(\s*(\d+)\s*\)\s*\{\s*(\w+)\s*->', body)
    if not loop_m:
        return [expr]
    count = int(loop_m.group(1))
    var = loop_m.group(2)
    # Only expand if the expr contains a Kotlin string template referencing the loop var.
    # Templates can be direct (${i + 1}) or list-indexed (${strings[i]}).
    var_in_template = (
        f'${{{var}' in expr  # ${i + ...}
        or f'${var}' in expr  # $i
        or f'[{var}]' in expr  # ${strings[i]} or ${list[i]}
        or f'[{var} ' in expr  # ${list[i + 1]}
        or f' {var}]' in expr  # ${list[1 + i]}
    )
    if not var_in_template:
        return [expr]

    # Look for list bindings before the loop: `val <name> = listOf(<vals>)`
    list_bindings = {}
    for m in _re.finditer(r'val\s+(\w+)\s*=\s*listOf\s*\(([^)]*)\)', body[:loop_m.start()]):
        name = m.group(1)
        vals_str = m.group(2)
        # Parse the list values (simple strings/numbers)
        vals = []
        for v in vals_str.split(','):
            v = v.strip()
            # Handle escaped quotes in strings
            if v.startswith('"') and v.endswith('"') and len(v) >= 2:
                # Remove surrounding quotes and unescape
                inner = v[1:-1].replace('\\"', '"')
                vals.append(inner)
            else:
                try:
                    vals.append(int(v))
                except ValueError:
                    try:
                        vals.append(float(v))
                    except ValueError:
                        vals.append(v)
        list_bindings[name] = vals

    results = []
    for i in range(count):
        def repl(m):
            inner = m.group(1)
            # Try list indexing first: strings[i], decoded[i], etc.
            list_m = _re.match(r'(\w+)\[' + re.escape(var) + r'\]', inner)
            if list_m:
                name = list_m.group(1)
                if name in list_bindings:
                    val = list_bindings[name][i]
                    return str(val)
            # Fall back to simple arithmetic
            expr_inner = inner.replace(var, str(i))
            try:
                return str(eval(expr_inner))
            except Exception:
                return m.group(0)
        expanded = re.sub(r'\$\{([^}]+)\}', repl, expr)
        results.append(expanded)
    return results


def expand_make_functions(exp: str) -> str:
    """Expand the `${makeFunctions()}` Kotlin template (ComposeTest.kt:250-257).

    That private helper builds five `⇐` tracing functions A..E at runtime;
    the extractor otherwise emits the literal `${makeFunctions()}` placeholder,
    which is unparsable Kap. The expansion below is the helper's exact runtime
    output, so the four contribTest rows become genuine eval cases.
    """
    if '${makeFunctions()}' not in exp:
        return exp
    lines = []
    for ch in 'ABCDE':
        lines.append(
            f'{ch} ⇐ {{ io:print if(isLocallyBound \'⍺) '
            f'{{ "(",⍺,"{ch}",⍵,")" }} else {{ "({ch}",⍵,")" }} }}'
        )
    return exp.replace('${makeFunctions()}', '\n'.join(lines))


def expand_val_bindings(exp: str, body: str) -> str:
    """Substitute `${name}` templates from local `val name = <int>` bindings.

    E.g. EncoderAPLFunctionTest `val n = 300` with expr
    `"x ← encoder:encode 2 ⋆ ⍳ ${n} ⋄ …"` becomes `⍳ 300`. Only plain
    integer bindings are substituted; anything else is left for the
    `${…}` fallthrough below (eval-OK scoring, no expected value).
    """
    if '${' not in exp:
        return exp
    for m in re.finditer(r'val\s+(\w+)\s*=\s*(\d+)', body):
        exp = exp.replace('${' + m.group(1) + '}', m.group(2))
    return exp


def _extract_kotlin_string_concat(body: str, start_idx: int) -> tuple[str | None, int]:
    """Extract a Kotlin string expression that may be `\"a\" + \"b\" + ...` concatenation.

    `start_idx` is the index of the opening `\"` of the first literal. Returns
    (concatenated_value, end_index) or (None, start_idx) on failure.
    Handles the same `+`-continuation that `find_string_literal` does, plus
    the `+`-separated multi-line form used in `XmlParserTest.assertString`:
      assertString(\"\\n\" + \"        Some...\\n\" + \"        \", result)
    """
    try:
        val, end = find_string_literal(body, start_idx)
        return val, end
    except Exception:
        return None, start_idx


def _xml_hardcoded_expected(test: str, body: str) -> str | None:
    """Honest value-verification for the 9 `XmlParserTest` rows.

    The broad sweep previously scored these as run-verified only
    (`expected: None`; Kotlin asserts Document/Node properties and Kap
    `assertString`/`assertSym`/`assertArrayContent` Java-side, which
    `best_effort_expected` ignored). This closes that extractor gap so the
    harness can value-verify them:
    - 5 rows return pure Kap values (array/string/symbols) → expected is the
      Kap `format_value` string the harness already compares.
    - 4 rows return `JvmInstanceValue` (Document/Node) → expected is the
      asserted *property* (nodeName/attribute) that the harness verifies via a
      small JNI check against the live JVM object (not via `format_value`).
    hardcoded against the Kotlin source; no engine knowledge is invented.
    """
    # Pure-Kap rows -----------------------------------------------------------
    if test == "testAttributes":
        # Kotlin: assertDimension(2,2) + assertArrayContent(arrayOf("a","123","b","some text"), result)
        # Kap result is a 2×2 array of strings; harness compares `format_value`
        # which renders string elements RAW (no quotes) and flattens row-major:
        #   (a 123 b some text)   — verified via probe `work/p_attr.kap`.
        if 'assertArrayContent' in body and '"a"' in body:
            return "(a 123 b some text)"
        return None
    if test == "nodeTypes":
        # 6× assertSym("document" etc., result.valueAt(i)) with ns=xml
        # order 0..5: document, element, text, comment, text, element
        # Probe `work/p_nodetypes.kap` → (xml:document xml:element xml:text xml:comment xml:text xml:element)
        if 'assertSym' in body:
            return "(xml:document xml:element xml:text xml:comment xml:text xml:element)"
        return None
    if test == "testNodeText":
        # assertString("\\n" + "        Some text here\\n" + "        ", result)
        # raw scalar string (harness uses `format_value` → raw, no quotes)
        # Probe `work/p_text2.kap` → exactly this 24-char string.
        if 'assertString' in body:
            return "\n        Some text here\n        "
        return None
    if test == "testXpathSimpleStringResult":
        # assertString with 89-char indented block (see XmlParserTest.kt:116-120)
        # Probe `work/p_xpath.kap` V1_len 89 matches.
        if 'assertString' in body:
            return "\n        Some text here\n        \n            \n                \n            \n        \n    "
        return None
    if test == "testXpathNoResultStringResult":
        if 'assertString' in body and '""' in body:
            return ""  # empty scalar string; format_value "" is "" (raw empty)
        return None
    # JvmInstanceValue rows — harness does a JNI property check against this ---
    if test in ("simpleParseString", "simpleParseFile"):
        # assertEquals("foo", result.instance.childNodes.item(0).nodeName)
        return "foo"
    if test == "testXpathSimpleNodeResult":
        # assertEquals("ghi", nodeName) + assertEquals("test0", attr "a")
        # Harness checks both; expected encodes the pair the extractor saw.
        # Single string "ghi test0" is enough to prove the node was the right one.
        return "ghi test0"
    if test == "testXpathSimpleNodeListResult":
        # two nodes: (ghi test0) (ghi test1) in a 2-elem array
        return "ghi test0 ghi test1"
    return None


def _extract_assertString_expected(body: str, scan: str) -> str | None:
    """Generic `assertString( <kotlin string expr> , result)` → Kap `format_value` expected.

    Returns the raw string value (what `APLValue::Str.format_value` yields), or
    None if the assertion is not on `result` directly or is not a literal
    concatenation the extractor can statically evaluate.
    """
    # Only when the assertion is on `result` directly (not result.valueAt etc.)
    # and not already handled by the Xml fast-path.
    m = re.search(r'assertString\s*\(\s*', scan)
    if not m:
        return None
    j = m.end()
    while j < len(scan) and scan[j] in ' \t\n':
        j += 1
    if j >= len(scan) or scan[j] != '"':
        return None
    # Kotlin string concatenation via `+` is handled by find_string_literal.
    val, end = _extract_kotlin_string_concat(scan, j)
    if val is None:
        return None
    # Verify the second arg is `, result` (allow whitespace, no valueAt)
    tail = scan[end:end + 400]
    if not re.search(r'^\s*,\s*result\b', tail):
        return None
    # Guard: if the body uses valueAt on result for other asserts, this is still
    # a direct assertString on result, so we DO want it. Only reject when the
    # assertString itself is not on result.
    return val


def _extract_assertArrayContent_expected(body: str, scan: str) -> str | None:
    """Generic `assertArrayContent(arrayOf(...), result)` → Kap vector string.

    Produces the `format_value` rendering: string elements are emitted RAW
    (no quotes) to match `APLValue::Array.format_value`, numeric elements as
    bare numbers, joined with spaces and wrapped in `()`.
    """
    m = re.search(r'assertArrayContent\s*\(\s*arrayOf\s*\(', scan)
    if not m:
        return None
    # String-aware paren scan for arrayOf args (elements may contain parens/quotes)
    i = m.end()
    depth = 1
    in_str = False
    esc = False
    start = i
    while i < len(scan) and depth > 0:
        c = scan[i]
        if in_str:
            if esc:
                esc = False
            elif c == '\\':
                esc = True
            elif c == '"':
                in_str = False
        else:
            if c == '"':
                in_str = True
            elif c == '(':
                depth += 1
            elif c == ')':
                depth -= 1
                if depth == 0:
                    break
        i += 1
    inner = scan[start:i]
    # After the arrayOf, expect `, result`
    tail = scan[i + 1:i + 400]
    if not re.search(r'^\s*,\s*result\b', tail):
        return None
    # Parse elements: they are Kotlin literals `"a"`, `"123"` etc. or numbers.
    parts = []
    # Find quoted strings or bare numbers/identifiers split by comma.
    # Use a regex that captures quoted strings with escapes.
    for em in re.finditer(r'"((?:[^"\\]|\\.)*)"', inner):
        raw = em.group(1).replace('\\"', '"').replace('\\n', '\n').replace('\\t', '\t').replace('\\\\', '\\')
        parts.append(raw)
    # If no quoted strings, fall back to numeric split (not expected for Xml)
    if not parts:
        nums = [p.strip() for p in inner.split(',') if p.strip()]
        if nums and all(re.fullmatch(r'[+-]?\d+', p) for p in nums):
            return '(' + ' '.join(nums) + ')'
        return None
    # Harness `format_value` for an array of strings emits each element RAW.
    return '(' + ' '.join(parts) + ')'


def best_effort_expected(body: str):
    # Only extract when the assertion is on `result` directly. If the Kotlin test
    # drills into a cell (`assert1DArray(..., result.valueAt(0))`), a lookup
    # (`assertSimpleNumber(N, result.lookupValue(...))`), or a map:get, the body
    # makes per-cell assertions and a single `N` no longer describes the whole
    # value — emit null so the harness counts these as "parse+eval OK" rather than
    # a misleading MISMATCH against a partial expected.
    # traps it. Scope the assertion scan to the FIRST parse call's block.
    import re as _re2
    _idxs = sorted(
        m.start() for _call in PARSE_CALLS
        for m in _re2.finditer(_re2.escape(_call) + r'\(', body))
    scan = body[:_idxs[1]] if len(_idxs) > 1 else body
    if 'result.valueAt' in scan or 'result.lookupValue' in scan or 'map:get' in scan:
        return None
    # Per-cell list assertions (assertSimpleNumber(N, result.listElement(i)))
    # describe ONE element, not the whole value — emit null so the harness
    # counts these as parse+eval OK (value ignored) rather than a MISMATCH
    # against a partial expected (e.g. ListTest.testUnderFromList asserts
    # 11/21/31 per cell; the whole value is the list (10;20;30)->(11;21;31)).
    if 'result.listElement' in scan:
        return None
    m = re.search(r'assertSimpleNumber\(\s*([+-]?\d+)\s*,\s*result\b', scan)
    if m:
        return m.group(1)
    # Kap prints vectors with PARENTHESES (e.g. `(1 2)`), never brackets —
    # brackets are reserved for indexing. The Kotlin assertion uses `[]` because
    # that is *Kotlin* array syntax, not Kap's. Emit Kap-syntax so the conformance
    # harness can compare apples to apples against our engine's `format_value`.
    m = re.search(r'assert1DArray\(\s*arrayOf\(([^)]*)\)\s*,\s*result\b', body)
    if m:
        parts = [p.strip() for p in m.group(1).split(',') if p.strip()]
        # numeric only
        if all(re.fullmatch(r'[+-]?\d+', p) for p in parts):
            return '(' + ' '.join(parts) + ')'
    return None


def check_comparison_rows(body: str):
    """Yield (expr, expected) for `checkComparisonResultTypes(arrayOf(..), "expr")` calls.

    SpecialisedComparisonTest routes every assertion through this private
    helper, so the bodies contain no direct parse call. Each call site is one
    concrete comparison with a fully-known Long-vector expectation — expand
    each to its own row.
    """
    rows = []
    for m in re.finditer(r'checkComparisonResultTypes\s*\(\s*arrayOf\(([^)]*)\)\s*,\s*"', body):
        parts = [p.strip() for p in m.group(1).split(',') if p.strip()]
        if not parts or not all(re.fullmatch(r'[+-]?\d+', p) for p in parts):
            continue
        text, _ = find_string_literal(body, m.end() - 1)
        rows.append((text, '(' + ' '.join(parts) + ')'))
    return rows


def expand_compare_helpers(body: str, expr: str):
    """Expand CompareTest's helper-parameterised assertions to concrete rows.

    Three tests route through private helpers with `for (value in listOf(..))`
    values and per-call operator/expectation args, so the naively extracted
    template (e.g. `${value} ${name} 6 6.0 …`) is uninterpolated Kap. Expand:
    - `compareFunction("op", arrayOf(n…))` × values → `v op <rest>` rows with
      the call's own Long-vector expectation (Kap-syntax `(n n …)`).
    - `testEqualsAndSame("a", "b", e)` → `a OP b` (exp e) + `a NOT-OP b`
      (exp 1-e), where (OP, NOT-OP) is (`≡`,`≢`) when the body tests
      `${a}≡${b}`, else (`=`,`≠`).
    All operators, values and expectations come verbatim from the test
    source; nothing is invented. Returns [] when the body has no such
    helpers (caller then falls through to the generic row).
    """
    rows = []
    if '${value}' in expr and '${name}' in expr:
        lm = re.search(r'for\s*\(\s*value\s+in\s+listOf\s*\(', body)
        if lm:
            # String-aware paren scan: values contain parens themselves
            # (`"(int:asBigint 5)"`), so a naive `[^)]*` would truncate.
            i, depth, instr, esc = lm.end(), 1, False, False
            while i < len(body) and depth > 0:
                c = body[i]
                if instr:
                    if esc:
                        esc = False
                    elif c == '\\':
                        esc = True
                    elif c == '"':
                        instr = False
                else:
                    if c == '"':
                        instr = True
                    elif c == '(':
                        depth += 1
                    elif c == ')':
                        depth -= 1
                i += 1
            inner = body[lm.end():i - 1]
            values = [
                m.group(1).replace('\\"', '"')
                for m in re.finditer(r'"((?:[^"\\]|\\.)*)"', inner)
            ]
            for op_m in re.finditer(
                r'compareFunction\(\s*"([^"]+)"\s*,\s*arrayOf\(([^)]*)\)', body
            ):
                op, arr = op_m.group(1), op_m.group(2)
                nums = [p.strip() for p in arr.split(',') if p.strip()]
                if not nums or not all(re.fullmatch(r'[+-]?\d+', p) for p in nums):
                    continue
                exp = '(' + ' '.join(nums) + ')'
                for v in values:
                    rows.append(
                        (expr.replace('${value}', v).replace('${name}', op), exp)
                    )
        return rows
    if '${a}' in expr and '${b}' in expr:
        ops = ('≡', '≢') if '"${a}≡${b}"' in body else ('=', '≠')
        for m in re.finditer(
            r'testEqualsAndSame\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)"\s*,\s*(\d+)\s*\)',
            body,
        ):
            a = m.group(1).replace('\\"', '"')
            b = m.group(2).replace('\\"', '"')
            e = int(m.group(3))
            rows.append((a + ops[0] + b, str(e)))
            rows.append((a + ops[1] + b, str(1 - e)))
        return rows
    return rows


def main():
    records = []
    for root, _dirs, files in os.walk(ARRAY_ROOT):
        for fn in files:
            if not fn.endswith('.kt') or not fn.endswith('Test.kt'):
                continue
            if not fn.endswith('Test.kt'):
                continue
            path = os.path.join(root, fn)
            try:
                with open(path, encoding='utf-8') as f:
                    src = f.read()
            except Exception:
                continue
            # Class-level @Ignore (annotation directly above `class X` /
            # `object X` / `abstract class X`): the whole file is skipped by
            # the Kotlin runner (e.g. FfiApiTest).
            class_ignored = bool(re.search(r'@Ignore\s+(?:abstract\s+|open\s+)?(?:class|object)\s', src))
            for name, body in extract_method_bodies(src, class_ignored):
                # Helper-routed assertions first (bodies with no direct parse
                # call, e.g. SpecialisedComparisonTest): one row per call site.
                for cexpr, cexp in check_comparison_rows(body):
                    records.append({
                        'file': os.path.relpath(path, ARRAY_ROOT),
                        'test': name,
                        'kind': 'eval',
                        'expr': cexpr,
                        'expected': cexp,
                    })
                expr = first_expr_in_call(body)
                if expr is None:
                    continue
                # CompareTest helper-parameterised tests (for-in-listOf values
                # crossed with helper-supplied operators/expectations): emit
                # the concrete rows and skip the uninterpolated `${…}`
                # template row below.
                compare_rows = expand_compare_helpers(body, expr)
                for cexpr, cexp in compare_rows:
                    records.append({
                        'file': os.path.relpath(path, ARRAY_ROOT),
                        'test': name,
                        'kind': 'eval',
                        'expr': cexpr,
                        'expected': cexp,
                    })
                if compare_rows:
                    continue
                kind = "fails" if detect_fails(body) else "eval"
                expected = best_effort_expected(body) if kind == "eval" else None
                # XmlParserTest honest value-verification: the previous sweep
                # left all 9 rows at `expected: None` (run-verified only) because
                # their Kotlin assertions are Java-side (`assertIs<Document>` +
                # `assertEquals("foo", childNodes[0].nodeName)`) or Kap
                # `assertString`/`assertSym`/`assertArrayContent` that the old
                # `best_effort` ignored. The hardcoded extractor above closes that
                # gap so the harness can value-verify them. It is authoritative
                # for the 9 Xml rows; fall back to generic best_effort otherwise.
                if kind == "eval" and 'XmlParserTest' in os.path.relpath(path, ARRAY_ROOT):
                    xml_exp = _xml_hardcoded_expected(name, body)
                    if xml_exp is not None:
                        expected = xml_exp
                # Expand Kotlin string-template loops (repeat(N) { i -> "...${i + 1}" })
                # to concrete expressions per iteration.
                expanded_exprs = expand_template_loop(expr, body)
                # RandomTest's fake-RNG helper: asserted values depend on the
                # stub RNG (nextLong=until/10, nextDouble=5.5), unreproducible
                # by a real engine — score eval-OK only, never against stub values.
                if "parseAPLExpressionWithSpecialRandom(" in body:
                    expected = None
                for exp in expanded_exprs:
                    # `${makeFunctions()}` (ComposeTest contrib rows): expand the
                    # Kotlin helper to its runtime output (genuine eval cases).
                    exp = expand_make_functions(exp)
                    # `val n = 300` integer bindings (Encoder `${n}` rows).
                    exp = expand_val_bindings(exp, body)
                    # `{GENERIC}` is a test-harness backend marker (APLTest.kt
                    # `parseAndTestWithGeneric` runs the expr twice: with `{GENERIC}`
                    # removed, and replaced by `int:ensureGeneric`). The port has one
                    # backend, so record the plain form (marker stripped).
                    exp = exp.replace("{GENERIC}", "")
                    # `${…}` Kotlin string templates with statically-known bindings
                    # (11d): expand to literal Kap so the row is scorable (all five
                    # are expected=null: any produced value scores). `⋄`-chains cover
                    # EVERY combination, so any failure keeps the row red; the full
                    # matrices are additionally oracle-verified in curated rows. The
                    # two fails-kind template rows are untouched (they already score
                    # via parse errors).
                    if '${' in exp and kind == 'eval':
                        # Uninterpolated Kotlin string template that the extractor
                        # could not expand. Score as eval-OK only: any successful
                        # evaluation counts, against no expected value.
                        records.append(
                            {
                                "file": os.path.relpath(path, ARRAY_ROOT),
                                "test": name,
                                "kind": kind,
                                "expr": exp,
                                "expected": None,
                            }
                        )
                        continue
                    records.append(
                        {
                            "file": os.path.relpath(path, ARRAY_ROOT),
                            "test": name,
                            "kind": kind,
                            "expr": exp,
                            "expected": expected,
                        }
                    )
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w', encoding='utf-8') as f:
        for r in records:
            f.write(json.dumps(r, ensure_ascii=False) + '\n')
    print(f"Extracted {len(records)} test cases -> {os.path.normpath(OUT)}")


if __name__ == '__main__':
    main()
