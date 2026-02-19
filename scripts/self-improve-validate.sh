#!/usr/bin/env bash
# Quick validation for self-improvement cycles
# Runs S1 (parallel subagents), S4 (FTS5), S5 (grounded exec)
set -euo pipefail

source /root/.cargo/env
WORKSPACE="/root/.aethervault"
MV2="${WORKSPACE}/capsule.mv2"
PASS=0
FAIL=0

run_quick() {
    local name="$1" prompt="$2"
    local output
    output=$(timeout 120 aethervault agent \
        --mv2 "$MV2" \
        --session "validate-${name}-$$" \
        --max-steps 32 \
        --prompt "$prompt" 2>&1) || { echo "FAIL: $name (timeout)"; FAIL=$((FAIL+1)); return; }

    # Basic sanity: got output, no panic, no broken pipe
    if echo "$output" | grep -qi "panic\|broken pipe\|SIGSEGV"; then
        echo "FAIL: $name (crash detected)"
        FAIL=$((FAIL+1))
    else
        echo "PASS: $name"
        PASS=$((PASS+1))
    fi
}

# V1: Cargo check (most critical)
echo "=== V1: cargo check ==="
cd /root/aethervault
if cargo check 2>&1 | tail -5; then
    echo "PASS: cargo_check"
    PASS=$((PASS+1))
else
    echo "FAIL: cargo_check"
    FAIL=$((FAIL+1))
    echo "RESULT: $PASS pass, $FAIL fail"
    exit 1  # Hard fail — don't continue if it doesn't compile
fi

# V2: Cargo test
echo "=== V2: cargo test ==="
if cargo test 2>&1 | tail -10; then
    echo "PASS: cargo_test"
    PASS=$((PASS+1))
else
    echo "FAIL: cargo_test"
    FAIL=$((FAIL+1))
    echo "RESULT: $PASS pass, $FAIL fail"
    exit 1  # Hard fail
fi

# V3: Agent basic response
echo "=== V3: agent basic ==="
run_quick "basic" "What is 2+2? Answer with just the number."

# V4: FTS5 search
echo "=== V4: fts5 ==="
run_quick "fts5" "Search your memory for 'NOT working' and 'error OR failure'. Report what you find."

# V5: Subagent spawn
echo "=== V5: subagent ==="
run_quick "subagent" "Spawn a subagent named 'ping-test' with task 'run hostname and report it'. Wait for its result."

echo ""
echo "=== VALIDATION RESULT: $PASS pass, $FAIL fail ==="
[[ $FAIL -eq 0 ]]  # Exit 0 only if all pass
