#!/usr/bin/env python3
"""Extract raw Kap source expressions from the Kotlin *Test.kt reference suite.

For every `@Test fun <name>(...) { ... }` that calls one of
`parseAPLExpression`, `parseAPLExpressionWithTest`, or `parseAndTestWithGeneric`
with a string-literal argument, extract that string (the Kap program under test),
un-escape Kotlin string escapes, and write it to a `.kap` file under
`kap-stdlib/test/extracted/`. The result is verbatim Kap source you can run on the
real Kotlin Kap engine — no assertion wrapper, no translation.

ALSO captures the Kotlin assertion(s) on `result` (e.g. `assert1DArray(...)`,
`assertSimpleNumber(...)`) and writes them:
  - as a `⍝ assert: ...` header comment inside the `.kap`, and
  - verbatim into a sidecar `<name>.assert` file (one assertion per line)
so you can compare the Kotlin-expected value without flipping back to the .kt.

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

# Kotlin assertion helpers that operate on `result` (the evaluated value).
ASSERT_RE = re.compile(r'\b(assert\w+|assertEquals|assertTrue|assertFalse|assertFailsWith)\s*')


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


def _balanced_args(s: str, start: int):
    """Given s[start] == '(', return the full '(...) ' substring with balanced parens."""
    assert s[start] == '('
    depth = 0
    i = start
    while i < len(s):
        c = s[i]
        if c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                return s[start:i + 1]
        i += 1
    return s[start:]


def extract_assertions(body: str):
    """Return the list of assertion calls found in the test body, verbatim."""
    out = []
    for m in ASSERT_RE.finditer(body):
        helper = m.group(1)
        if helper == 'assertFailsWith':
            gen = re.search(r'assertFailsWith<([^>]+)>', body)
            exc = gen.group(1) if gen else '?'
            out.append(f"assertFailsWith<{exc}>")
            continue
        # the args start at the '(' right after the helper name
        j = m.end()
        while j < len(body) and body[j] != '(':
            j += 1
        if j < len(body) and body[j] == '(':
            args = _balanced_args(body, j)
            out.append(f"{helper}{args}")
    return out


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
                # skip: no expr, empty, or the {GENERIC} placeholder ("test" used by
                # parseAndTestWithGeneric) which is not real Kap source.
                if expr is None or expr.strip() == '' or expr.strip() == 'test':
                    skipped += 1
                    continue
                base = f"{fn[:-len('.kt')]}__{name}"
                kap_path = os.path.join(OUT_DIR, base + ".kap")
                assertions = extract_assertions(body)
                # write the .kap with an assertion header comment
                header = "".join(f"⍝ assert: {a}\n" for a in assertions)
                with open(kap_path, 'w', encoding='utf-8') as f:
                    if header:
                        f.write(header)
                    f.write(expr)
                    if not expr.endswith('\n'):
                        f.write('\n')
                # write sidecar .assert (verbatim, one per line)
                if assertions:
                    with open(os.path.join(OUT_DIR, base + ".assert"), 'w', encoding='utf-8') as f:
                        f.write("\n".join(assertions) + "\n")
                count += 1
    print(f"Extracted {count} Kap source files -> {os.path.normpath(OUT_DIR)} "
          f"({skipped} @Test cases had no extractable Kap expr)")


if __name__ == '__main__':
    main()
