#!/usr/bin/env bash
# Owner-behavior checks for pip, Cargo, and Go caches. Fills a scratch cache,
# damages it like a partial Clean, and checks that the owner still installs or
# builds correctly without a repair step. Needs network access.
# Usage: native.sh <pip|cargo|go> <damage.py|interrupt.py> <fraction>
set -u
tool=$1 damage=$2 fraction=$3
here=$(cd "$(dirname "$0")" && pwd)
base=${OWNER_CHECK_DIR:-${TMP:-/tmp}/cleaner-owner-checks}/$tool-${damage%.py}-$fraction
rm -rf "$base"; mkdir -p "$base"

case $tool in
  pip)
    cache="$base/cache"
    run() { PIP_CACHE_DIR="$cache" python -m pip install --quiet --disable-pip-version-check \
              --target "$base/$1" requests==2.32.3 six==1.16.0 >/dev/null 2>&1; }
    # Launchers, bytecode, and RECORD embed the install path, so they are excluded.
    digest() { (cd "$base/$1" && find . -path ./bin -prune -o -name __pycache__ -prune -o -name RECORD -prune -o -type f -print0 | sort -z | xargs -0 sha256sum) | sha256sum | cut -c1-16; }
    ;;
  cargo)
    # A scratch CARGO_HOME; the Target is its registry folder.
    export CARGO_HOME="$base/home"
    mkdir -p "$base/proj/src"
    printf '[package]\nname="probe"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nserde={version="=1.0.203",features=["derive"]}\nitoa="=1.0.11"\n' > "$base/proj/Cargo.toml"
    echo 'fn main(){ println!("{}", itoa::Buffer::new().format(7)); }' > "$base/proj/src/main.rs"
    cache="$CARGO_HOME/registry"
    run() { rm -rf "$base/proj/target"; (cd "$base/proj" && cargo run --quiet 2>&1 | tail -3) | grep -qx 7; }
    digest() { echo same; }
    ;;
  go)
    export GOCACHE="$base/cache" GOFLAGS=-mod=mod GOPATH="$base/gopath"
    mkdir -p "$base/proj"
    printf 'module probe\n\ngo 1.24\n' > "$base/proj/go.mod"
    printf 'package main\n\nimport (\n\t"fmt"\n\t"net/http"\n\t"encoding/json"\n)\n\nfunc main() { b, _ := json.Marshal(http.StatusOK); fmt.Println(string(b)) }\n' > "$base/proj/main.go"
    cache="$GOCACHE"
    run() { (cd "$base/proj" && go run . 2>&1) | grep -qx 200; }
    digest() { echo same; }
    ;;
esac

run reference || { echo "$tool: reference failed"; exit 1; }
ref=$(digest reference)
echo "$tool: $(python "$here/$damage" "$cache" "$fraction")"
if run after-damage && [ "$(digest after-damage)" = "$ref" ]; then
  echo "$tool $damage $fraction: PASS"
else
  echo "$tool $damage $fraction: FAIL"
fi
