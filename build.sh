#!/bin/bash
# build.sh – build the maudebox Docker image
#
# Usage:
#   ./build.sh [--mvnd-version VERSION] [--jj-version VERSION] [--tag TAG]
#
# Defaults:
#   --mvnd-version  1.0.5
#   --jj-version    0.34.0
#   --tag           maudebox

set -euo pipefail

MVND_VERSION="1.0.5"
JJ_VERSION="0.34.0"
TAG="maudebox"

# ── parse flags ───────────────────────────────────────────────────────────────
while [[ "${1:-}" == --* ]]; do
    case "$1" in
        --mvnd-version) MVND_VERSION="$2"; shift 2 ;;
        --jj-version)   JJ_VERSION="$2";   shift 2 ;;
        --tag)          TAG="$2";           shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Building image: $TAG"
echo "  mvnd : $MVND_VERSION"
echo "  jj   : $JJ_VERSION"

docker build \
    --build-arg MVND_VERSION="$MVND_VERSION" \
    --build-arg JJ_VERSION="$JJ_VERSION" \
    -t "$TAG" \
    "$SCRIPT_DIR"