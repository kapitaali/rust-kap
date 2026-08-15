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
    """Return (inner_text, end_index_after_closing_quote) for a "..." starting at s[start]=='\"'."""
    assert s[start] == '"'
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
            return ''.join(buf), i + 1
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
    """Find the first parseAPLExpression(...) call and return its raw string-literal arg."""
    for call in PARSE_CALLS:
        idx = body.find(call + '(')
        if idx == -1:
            continue
        # walk to first string literal
        j = idx + len(call) + 1
        while j < len(body):
            if body[j] == '"':
                text, _ = find_string_literal(body, j)
                return text
            if body[j] == '{':  # parseAPLExpressionWithTest("...", callback = { ... })
                # the first string before the callback lambda is the expr
                text, _ = find_string_literal(body, j + 1) if False else (None, 0)
                # fallback: search forward for first '"'
                q = body.find('"', j)
                if q != -1:
                    text, _ = find_string_literal(body, q)
                    return text
                return None
            j += 1
    return None


def detect_fails(body: str):
    return 'assertFailsWith' in body


def best_effort_expected(body: str):
    """Pull a single-line assertSimpleNumber(N, ...) / assert1DArray(arrayOf(...), ...) expectation."""
    m = re.search(r'assertSimpleNumber\(\s*([+-]?\d+)\s*,', body)
    if m:
        return m.group(1)
    # assert1DArray(arrayOf(a, b, ...), result) -> "[a b ...]"
    m = re.search(r'assert1DArray\(\s*arrayOf\(([^)]*)\)\s*,', body)
    if m:
        parts = [p.strip() for p in m.group(1).split(',') if p.strip()]
        # numeric only
        if all(re.fullmatch(r'[+-]?\d+', p) for p in parts):
            return '[' + ' '.join(parts) + ']'
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
