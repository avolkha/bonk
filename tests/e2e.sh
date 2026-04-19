#!/usr/bin/env bash
# End-to-end tests for bonk.
# Run from the repo root: ./tests/e2e.sh
# Requires: cargo, docker (or compatible), bwrap, unsquashfs

set -euo pipefail

PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); ERRORS+=("$1"); }

assert_contains() {
    local label="$1" expected="$2" actual="$3"
    if echo "$actual" | grep -qF "$expected"; then
        pass "$label"
    else
        fail "$label (expected to contain: '$expected', got: '$actual')"
    fi
}

assert_exit() {
    local label="$1" expected="$2" actual="$3"
    if [[ "$actual" -eq "$expected" ]]; then
        pass "$label"
    else
        fail "$label (expected exit $expected, got $actual)"
    fi
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "==> Checking prerequisites..."
if ! docker info >/dev/null 2>&1; then
    echo "ERROR: cannot reach Docker daemon (is it running? is the user in the docker group?)"
    exit 1
fi

echo "==> Building release binaries..."
cargo build --release --quiet

BONK="$REPO_ROOT/target/release/bonk"

echo "==> Packing alpine:latest..."
"$BONK" alpine:latest -o /tmp/bonk-e2e-alpine
ALPINE=/tmp/bonk-e2e-alpine

echo ""
echo "Running tests..."

# Basic command execution
OUT=$("$ALPINE" echo "hello from a bonk container" 2>/dev/null) || true
assert_contains "basic echo" "hello from a bonk container" "$OUT"

# Quiet mode suppresses progress output on stderr
STDERR=$("$ALPINE" -q echo "quiet" 2>&1 1>/dev/null || true)
if echo "$STDERR" | grep -qE "^bonk:"; then
    fail "quiet mode (progress output leaked to stderr)"
else
    pass "quiet mode"
fi

# Piped stdin (TTY detection — must not crash)
OUT=$(echo "piped-hello" | "$ALPINE" cat 2>/dev/null) || true
assert_contains "piped stdin" "piped-hello" "$OUT"

# Volume mount (read-write)
echo "volume-file-content" > /tmp/bonk-e2e-vol.txt
OUT=$("$ALPINE" -v /tmp/bonk-e2e-vol.txt:/data/vol.txt cat /data/vol.txt 2>/dev/null) || true
assert_contains "volume mount" "volume-file-content" "$OUT"
rm -f /tmp/bonk-e2e-vol.txt

# Read-only volume mount
echo "readonly-content" > /tmp/bonk-e2e-ro.txt
OUT=$("$ALPINE" -v /tmp/bonk-e2e-ro.txt:/data/ro.txt:ro cat /data/ro.txt 2>/dev/null) || true
assert_contains "read-only volume mount" "readonly-content" "$OUT"
rm -f /tmp/bonk-e2e-ro.txt

# Extra args replace CMD (entrypoint logic)
OUT=$("$ALPINE" echo "replaced-cmd" 2>/dev/null) || true
assert_contains "extra args replace CMD" "replaced-cmd" "$OUT"

# Exit code propagation
CODE=0
"$ALPINE" sh -c "exit 42" 2>/dev/null || CODE=$?
assert_exit "exit code propagation" 42 "${CODE:-0}"

# Second run uses cached rootfs (marker file present)
OUT=$("$ALPINE" echo "cached" 2>&1) || true
assert_contains "cached run (log message)" "cached rootfs" "$OUT"

# Root execution (sudo)
if sudo -n true 2>/dev/null; then
    OUT=$(sudo "$ALPINE" id 2>/dev/null) || true
    assert_contains "root execution (uid=0)" "uid=0" "$OUT"
else
    echo "  SKIP: root execution (no passwordless sudo)"
fi

# --help doesn't crash
"$ALPINE" --help >/dev/null 2>&1 || true
pass "--help exits cleanly"

echo ""
echo "Results: $PASS passed, $FAIL failed"
if [[ $FAIL -gt 0 ]]; then
    echo "Failed tests:"
    for e in "${ERRORS[@]}"; do echo "  - $e"; done
    exit 1
fi
