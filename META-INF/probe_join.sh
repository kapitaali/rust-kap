#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
BIN=./target/debug/kap
cmp() {
  local e="$1"
  local po
  po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -a '>>>' | grep -av '^>>> *$' | head -1 | sed 's/^>>> //')
  [ -z "$po" ] && po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -ai 'error' | head -1)
  local or
  or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  [ -z "$or" ] && or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1)
  printf '%-28s | port: %-26s | oracle: %-26s\n' "$e" "${po:-<none>}" "${or:-<none>}"
}
echo "=== OUTER PRODUCT ∘∙ ==="
cmp '1 2 ∘∙× 1 2 3 4'
cmp '1 2 ∘∙{⍺,⍵} 9 8 7 6'
cmp '(15+⍳4)∘∙+10'
cmp '10∘∙+(15+⍳4)'
echo "=== INNER PRODUCT ∙ ==="
cmp '100 200 300+∙×5 6 7'
cmp '(1+⍳8) +∙× (10+⍳8)'
cmp '10 +∙× ⍳8'
cmp '1 2 3 ,∙, ⍪4 5 6'
echo "=== SCALAR OUTER ⌻ ==="
cmp '10 +⌻ 11'
cmp '1 2 3 -⌻ ×/1+⍳4'
