#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
BIN=./target/debug/kap
cmp() {
  local e="$1"
  local po er
  po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -a '>>>' | grep -av '^>>> *$' | head -1 | sed 's/^>>> //')
  [ -z "$po" ] && po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -ai 'error' | head -1)
  local or
  or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  [ -z "$or" ] && or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1)
  local pn pd
  pn=$(echo "$po" | tr '¯' '-' | sed 's/[⟨⟩() ]//g'); pd=$(echo "$or" | tr '¯' '-' | sed 's/[⟨⟩() ]//g')
  local m="OK"; [ "$pn" != "$pd" ] && m="MISMATCH"
  printf '%-26s | port: %-22s | oracle: %-22s | %s\n' "$e" "${po:-<none>}" "${or:-<none>}" "$m"
}
cmp '({⍵+10}⍣3) 5'
cmp '({⍵+10}⍣0) 5'
cmp '({⍵+200}⍣1) 5'
cmp '(×⍣¯1)5'
cmp '(+⍣2) 10'
cmp '({⍵×2}⍣5) 1'
cmp '({⍵+1}⍣3) 1'
cmp '({⍵-1}⍣{⍵=0}) 9'
cmp '({⍵-1}⍣{⍺=1}) 9'
