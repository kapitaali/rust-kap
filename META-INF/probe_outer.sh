#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
BIN=./target/debug/kap
echo "=== ORACLE: outer product ∘.f ==="
probe_o() {
  local e="$1"
  or=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  er=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1 | grep -ai 'error at' | grep -v WARNING | head -1)
  printf '  %-22s => %s\n' "$e" "${or:-${er:-<none>}}"
}
probe_o '1 2 3 ∘.+ 4 5 6'
probe_o '1 2 ∘.× 3 4'
probe_o '1 2 ∘.= 1 2 1'
probe_o '∘.+'
echo "=== PORT ==="
probe_p() {
  local e="$1"
  po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -a '>>>' | grep -av '^>>> *$' | head -1 | sed 's/^>>> //')
  [ -z "$po" ] && po=$(printf '%s\n' "$e" | "$BIN" --no-standard-lib 2>&1 | grep -ai 'error' | head -1)
  printf '  %-22s => %s\n' "$e" "${po:-<none>}"
}
probe_p '1 2 3 ∘.+ 4 5 6'
probe_p '1 2 ∘.× 3 4'
probe_p '∘.+'
