#!/usr/bin/env python3
"""Extract raw Kap source expressions from the Kotlin *Test.kt reference suite.

For every `@Test fun <name>(...) { ... }` that calls one of
`parseAPLExpression`, `parseAPLExpressionWithTest`, or `parseAndTestWithGeneric`
with a string-literal argument, extract that string (the Kap program under test),
un-escape Kotlin string escapes, and write it to a `.kap` file under
`kap-stdlib/test/extracted/`. The result is verbatim Kap source you can run on the
real Kotlin Kap engine — no assertion wrapper, no translation.

Usage:
    python3 tools/extract_kap_sources.py [ARRAY_ROOT]
"""
import os
import re
import sys

ARRAY_ROOT = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/Apps/array")
OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "kap-stdlib", "test", "extracted")


def find_string_literal(s: str, start: int):
    """Given s[start] == '"', return (inner_text, index_after_closing_quote)."""
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


PARSE_CALLS = ("parseAPLExpressionWithTest", "parseAPLExpression", "parseAndTestWithGeneric")


def first_expr_in_call(body: str):
    for call in PARSE_CALLS:
        idx = body.find(call + '(')
        if idx == -1:
            continue
        j = idx + len(call) + 1
        while j < len(body):
            if body[j] == '"':
                return find_string_literal(body, j)[0]
            # parseAPLExpressionWithTest("...", { lambda }) -> expr is the first string
            if body[j] == '{':
                q = body.find('"', j)
                if q != -1:
                    return find_string_literal(body, q)[0]
                return None
            j += 1
    return None


def extract_method_bodies(src: str):
    for m in re.finditer(r'@Test\s+fun\s+(\w+)\s*\(([^)]*)\)\s*\{', src):
        name = m.group(1)
        body_start = m.end() - 1
        body_end = find_balanced(src, body_start)
        yield name, src[body_start + 1:body_end]


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    count = 0
    skipped = 0
    for root, _dirs, files in os.walk(ARRAY_ROOT):
        for fn in files:
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
                    skipped += 1
                    continue
                # skip: no expr, empty, or the {GENERIC} placeholder ("test" used by
                # parseAndTestWithGeneric) which is not real Kap source.
                if expr.strip() == '' or expr.strip() == 'test':
                    skipped += 1
                    continue
                out_name = f"{fn[:-len('.kt')]}__{name}.kap"
                out_path = os.path.join(OUT_DIR, out_name)
                with open(out_path, 'w', encoding='utf-8') as f:
                    f.write(expr)
                    if not expr.endswith('\n'):
                        f.write('\n')
                count += 1
    print(f"Extracted {count} Kap source files -> {os.path.normpath(OUT_DIR)} "
          f"({skipped} @Test cases had no extractable Kap expr)")


if __name__ == '__main__':
    main()
