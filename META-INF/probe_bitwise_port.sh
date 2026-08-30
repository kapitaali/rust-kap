#!/usr/bin/env bash
BIN=./target/debug/kap
p() { local e="$1"; local o; o=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -a '>>>' | grep -av '^>>> *$' | head -1 | sed 's/^>>> //'); [ -z "$o" ] && o=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -ai 'error' | head -1); echo "  $e => ${o:-?}"; }
echo "=== port CURRENT bitwise (99,70) ==="
for g in '×' '+' '-' '∧' '∨' '≠' '=' '⍲' '⍱' '<' '>' '≤' '≥' '⌽'; do p "99 ${g}∵ 70"; done
echo "=== port monadic ==="
for g in '~' '⍴' '⍸'; do p "${g}∵ 13"; done
p '⍸∵ 0 1 0b1111000'
p '⍸∵ ¯1'
