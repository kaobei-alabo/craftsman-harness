#!/usr/bin/env bash
# Local production-readiness checks. Keep both native modes visible on failure.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

verification_python=""
for verification_candidate in python3.14 python3.13 python3.12 python3.11 python3 python; do
  if command -v "$verification_candidate" >/dev/null 2>&1 \
    && "$verification_candidate" -I -c 'import sys; raise SystemExit(sys.version_info < (3, 11))' >/dev/null 2>&1; then
    verification_python="$verification_candidate"
    break
  fi
done
if [[ -z "$verification_python" ]]; then
  printf 'Verification requires Python 3.11 or newer. Install it and rerun scripts/verify.sh.\n' >&2
  exit 1
fi

verification_failed=0
verification_source=$(git rev-parse --short HEAD) || exit 1
printf 'Checking source %s with %s\n' "$verification_source" "$verification_python"
if [[ -n "$(git status --porcelain)" ]]; then
  printf 'Working tree changes are included in this local verification.\n'
fi
run_check() {
  local verification_name="$1"
  shift
  if "$@"; then
    printf 'PASS: %s\n' "$verification_name"
  else
    local verification_exit=$?
    printf 'FAIL: %s (exit %s)\n' "$verification_name" "$verification_exit" >&2
    verification_failed=1
  fi
}

run_check formatting cargo fmt --all -- --check
run_check core-build cargo check --locked --no-default-features --workspace
run_check default-build env GGML_NATIVE=OFF cargo check --locked --workspace
run_check exec-contract "$verification_python" -I docs/schemas/check_exec.py --self-test
run_check benchmark-driver "$verification_python" -B scripts/test-benchmark-gadgets.py
run_check provenance ./scripts/check-lineage.sh
run_check core-tests cargo test --locked --no-default-features --workspace --no-fail-fast -- --test-threads=4
run_check gadget-tests cargo test --locked --no-default-features --features craftsman-rs/gadgets --workspace --no-fail-fast -- --test-threads=4
run_check goldens-unchanged git diff --exit-code -- crates/craftsman-rs/tests/golden
exit "$verification_failed"
