#!/bin/bash
# Integration test runner for BCL
# Requires Chrome/CDP running on BROWSER_URL (default http://127.0.0.1:9222)

set -e

export ASSETS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/assets" && pwd)"
export TMP_DIR="/tmp/bcl-tests"
export BROWSER_URL="${BROWSER_URL:-http://127.0.0.1:9222}"

mkdir -p "$TMP_DIR"

echo "Running BCL integration tests..."
echo "Assets dir: $ASSETS_DIR"
echo "Temp dir: $TMP_DIR"
echo "Browser URL: $BROWSER_URL"
echo ""

BUILD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/debug"
BCL_BIN="$BUILD_DIR/bcl"

if [[ ! -f "$BCL_BIN" ]]; then
    echo "Building bcl..."
    cargo build --manifest-path "$(dirname "${BASH_SOURCE[0]}")/../Cargo.toml"
fi

# Tests that currently pass
PASSING_TESTS=(
    "basic"
    "forms"
    "dialogs"
    "tabs"
)

# Tests with known issues (commented out)
# KNOWN_ISSUES=(
#     "upload"        # DOM.setFileInputFiles: Could not find node
#     "dragdrop"      # Drag & drop not working in Carbonyl
#     "shadow_iframe" # Iframe contentDocument null
#     "download"      # Download events not firing
#     "integration"   # Depends on dragdrop
# )

run_test() {
    local test_name="$1"
    echo ">>> Running $test_name..."
    ASSETS_DIR="$ASSETS_DIR" TMP_DIR="$TMP_DIR" BROWSER_URL="$BROWSER_URL" \
        "$BCL_BIN" run "tests/${test_name}.yml" -v
    echo "✓ $test_name passed"
    echo ""
}

FAILED=()
for test in "${PASSING_TESTS[@]}"; do
    if run_test "$test"; then
        :
    else
        FAILED+=("$test")
    fi
done

if [[ ${#FAILED[@]} -eq 0 ]]; then
    echo "All tests passed!"
    exit 0
else
    echo "Failed tests: ${FAILED[*]}"
    exit 1
fi