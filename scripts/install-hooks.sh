#!/bin/sh
set -eu
cd "$(git rev-parse --show-toplevel)"
if [ ! -x .githooks/pre-push ]; then
  echo 'Hook missing or not executable. Restore .githooks/pre-push from this checkout.' >&2
  exit 1
fi
configured=true
current=$(git config --get core.hooksPath) || {
  code=$?
  if [ "$code" -ne 1 ]; then exit "$code"; fi
  configured=false
  current=''
}
if [ "$configured" = true ] && [ "$current" != '.githooks' ]; then
  echo "Refusing to replace existing core.hooksPath: $current" >&2
  exit 1
fi
if [ "$configured" = false ]; then
  hooks=$(git rev-parse --git-path hooks)
  for hook in "$hooks"/* "$hooks"/.[!.]* "$hooks"/..?*; do
    [ -f "$hook" ] && [ -x "$hook" ] || continue
    case "$hook" in *.sample) continue ;; esac
    echo "Refusing to hide existing executable hook: $hook" >&2
    exit 1
  done
fi
git config --local core.hooksPath .githooks
printf '%s\n' 'Installed opt-in pre-push hook. No dependencies installed or hooks run.'
