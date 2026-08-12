#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

strategy_bins() {
    awk -F'"' '/^name = /{name=$2} /^path = "strategies\//{print name}' Cargo.toml
}

usage() {
    echo "usage: $0 <strategy-bin-name>" >&2
    echo "strategy bins:" >&2
    strategy_bins | sed 's/^/  /' >&2
    exit 1
}

[ $# -eq 1 ] || usage
BIN="$1"
strategy_bins | grep -qxF -- "$BIN" || { echo "unknown strategy bin: $BIN" >&2; usage; }

docker buildx build \
    --platform linux/arm64 \
    --build-arg "BIN=$BIN" \
    --output "type=local,dest=dist" \
    .

file "dist/$BIN"
ls -lh "dist/$BIN"
