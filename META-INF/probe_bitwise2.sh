#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
o() { local e="$1"; local r; r=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1); [ -z "$r" ] && r=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1); echo "  $e => ${r:-?}"; }
echo "=== monadic bitwise (single arg 13) ==="
for g in '×' '+' '-' '∧' '∨' '≠' '=' '⍲' '⍱' '<' '>' '≤' '≥' '⌽' '|'; do o "${g}∵ 13"; done
echo "=== array popcount ==="
o '⍸∵ 0 1 0b1111000'
o '⍸∵ 255'
o '⍸∵ ¯1'
echo "=== bigint/long shift range ==="
o '2 ⌽∵ 5'
o '¯2 ⌽∵ 5'
o '100 ⌽∵ 5'
echo "=== negative operand bitwise ==="
o '¯99 ∧∵ 70'
o '99 ∨∵ ¯70'
