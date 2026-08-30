#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
row() {
  local g="$1"
  # dyadic: 99 g∵ 70 ; monadic: ~∵, ⍴∵, ⍸∵ (operate on 0b1101 = 13 and 0b1011=11)
  local d m1 m2 m3
  d=$(printf '99 %s∵ 70\n' "$g" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  [ -z "$d" ] && d=$(printf '99 %s∵ 70\n' "$g" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1)
  echo "  $g∵ (99,70) => ${d:-?}"
}
for g in '×' '+' '-' '|' '∧' '∨' '≠' '=' '⍲' '⍱' '<' '>' '≤' '≥' '⌽'; do row "$g"; done
echo "--- monadic ---"
mp() {
  local e="$1" o
  o=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  [ -z "$o" ] && o=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1)
  echo "  $e => ${o:-?}"
}
mp '~∵ 13'
mp '⍴∵ 13'
mp '⍸∵ 0b1101'
mp '⍸∵ 0 1 0b1111000'
