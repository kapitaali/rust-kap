#!/usr/bin/env bash
ORACLE=~/Apps/array/kap-jvm-text/bin/kap-jvm-text
probe() {
  local e="$1"
  local out
  out=$(printf '%s\n' "$e" | "$ORACLE" --lib-path="$HOME/Apps/array/kap-jvm-text/standard-lib" 2>&1)
  local or er
  or=$(echo "$out" | grep -a '⊢' | grep -v WARNING | sed 's/.*⊢ //' | head -1)
  er=$(echo "$out" | grep -ai 'error at' | grep -v WARNING | head -1)
  if [ -n "$or" ]; then printf '%-26s => %s\n' "$e" "$or";
  elif [ -n "$er" ]; then printf '%-26s => ERR:%s\n' "$e" "$er";
  else printf '%-26s => <none>\n' "$e"; fi
}
probe '({⍵+10}⍣3) 5'
probe '({⍵+10}⍣0) 5'
probe '({⍵+200}⍣1) 5'
probe '(×⍣¯1)5'
probe '(io:print⍣(3)) 10'
probe '(+⍣2) 10'
probe '({⍵×2}⍣5) 1'
probe '({⍵+1}⍣3) 1'
