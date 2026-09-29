#!/usr/bin/env bash
# Owner-behavior check for a JS package manager cache. Installs into a scratch
# cache, damages it like a partial Clean, reinstalls in a fresh project, and
# compares the installed files with the reference install. Needs network access.
# Usage: js.sh <npm|pnpm|bun|yarn> <damage.py|interrupt.py> <fraction>
set -u
tool=$1 damage=$2 fraction=$3
here=$(cd "$(dirname "$0")" && pwd)
base=${OWNER_CHECK_DIR:-${TMP:-/tmp}/cleaner-owner-checks}/$tool-${damage%.py}-$fraction
# Refuse to delete an existing folder that an owner check did not create.
if [ -e "$base" ] && [ ! -e "$base/.cleaner-owner-check" ]; then
  echo "refusing to delete $base: it was not created by an owner check" >&2; exit 1
fi
rm -rf "$base"; mkdir -p "$base"; touch "$base/.cleaner-owner-check"
cache="$base/cache"
pkgs="lodash@4.17.21 chalk@5.3.0 typescript@5.4.5"

install() { # <project dir>
  mkdir -p "$1"; cd "$1" || return 1
  echo '{"name":"probe","version":"1.0.0","private":true}' > package.json
  case $tool in
    npm)  npm_config_cache="$cache" npm_config_logs_dir="$base/logs" npm install --no-audit --no-fund --loglevel=error $pkgs >/dev/null ;;
    pnpm) pnpm add --store-dir "$cache" --config.confirmModulesPurge=false $pkgs >/dev/null ;;
    bun)  BUN_INSTALL_CACHE_DIR="$cache" bun add $pkgs >/dev/null ;;
    yarn) YARN_CACHE_FOLDER="$cache" npx -y yarn@1.22.22 add $pkgs --silent --no-progress >/dev/null ;;
  esac
  local status=$?
  cd - >/dev/null
  return $status
}

# Hashes the installed packages' files, following pnpm's links. Nested `.bin`
# shims embed the project path, so they are excluded.
digest() {
  (cd "$1/node_modules" && for p in lodash chalk typescript; do
     find -L "$p" -path "*/node_modules/.bin" -prune -o -type f -print0 | sort -z | xargs -0 sha256sum
   done) | sha256sum | cut -c1-16
}

install "$base/reference" || { echo "$tool: reference install failed"; exit 1; }
ref=$(digest "$base/reference")
echo "$tool: $(python "$here/$damage" "$cache" "$fraction")"
for attempt in 1 2; do
  if install "$base/after-$attempt"; then
    [ "$(digest "$base/after-$attempt")" = "$ref" ] \
      && echo "$tool $damage $fraction: install $attempt PASS (files match)" \
      || echo "$tool $damage $fraction: install $attempt FAIL (installed files differ)"
    break
  fi
  echo "$tool $damage $fraction: install $attempt FAIL (exited non-zero)"
done
