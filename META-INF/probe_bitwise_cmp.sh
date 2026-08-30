#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
BIN=./target/debug/kap
cmp() {
  local e="$1"
  local po er
  po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -a '>>>' | grep -av '^>>> *$' | head -1 | sed 's/^>>> //')
  [ -z "$po" ] && er=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -ai 'error' | head -1)
  local or
  or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  [ -z "$or" ] && or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1)
  local pn pd
  pn=$(echo "${po:-$er}" | tr '¯' '-' | sed 's/[⟨⟩() ]//g'); pd=$(echo "$or" | tr '¯' '-' | sed 's/[⟨⟩() ]//g')
  local m="OK"; [ "$pn" != "$pd" ] && m="MISMATCH"
  printf '%-22s | port: %-22s | oracle: %-22s | %s\n' "$e" "${po:-${er:-<none>}}" "${or:-<none>}" "$m"
}
echo "=== dyadic (99,70) ==="
for g in '×' '+' '-' '|' '∧' '∨' '≠' '=' '⍲' '⍱' '<' '>' '≤' '≥' '⌽'; do cmp "99 ${g}∵ 70"; done
echo "=== monadic ==="
cmp '~∵ 13'
cmp '⍴∵ 13'
cmp '⍸∵ 0b1101'
cmp '⍸∵ 0 1 0b1111000'
cmp '⍸∵ 255'
cmp '⍸∵ ¯1'
echo "=== shift range / negatives ==="
cmp '2 ⌽∵ 5'
cmp '¯2 ⌽∵ 5'
cmp '100 ⌽∵ 5'
cmp '¯99 ∧∵ 70'
cmp '99 ∨∵ ¯70'
echo "=== array popcount ==="
cmp '⍸∵ (0 1 0b1111000) (0 0 255)'
