#!/usr/bin/env bash
# Side-by-side oracle vs port probe for ˝ (inverse adverb) cases.
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
LIB=$HOME/Apps/array/kap-jvm-text/standard-lib
PORT=./target/debug/kap

cases=(
  '((1+)˝) 8'
  '(1+)˝ 8'
  '(2×)˝ 5'
  '(10+)˝ 3'
  '(÷)˝ 8'
  '(1+2×)˝ 8'
  '(1+)˝ 2 3 4'
  '(1+) 8'
  '(÷) 8'
  '(+/)˝ 1 2 3 4'
  '((10+)˝) 3'
)

# Oracle: first non-WARNING, non-empty line that has content after '⊢ '.
orc() {
  printf '%s\n' "$1" | "$ORACLE" --lib-path="$LIB" 2>&1 \
    | grep -av '^WARNING' | grep -av '^$' \
    | awk '/⊢ /{sub(/^⊢ /,""); print; exit}'
}
# Port: last line matching '>>> ' (trailing space) — keeps result, drops bare prompt.
prt() {
  printf '%s\n' "$1" | "$PORT" --no-standard-lib 2>&1 \
    | grep -aE '>>> .+' | tail -1 | sed 's/^>>> //'
}

printf '%-22s | %-22s | %s\n' 'EXPR' 'ORACLE' 'PORT'
for c in "${cases[@]}"; do
  o=$(orc "$c")
  p=$(prt "$c")
  printf '%-22s | %-22s | %s\n' "$c" "$o" "$p"
done
