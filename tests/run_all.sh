#!/bin/sh
# Runs every test; no mouse, root or GNOME session needed.
set -e
cd "$(dirname "$0")/.."
for t in tests/test_*.py; do
    echo "== $t"
    python3 -I "$t"
done
echo "All tests passed."
