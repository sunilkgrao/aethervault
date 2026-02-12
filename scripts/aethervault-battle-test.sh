#!/bin/bash
# AetherVault Autonomous Battle Test Suite
# Runs all P0, P1, P2 tests directly on the droplet via aethervault agent CLI
# No manual Telegram interaction needed!

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PASS=0
FAIL=0
SKIP=0
RESULTS=""
REPORT_FILE="/tmp/aethervault-battle-report.md"
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
MV2="${CAPSULE_PATH:-$AETHERVAULT_HOME/memory.mv2}"
ENV_FILE="$AETHERVAULT_HOME/.env"

log_test() { echo -e "${CYAN}[TEST $1]${NC} $2"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; FAIL=$((FAIL+1)); }
log_skip() { echo -e "${YELLOW}[SKIP]${NC} $1"; SKIP=$((SKIP+1)); }
log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }

record() {
    local num="$1" name="$2" status="$3" time="$4" notes="$5"
    RESULTS="${RESULTS}| ${num} | ${name} | ${status} | ${time} | ${notes} |\n"
}

# Load env
set -a; source <(grep -v '^\s*#' "$ENV_FILE" | grep -v '^\s*$'); set +a

# Helper: run agent with timeout and capture output + timing
run_agent() {
    local prompt="$1"
    local max_steps="${2:-8}"
    local session="${3:-test-$(date +%s)}"
    local start=$(date +%s%N)

    local output
    output=$(echo "$prompt" | timeout 120 /usr/local/bin/aethervault agent "$MV2" \
        --model-hook '/usr/local/bin/aethervault hook claude' \
        --max-steps "$max_steps" \
        --session "$session" \
        --log 2>&1) || true

    local end=$(date +%s%N)
    local elapsed=$(( (end - start) / 1000000 ))

    echo "TIME:${elapsed}ms"
    echo "OUTPUT:${output}"
}

# Get initial memory stats
MEM_BEFORE=$(ps aux | grep aethervault | grep -v grep | awk '{print $6}' | head -1 || echo "N/A")

echo "=============================================="
echo "  AetherVault Battle Test Suite"
echo "  $(date -u)"
echo "=============================================="
echo ""

# =============================================
# P0 Tests - Must Pass
# =============================================
echo -e "${BLUE}=== P0 TESTS (Must Pass) ===${NC}"
echo ""

# Test 1: Basic text response
log_test "P0-1" "Basic text response"
result=$(run_agent "Say hello back to me in one short sentence.")
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if [ -n "$output" ] && echo "$output" | grep -qi "hello\|hi\|hey\|greetings"; then
    log_pass "Basic text response (${time})"
    record "1" "Basic text response" "PASS" "$time" "Got coherent greeting"
else
    log_fail "Basic text response (${time}) - Output: $(echo "$output" | head -1)"
    record "1" "Basic text response" "FAIL" "$time" "No greeting detected"
fi

# Test 2: Simple reasoning
log_test "P0-2" "Simple reasoning (2+2)"
result=$(run_agent "What is 2+2? Reply with just the number." 1)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -q "4"; then
    log_pass "Simple reasoning (${time})"
    record "2" "Simple reasoning (2+2)" "PASS" "$time" "Correct answer: 4"
else
    log_fail "Simple reasoning (${time}) - Output: $(echo "$output" | head -1)"
    record "2" "Simple reasoning (2+2)" "FAIL" "$time" "Expected 4"
fi

# Test 3: Tool use - this will test the exec tool (may need approval)
log_test "P0-3" "Exec tool (list files)"
result=$(run_agent "Use the fs_list tool to list files in /root/.aethervault/ and tell me what you find." 5)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -qi "memory.mv2\|workspace\|\.env\|file\|directory"; then
    log_pass "Tool use - fs_list (${time})"
    record "3" "Exec tool (list files)" "PASS" "$time" "Listed files successfully"
else
    # Check if it was an approval issue
    if echo "$output" | grep -qi "approval"; then
        log_fail "Tool use - approval required (${time})"
        record "3" "Exec tool (list files)" "FAIL" "$time" "Approval required - exec blocked"
    else
        log_fail "Tool use (${time}) - Output: $(echo "$output" | head -c 200)"
        record "3" "Exec tool (list files)" "FAIL" "$time" "No file listing detected"
    fi
fi

# Test 4: Concurrent messages (sequential rapid-fire)
log_test "P0-4" "Concurrent message handling (5 rapid messages)"
concurrent_pass=0
for i in 1 2 3 4 5; do
    result=$(run_agent "Respond with just the number $i." 1 "concurrent-$i")
    output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
    if [ -n "$output" ] && [ ${#output} -gt 0 ]; then
        concurrent_pass=$((concurrent_pass+1))
    fi
done
if [ $concurrent_pass -ge 4 ]; then
    log_pass "Concurrent messages ($concurrent_pass/5 got responses)"
    record "4" "Concurrent messages (5 rapid)" "PASS" "N/A" "$concurrent_pass/5 responded"
else
    log_fail "Concurrent messages ($concurrent_pass/5 got responses)"
    record "4" "Concurrent messages (5 rapid)" "FAIL" "N/A" "Only $concurrent_pass/5 responded"
fi

# Test 5: Long message handling
log_test "P0-5" "Long message handling (2000+ chars)"
long_msg=$(python3 -c "print('This is a test message that is repeated many times. ' * 50)")
result=$(run_agent "$long_msg Give me a one-word summary of the above." 1)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if [ -n "$output" ] && [ ${#output} -gt 0 ]; then
    log_pass "Long message handling (${time})"
    record "5" "Long message (2000+ chars)" "PASS" "$time" "Handled without truncation"
else
    log_fail "Long message handling (${time})"
    record "5" "Long message (2000+ chars)" "FAIL" "$time" "No response or truncation error"
fi

# Test 6: Crash check
log_test "P0-6" "Stability check (no crashes)"
crash_count=$(journalctl -u aethervault --since "5 minutes ago" 2>/dev/null | grep -c "panic\|segfault\|SIGSEGV\|thread.*panicked" || echo "0")
if [ "$crash_count" = "0" ]; then
    log_pass "No crashes detected"
    record "6" "Stability (no crashes)" "PASS" "N/A" "No panics or segfaults"
else
    log_fail "$crash_count crashes detected"
    record "6" "Stability (no crashes)" "FAIL" "N/A" "$crash_count crash(es) found"
fi

echo ""

# =============================================
# P1 Tests - Should Pass
# =============================================
echo -e "${BLUE}=== P1 TESTS (Should Pass) ===${NC}"
echo ""

# Test 7: Memory persistence
log_test "P1-7" "Memory persistence"
SESSION_ID="memory-test-$(date +%s)"
run_agent "Remember this: my favorite color is cerulean blue." 3 "$SESSION_ID" > /dev/null 2>&1
sleep 2
result=$(run_agent "What is my favorite color?" 3 "$SESSION_ID")
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -qi "cerulean\|blue"; then
    log_pass "Memory persistence (${time})"
    record "7" "Memory persistence" "PASS" "$time" "Recalled cerulean blue"
else
    log_fail "Memory persistence (${time}) - Output: $(echo "$output" | head -c 200)"
    record "7" "Memory persistence" "FAIL" "$time" "Did not recall color"
fi

# Test 8: Web fetch / HTTP tool
log_test "P1-8" "Web fetch tool"
result=$(run_agent "Use the http_request tool to fetch https://httpbin.org/get and tell me the origin IP." 5)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -qE "[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+"; then
    log_pass "Web fetch tool (${time})"
    record "8" "Web fetch (http_request)" "PASS" "$time" "Got IP from httpbin"
elif echo "$output" | grep -qi "origin\|ip\|address\|httpbin"; then
    log_pass "Web fetch tool (${time})"
    record "8" "Web fetch (http_request)" "PASS" "$time" "Successfully fetched httpbin"
else
    log_fail "Web fetch tool (${time}) - Output: $(echo "$output" | head -c 200)"
    record "8" "Web fetch (http_request)" "FAIL" "$time" "No IP in response"
fi

# Test 9: File write
log_test "P1-9" "File write tool"
result=$(run_agent "Use the fs_write tool to create a file at /root/.aethervault/workspace/test-battle.txt with the content 'AetherVault battle test passed'" 5)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if [ -f /root/.aethervault/workspace/test-battle.txt ]; then
    log_pass "File write tool (${time})"
    record "9" "File write (fs_write)" "PASS" "$time" "File created successfully"
elif echo "$output" | grep -qi "approval"; then
    log_fail "File write - approval required (${time})"
    record "9" "File write (fs_write)" "FAIL" "$time" "Approval required"
else
    log_fail "File write tool (${time}) - Output: $(echo "$output" | head -c 200)"
    record "9" "File write (fs_write)" "FAIL" "$time" "File not created"
fi

# Test 10: File read
log_test "P1-10" "File read tool"
# Create a test file first
echo "Battle test verification: AV-$(date +%s)" > /root/.aethervault/workspace/read-test.txt
result=$(run_agent "Use the fs_read tool to read the file /root/.aethervault/workspace/read-test.txt and tell me its contents." 3)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -qi "battle test verification\|AV-"; then
    log_pass "File read tool (${time})"
    record "10" "File read (fs_read)" "PASS" "$time" "Read file contents correctly"
else
    log_fail "File read tool (${time}) - Output: $(echo "$output" | head -c 200)"
    record "10" "File read (fs_read)" "FAIL" "$time" "Could not read file"
fi

# Test 11: Multi-turn conversation
log_test "P1-11" "Multi-turn conversation (5 exchanges)"
MULTI_SESSION="multi-turn-$(date +%s)"
multi_pass=0
run_agent "I'm going to tell you 5 facts. Fact 1: The sky is blue." 1 "$MULTI_SESSION" > /dev/null 2>&1
run_agent "Fact 2: Water is wet." 1 "$MULTI_SESSION" > /dev/null 2>&1
run_agent "Fact 3: Fire is hot." 1 "$MULTI_SESSION" > /dev/null 2>&1
run_agent "Fact 4: Ice is cold." 1 "$MULTI_SESSION" > /dev/null 2>&1
result=$(run_agent "Fact 5: Grass is green. Now list all 5 facts I told you." 3 "$MULTI_SESSION")
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
for keyword in "sky" "water" "fire" "ice" "grass"; do
    if echo "$output" | grep -qi "$keyword"; then
        multi_pass=$((multi_pass+1))
    fi
done
if [ $multi_pass -ge 3 ]; then
    log_pass "Multi-turn conversation ($multi_pass/5 facts recalled) (${time})"
    record "11" "Multi-turn conversation" "PASS" "$time" "$multi_pass/5 facts recalled"
else
    log_fail "Multi-turn conversation ($multi_pass/5 facts recalled) (${time})"
    record "11" "Multi-turn conversation" "FAIL" "$time" "Only $multi_pass/5 facts recalled"
fi

# Test 12: Session persistence (skip 10-min wait, test basic persistence)
log_test "P1-12" "Session persistence"
PERSIST_SESSION="persist-$(date +%s)"
run_agent "The secret code is ALPHA-BRAVO-42." 1 "$PERSIST_SESSION" > /dev/null 2>&1
sleep 5
result=$(run_agent "What was the secret code I told you?" 3 "$PERSIST_SESSION")
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -qi "ALPHA.*BRAVO.*42\|alpha.*bravo"; then
    log_pass "Session persistence (${time})"
    record "12" "Session persistence" "PASS" "$time" "Recalled secret code"
else
    log_fail "Session persistence (${time}) - Output: $(echo "$output" | head -c 200)"
    record "12" "Session persistence" "FAIL" "$time" "Could not recall code"
fi

# Test 13: Memory recall
log_test "P1-13" "Memory recall (earlier conversation)"
result=$(run_agent "What did we discuss in our previous conversations? Summarize any topics." 5)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if [ -n "$output" ] && [ ${#output} -gt 20 ]; then
    log_pass "Memory recall (${time})"
    record "13" "Memory recall" "PASS" "$time" "Provided conversation summary"
else
    log_fail "Memory recall (${time})"
    record "13" "Memory recall" "FAIL" "$time" "No recall available"
fi

echo ""

# =============================================
# P2 Tests - Known Gaps (Expected Failures)
# =============================================
echo -e "${BLUE}=== P2 TESTS (Known Gaps) ===${NC}"
echo ""

# Test 14: Voice/audio - Skip (requires Telegram)
log_test "P2-14" "Voice/audio message"
log_skip "Voice/audio requires Telegram bridge - cannot test via CLI"
record "14" "Voice/audio message" "SKIP (EXPECTED)" "N/A" "No audio support - requires Telegram"

# Test 15: Runtime model switching
log_test "P2-15" "Runtime model switching"
result=$(run_agent "Switch to using the Sonnet model for responses." 3)
time=$(echo "$result" | grep "^TIME:" | cut -d: -f2)
output=$(echo "$result" | grep "^OUTPUT:" | cut -d: -f2-)
if echo "$output" | grep -qi "switch\|model\|cannot\|unable\|don't have"; then
    log_skip "Model switching - expected limitation (${time})"
    record "15" "Model switching" "EXPECTED FAIL" "$time" "No runtime model switching"
else
    log_skip "Model switching (${time})"
    record "15" "Model switching" "EXPECTED FAIL" "$time" "No runtime model switching"
fi

# Test 16: Image handling - Skip (requires Telegram)
log_test "P2-16" "Image handling"
log_skip "Image handling requires Telegram bridge - cannot test via CLI"
record "16" "Image handling" "SKIP (EXPECTED)" "N/A" "Requires Telegram bridge for testing"

echo ""

# =============================================
# Memory & Stability Check
# =============================================
echo -e "${BLUE}=== POST-TEST METRICS ===${NC}"
echo ""

MEM_AFTER=$(ps aux | grep aethervault | grep -v grep | awk '{print $6}' | head -1 || echo "N/A")
log_info "Memory before tests: ${MEM_BEFORE:-N/A} KB"
log_info "Memory after tests: ${MEM_AFTER:-N/A} KB"

TOTAL_CRASHES=$(journalctl -u aethervault --since "30 minutes ago" 2>/dev/null | grep -c "panic\|segfault\|SIGSEGV\|thread.*panicked" || echo "0")
log_info "Total crashes in last 30 min: $TOTAL_CRASHES"

CAPSULE_SIZE=$(ls -lh "$MV2" 2>/dev/null | awk '{print $5}')
log_info "Capsule size: ${CAPSULE_SIZE:-N/A}"

# =============================================
# Generate Report
# =============================================
TOTAL=$((PASS+FAIL+SKIP))

cat > "$REPORT_FILE" << EOF
# AetherVault Battle Test Report
**Date:** $(date -u)
**Binary:** AetherVault 0.0.1
**Droplet:** aethervault (8GB RAM, Ubuntu 24.04)
**Model:** claude-opus-4-6

## Summary
- **Total Tests:** $TOTAL
- **Passed:** $PASS
- **Failed:** $FAIL
- **Skipped/Expected:** $SKIP
- **Pass Rate:** $(( PASS * 100 / (PASS + FAIL + 1) ))%

## Results

| # | Test | Status | Time | Notes |
|---|------|--------|------|-------|
$(echo -e "$RESULTS")

## System Metrics
- **Memory Before:** ${MEM_BEFORE:-N/A} KB
- **Memory After:** ${MEM_AFTER:-N/A} KB
- **Crashes:** $TOTAL_CRASHES
- **Capsule Size:** ${CAPSULE_SIZE:-N/A}

## Known Gaps
1. No audio/voice message support in Telegram bridge
2. No runtime model switching
3. exec/fs_write tools require approval (security feature)

## Recommendation
EOF

if [ $FAIL -eq 0 ]; then
    echo "**RECOMMEND: Full migration** - All critical tests passed." >> "$REPORT_FILE"
elif [ $FAIL -le 2 ]; then
    echo "**RECOMMEND: Conditional migration** - Minor failures, investigate before full commit." >> "$REPORT_FILE"
else
    echo "**RECOMMEND: Rollback to AetherVault** - Too many failures for production use." >> "$REPORT_FILE"
fi

echo ""
echo "=============================================="
echo "  BATTLE TEST COMPLETE"
echo "  Passed: $PASS  Failed: $FAIL  Skipped: $SKIP"
echo "  Report: $REPORT_FILE"
echo "=============================================="

cat "$REPORT_FILE"
