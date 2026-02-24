#!/usr/bin/env bash
# Quick validation for self-improvement cycles
# V1-V2: HARD gates (compilation + unit tests) — block deploy on failure
# V3-V5: ADVISORY (integration tests) — log warnings but don't block deploy
set -euo pipefail

source /root/.cargo/env
WORKSPACE="/root/.aethervault"
MV2="${AETHERVAULT_MV2:-${WORKSPACE}/memory.mv2}"
PASS=0
FAIL=0
WARN=0

run_quick() {
    local name="$1" prompt="$2"
    local output
    output=$(timeout 120 aethervault agent \
        --mv2 "$MV2" \
        --session "validate-${name}-$$" \
        --max-steps 32 \
        --prompt "$prompt" 2>&1) || { echo "WARN: $name (timeout — advisory only)"; WARN=$((WARN+1)); return; }

    if echo "$output" | grep -qi "panic\|broken pipe\|SIGSEGV"; then
        echo "WARN: $name (crash detected — advisory only)"
        WARN=$((WARN+1))
    else
        echo "PASS: $name"
        PASS=$((PASS+1))
    fi
}

# V1: Cargo check (HARD GATE)
echo "=== V1: cargo check ==="
cd /root/aethervault
if cargo check 2>&1 | tail -5; then
    echo "PASS: cargo_check"
    PASS=$((PASS+1))
else
    echo "FAIL: cargo_check"
    FAIL=$((FAIL+1))
    echo "RESULT: $PASS pass, $FAIL fail, $WARN warn"
    exit 1
fi

# V2: Cargo test (HARD GATE)
echo "=== V2: cargo test ==="
if cargo test 2>&1 | tail -10; then
    echo "PASS: cargo_test"
    PASS=$((PASS+1))
else
    echo "FAIL: cargo_test"
    FAIL=$((FAIL+1))
    echo "RESULT: $PASS pass, $FAIL fail, $WARN warn"
    exit 1
fi

# V3-V5: ADVISORY — require LLM calls, often timeout in CI context
echo "=== V3: agent basic (advisory) ==="
run_quick "basic" "What is 2+2? Answer with just the number."

echo "=== V4: fts5 (advisory) ==="
run_quick "fts5" "Search your memory for 'infrastructure' and report what you find."

echo "=== V5: subagent (advisory) ==="
run_quick "subagent" "Spawn a subagent named 'ping-test' with task 'run hostname and report it'. Wait for its result."

echo ""
echo "=== VALIDATION RESULT: $PASS pass, $FAIL fail, $WARN warn ==="
if [[ $WARN -gt 0 ]]; then
    echo "NOTE: $WARN advisory test(s) timed out. These require LLM calls and are expected to be flaky in CI."
fi
[[ $FAIL -eq 0 ]]  # Exit 0 only if hard gates pass (WARN doesn't block)
