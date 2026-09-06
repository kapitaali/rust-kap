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

PARSE_CALLS = ("parseAPLExpression", "parseAPLExpressionWithTest", "parseAndTestWithGeneric")


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


def extract_method_bodies(src: str):
    """Yield (name, body) for each `@Test fun <name>(...) { ... }`."""
    for m in re.finditer(r'@Test\s+fun\s+(\w+)\s*\(([^)]*)\)\s*\{', src):
        name = m.group(1)
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
            # Kotlin triple-quoted strings still process backslash escapes.
            text = text.replace('\\\\', '\x00').replace('\\"', '"').replace('\x00', '\\')
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
    return 'assertFailsWith' in body


def best_effort_expected(body: str):
    """Pull a single-line assertSimpleNumber(N, ...) / assert1DArray(arrayOf(...), ...) expectation.

    Expected is paired with the FIRST parse call's result only: scan only the
    body up to the second parse call (if any). Without this, the extractor
    pairs expr #1 with an assertion on expr #3 (e.g. TransposeTest reverse*
    asserted the shape/content of `⌽4 5 4⍴⍳1000` but got expected='1' from
    the trailing `⌽1` scalar check).
    """
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
            for name, body in extract_method_bodies(src):
                expr = first_expr_in_call(body)
                if expr is None:
                    continue
                kind = 'fails' if detect_fails(body) else 'eval'
                expected = best_effort_expected(body) if kind == 'eval' else None
                # `{GENERIC}` is a test-harness backend marker (APLTest.kt
                # `parseAndTestWithGeneric` runs the expr twice: with `{GENERIC}`
                # removed, and replaced by `int:ensureGeneric`). The port has one
                # backend, so record the plain form (marker stripped).
                expr = expr.replace('{GENERIC}', '')
                records.append({
                    'file': os.path.relpath(path, ARRAY_ROOT),
                    'test': name,
                    'kind': kind,
                    'expr': expr,
                    'expected': expected,
                })
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w', encoding='utf-8') as f:
        for r in records:
            f.write(json.dumps(r, ensure_ascii=False) + '\n')
    print(f"Extracted {len(records)} test cases -> {os.path.normpath(OUT)}")


if __name__ == '__main__':
    main()
