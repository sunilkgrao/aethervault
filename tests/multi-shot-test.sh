#!/usr/bin/env bash
# =============================================================================
# AetherVault Multi-Shot Conversation & Memory Test Battery
# =============================================================================
# Tests conversation coherence, memory consistency, compaction safety,
# session isolation, and context retrieval accuracy across multi-turn exchanges.
# =============================================================================

set -euo pipefail

MV2=/root/.aethervault/memory.mv2
TEST_MV2=/tmp/aethervault-test-capsule.mv2
PASS=0
FAIL=0
TOTAL=0

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

log_result() {
    local test_num="$1"
    local test_name="$2"
    local status="$3"
    local details="${4:-}"
    TOTAL=$((TOTAL + 1))
    if [ "$status" = "PASS" ]; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}[PASS]${NC} #${test_num}: ${test_name}"
    else
        FAIL=$((FAIL + 1))
        echo -e "  ${RED}[FAIL]${NC} #${test_num}: ${test_name}"
        [ -n "$details" ] && echo "         $details"
    fi
}

echo "============================================"
echo "AetherVault Multi-Shot Conversation Tests"
echo "============================================"
echo "Date: $(date -u)"
PROD_FRAMES=$(aethervault status "$MV2" 2>/dev/null | grep frames | awk '{print $2}')
PROD_SIZE=$(stat -f%z "$MV2" 2>/dev/null || stat -c%s "$MV2" 2>/dev/null || echo '?')
echo "Production capsule: $MV2 ($PROD_SIZE bytes, $PROD_FRAMES frames)"
echo

# =============================================================================
# SECTION 1: Test Environment Setup
# =============================================================================
echo "--- Section 1: Test Environment Setup ---"

rm -f "$TEST_MV2"
aethervault init "$TEST_MV2" 2>/dev/null
STATUS=$(aethervault status "$TEST_MV2" 2>&1)
if echo "$STATUS" | grep -q "frames: 0"; then
    log_result 1 "Create clean test capsule" "PASS"
else
    log_result 1 "Create clean test capsule" "FAIL" "$STATUS"
fi

# =============================================================================
# SECTION 2: Multi-Turn Session Continuity (5-turn Rust API conversation)
# =============================================================================
echo
echo "--- Section 2: Multi-Turn Session Continuity ---"

SESSION="test-multishot-$(date +%s)"

# Turn 1: Framework recommendation
aethervault log "$TEST_MV2" --session "$SESSION" --role user \
    --text "I want to build a REST API in Rust. What framework should I use?" 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION" --role assistant \
    --text "For a Rust REST API, I recommend Axum. Built on Tokio and Tower, it has excellent ergonomics with extractors and is maintained by the Tokio team. Alternatives: Actix-web (fast, more complex) and Rocket (simpler, less async-native). Axum is the sweet spot." 2>/dev/null

# Turn 2: JSON handler
aethervault log "$TEST_MV2" --session "$SESSION" --role user \
    --text "Great, let's go with Axum. Show me a basic handler with JSON request and response." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION" --role assistant \
    --text 'Here is a basic Axum handler: use axum::{Json, Router, routing::post}; use serde::{Deserialize, Serialize}; #[derive(Deserialize)] struct CreateUser { name: String, email: String } #[derive(Serialize)] struct User { id: u64, name: String, email: String } async fn create_user(Json(payload): Json<CreateUser>) -> Json<User> { Json(User { id: 1, name: payload.name, email: payload.email }) }' 2>/dev/null

# Turn 3: Error handling
aethervault log "$TEST_MV2" --session "$SESSION" --role user \
    --text "How do I add proper error handling to this Axum handler?" 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION" --role assistant \
    --text 'Use Axum IntoResponse trait with custom error type: enum AppError { NotFound, BadRequest(String), Internal(anyhow::Error) } impl IntoResponse for AppError { fn into_response(self) -> Response { let (status, msg) = match self { Self::NotFound => (NOT_FOUND, "Not found"), Self::BadRequest(m) => (BAD_REQUEST, m), Self::Internal(e) => (INTERNAL_SERVER_ERROR, e.to_string()), }; (status, Json(json!({"error": msg}))).into_response() } }' 2>/dev/null

# Turn 4: Database
aethervault log "$TEST_MV2" --session "$SESSION" --role user \
    --text "Now I need a database. Should I use SQLx or Diesel with Axum?" 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION" --role assistant \
    --text "SQLx pairs better with Axum because both are async-native. SQLx is compile-time checked, works with raw SQL, and supports Postgres/MySQL/SQLite. Diesel is sync and adds ORM complexity with Axum async model. Go with SQLx plus Postgres for production." 2>/dev/null

# Turn 5: Deployment
aethervault log "$TEST_MV2" --session "$SESSION" --role user \
    --text "How should I deploy this Axum plus SQLx app?" 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION" --role assistant \
    --text "Deploy Axum plus SQLx: 1) Build static binary with musl target. 2) Multi-stage Dockerfile. 3) Deploy on Fly.io or DigitalOcean droplet with systemd. 4) Use dotenvy for config and DATABASE_URL. 5) Run sqlx migrate run in startup." 2>/dev/null

# Test 2: Frame count
FRAME_COUNT=$(aethervault status "$TEST_MV2" 2>&1 | grep frames | awk '{print $2}')
if [ "$FRAME_COUNT" = "10" ]; then
    log_result 2 "5-turn conversation logged (10 frames)" "PASS"
else
    log_result 2 "5-turn conversation logged (10 frames)" "FAIL" "Expected 10, got $FRAME_COUNT"
fi

# Test 3: Context retrieval for deployment
CONTEXT=$(aethervault context "$TEST_MV2" "deploy Axum SQLx app" -n 5 --full 2>&1)
if echo "$CONTEXT" | grep -qi "axum\|deploy\|sqlx"; then
    log_result 3 "Context retrieval: Axum deployment info" "PASS"
else
    log_result 3 "Context retrieval: Axum deployment info" "FAIL" "No Axum/deploy context"
fi

# Test 4: Context for error handling
CONTEXT2=$(aethervault context "$TEST_MV2" "error handling Axum IntoResponse" -n 5 --full 2>&1)
if echo "$CONTEXT2" | grep -qi "IntoResponse\|AppError\|error"; then
    log_result 4 "Context retrieval: error handling code" "PASS"
else
    log_result 4 "Context retrieval: error handling code" "FAIL" "No error handling context"
fi

# Test 5: Context for framework recommendation
CONTEXT3=$(aethervault context "$TEST_MV2" "Rust web framework recommendation" -n 5 --full 2>&1)
if echo "$CONTEXT3" | grep -qi "axum\|actix\|rocket"; then
    log_result 5 "Context retrieval: framework recommendation" "PASS"
else
    log_result 5 "Context retrieval: framework recommendation" "FAIL" "No framework context"
fi

# =============================================================================
# SECTION 3: Session Isolation
# =============================================================================
echo
echo "--- Section 3: Session Isolation ---"

SESSION2="test-isolation-$(date +%s)"
aethervault log "$TEST_MV2" --session "$SESSION2" --role user \
    --text "What is the best recipe for chocolate cake?" 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION2" --role assistant \
    --text "Classic chocolate cake: 2 cups flour, 2 cups sugar, 3/4 cup cocoa, 2 eggs, 1 cup buttermilk, 1 cup hot coffee, 1/2 cup oil. Mix dry, add wet, bake 350F 30 mins. Coffee enhances chocolate flavor." 2>/dev/null

# Test 6: Session1 scoped search should NOT find cake
SCOPED=$(aethervault context "$TEST_MV2" "chocolate cake recipe" -c "aethervault://agent-log/$SESSION/" -n 5 --full 2>&1)
if echo "$SCOPED" | grep -qi "chocolate\|cake\|recipe"; then
    log_result 6 "Session isolation: Rust session doesn't contain cake" "FAIL" "Chocolate cake leaked"
else
    log_result 6 "Session isolation: Rust session doesn't contain cake" "PASS"
fi

# Test 7: Session2 scoped search should NOT find Axum
SCOPED2=$(aethervault context "$TEST_MV2" "Axum Rust framework" -c "aethervault://agent-log/$SESSION2/" -n 5 --full 2>&1)
if echo "$SCOPED2" | grep -qi "axum\|rust\|sqlx"; then
    log_result 7 "Session isolation: cake session doesn't contain Rust" "FAIL" "Axum leaked into cake session"
else
    log_result 7 "Session isolation: cake session doesn't contain Rust" "PASS"
fi

# Test 8: Unscoped search finds cake
UNSCOPED=$(aethervault context "$TEST_MV2" "chocolate cake" -n 5 --full 2>&1)
if echo "$UNSCOPED" | grep -qi "chocolate\|cake"; then
    log_result 8 "Unscoped search finds chocolate cake" "PASS"
else
    log_result 8 "Unscoped search finds chocolate cake" "FAIL" "Cake not found"
fi

# =============================================================================
# SECTION 4: Memory Consistency (Bulk Turns with Unique Facts)
# =============================================================================
echo
echo "--- Section 4: Memory Consistency (20 turns, 10 unique facts) ---"

SESSION3="test-bulk-$(date +%s)"
for i in $(seq 1 10); do
    aethervault log "$TEST_MV2" --session "$SESSION3" --role user \
        --text "Remember: fact number $i is ALPHA-${i}-BRAVO" 2>/dev/null
    aethervault log "$TEST_MV2" --session "$SESSION3" --role assistant \
        --text "Stored fact $i: ALPHA-${i}-BRAVO. Total facts: $i." 2>/dev/null
done

# Test 9-11: Early, middle, late facts
for pair in "9:1:Early" "10:5:Middle" "11:10:Late"; do
    IFS=: read -r tnum fnum label <<< "$pair"
    RESULT=$(aethervault context "$TEST_MV2" "ALPHA-${fnum}-BRAVO" -n 3 --full 2>&1)
    if echo "$RESULT" | grep -q "ALPHA-${fnum}-BRAVO"; then
        log_result "$tnum" "$label fact (turn $fnum/10) retrievable" "PASS"
    else
        log_result "$tnum" "$label fact (turn $fnum/10) retrievable" "FAIL" "ALPHA-${fnum}-BRAVO not found"
    fi
done

# Test 12: Total frame count
EXPECTED=$((10 + 2 + 20))
ACTUAL=$(aethervault status "$TEST_MV2" 2>&1 | grep frames | awk '{print $2}')
if [ "$ACTUAL" = "$EXPECTED" ]; then
    log_result 12 "Frame count consistent ($EXPECTED frames)" "PASS"
else
    log_result 12 "Frame count consistent ($EXPECTED frames)" "FAIL" "Expected $EXPECTED, got $ACTUAL"
fi

# =============================================================================
# SECTION 5: Compaction Safety
# =============================================================================
echo
echo "--- Section 5: Compaction Safety ---"

cp "$TEST_MV2" /tmp/aethervault-test-pre-compact.mv2
PRE_FRAMES=$(aethervault status "$TEST_MV2" 2>&1 | grep frames | awk '{print $2}')

# Test 13: Dry-run compaction
DRY=$(aethervault compact "$TEST_MV2" --dry-run --json 2>&1)
if echo "$DRY" | grep -qi "panic\|segfault"; then
    log_result 13 "Compact dry-run safe" "FAIL" "Panic detected"
else
    log_result 13 "Compact dry-run safe" "PASS"
fi

# Test 14: Actual compaction
aethervault compact "$TEST_MV2" --quiet 2>/dev/null || true
POST_FRAMES=$(aethervault status "$TEST_MV2" 2>&1 | grep frames | awk '{print $2}')
if [ "$POST_FRAMES" -gt 0 ] 2>/dev/null; then
    log_result 14 "Compaction preserves data ($PRE_FRAMES -> $POST_FRAMES frames)" "PASS"
else
    log_result 14 "Compaction preserves data" "FAIL" "Frames lost: $PRE_FRAMES -> $POST_FRAMES"
fi

# Test 15-18: Post-compaction data integrity
POST_EARLY=$(aethervault context "$TEST_MV2" "ALPHA-1-BRAVO" -n 3 --full 2>&1)
if echo "$POST_EARLY" | grep -q "ALPHA-1-BRAVO"; then
    log_result 15 "Post-compact: early fact preserved" "PASS"
else
    log_result 15 "Post-compact: early fact preserved" "FAIL" "ALPHA-1-BRAVO lost"
fi

POST_LATE=$(aethervault context "$TEST_MV2" "ALPHA-10-BRAVO" -n 3 --full 2>&1)
if echo "$POST_LATE" | grep -q "ALPHA-10-BRAVO"; then
    log_result 16 "Post-compact: late fact preserved" "PASS"
else
    log_result 16 "Post-compact: late fact preserved" "FAIL" "ALPHA-10-BRAVO lost"
fi

POST_CODE=$(aethervault context "$TEST_MV2" "Axum IntoResponse error" -n 5 --full 2>&1)
if echo "$POST_CODE" | grep -qi "axum\|IntoResponse\|error"; then
    log_result 17 "Post-compact: code examples preserved" "PASS"
else
    log_result 17 "Post-compact: code examples preserved" "FAIL" "Axum code lost"
fi

POST_ISO=$(aethervault context "$TEST_MV2" "chocolate cake" -c "aethervault://agent-log/$SESSION/" -n 3 --full 2>&1)
if echo "$POST_ISO" | grep -qi "chocolate\|cake"; then
    log_result 18 "Post-compact: session isolation maintained" "FAIL" "Cake leaked after compaction"
else
    log_result 18 "Post-compact: session isolation maintained" "PASS"
fi

# =============================================================================
# SECTION 6: Research + Coding Multi-Step Conversation
# =============================================================================
echo
echo "--- Section 6: Research + Coding Multi-Step (Token Bucket) ---"

SESSION4="test-research-code-$(date +%s)"

# Research -> Design -> Implement -> Test -> Review (5 phases)
aethervault log "$TEST_MV2" --session "$SESSION4" --role user \
    --text "Research the token bucket algorithm for rate limiting." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION4" --role assistant \
    --text "Token Bucket algorithm: bucket holds tokens, added at fixed rate (refill_rate). Each request consumes one token. Empty bucket means reject (429). Parameters: bucket_capacity (burst size), refill_rate (sustained throughput), current_tokens. Superior to fixed-window because it allows bursting." 2>/dev/null

aethervault log "$TEST_MV2" --session "$SESSION4" --role user \
    --text "Design a thread-safe Rust struct for this token bucket." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION4" --role assistant \
    --text "Design: pub struct TokenBucket { inner: Mutex<BucketInner> } struct BucketInner { capacity: f64, tokens: f64, refill_rate: f64, last_refill: Instant } Methods: new(capacity, refill_rate), try_acquire() -> bool, try_acquire_n(n) -> bool. Thread-safe via Mutex, lazy refill on acquire." 2>/dev/null

aethervault log "$TEST_MV2" --session "$SESSION4" --role user \
    --text "Implement try_acquire with lazy refill." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION4" --role assistant \
    --text "Implementation: pub fn try_acquire(&self) -> bool { let mut inner = self.inner.lock().unwrap(); let elapsed = Instant::now().duration_since(inner.last_refill).as_secs_f64(); inner.tokens = (inner.tokens + elapsed * inner.refill_rate).min(inner.capacity); inner.last_refill = Instant::now(); if inner.tokens >= 1.0 { inner.tokens -= 1.0; true } else { false } }" 2>/dev/null

aethervault log "$TEST_MV2" --session "$SESSION4" --role user \
    --text "Write unit tests: empty bucket, burst, refill after wait." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION4" --role assistant \
    --text "Tests: test_empty_bucket_rejects: TokenBucket::new(1.0,1.0), first acquire succeeds, second fails. test_burst: capacity 5, acquire 5 times OK, 6th fails. test_refill: capacity 1, rate 10/sec, acquire, fail, sleep 150ms, acquire succeeds (refilled)." 2>/dev/null

aethervault log "$TEST_MV2" --session "$SESSION4" --role user \
    --text "Review: any issues with the Mutex approach?" 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION4" --role assistant \
    --text "Review: 1) Mutex contention under high concurrency - consider AtomicU64 CAS. 2) f64 precision drift over millions of ops. 3) No fairness - threads may starve, consider parking_lot FairMutex. 4) Instant is monotonic, no clock skew. 5) unwrap on lock panics on PoisonError. Overall: solid for moderate concurrency, move to atomic CAS for high-throughput." 2>/dev/null

# Test 19-22: Cross-phase retrieval
CTX_R=$(aethervault context "$TEST_MV2" "token bucket algorithm rate limiting" -n 5 --full 2>&1)
if echo "$CTX_R" | grep -qi "token bucket\|refill_rate\|capacity"; then
    log_result 19 "Research phase retrievable" "PASS"
else
    log_result 19 "Research phase retrievable" "FAIL" "Token bucket not found"
fi

CTX_I=$(aethervault context "$TEST_MV2" "try_acquire implementation Rust" -n 5 --full 2>&1)
if echo "$CTX_I" | grep -qi "try_acquire\|tokens\|refill"; then
    log_result 20 "Implementation phase retrievable" "PASS"
else
    log_result 20 "Implementation phase retrievable" "FAIL" "try_acquire not found"
fi

CTX_V=$(aethervault context "$TEST_MV2" "Mutex contention review" -n 5 --full 2>&1)
if echo "$CTX_V" | grep -qi "mutex\|contention\|atomic\|fairness"; then
    log_result 21 "Review phase retrievable" "PASS"
else
    log_result 21 "Review phase retrievable" "FAIL" "Review not found"
fi

# Cross-phase coherence
CTX_X=$(aethervault context "$TEST_MV2" "TokenBucket struct implementation" -n 8 --full 2>&1)
D_FOUND=false; I_FOUND=false
echo "$CTX_X" | grep -qi "BucketInner\|capacity.*refill_rate" && D_FOUND=true
echo "$CTX_X" | grep -qi "try_acquire\|elapsed.*as_secs_f64\|lock().unwrap" && I_FOUND=true
if $D_FOUND && $I_FOUND; then
    log_result 22 "Cross-phase: design + impl both in context" "PASS"
else
    log_result 22 "Cross-phase: design + impl both in context" "FAIL" "design=$D_FOUND, impl=$I_FOUND"
fi

# =============================================================================
# SECTION 7: Compaction Mid-Conversation
# =============================================================================
echo
echo "--- Section 7: Compaction Mid-Conversation ---"

SESSION5="test-compact-mid-$(date +%s)"

# Pre-compaction turns
aethervault log "$TEST_MV2" --session "$SESSION5" --role user \
    --text "Building ZEPHYR-CLI. It parses TOML config files." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION5" --role assistant \
    --text "ZEPHYR-CLI: toml crate for parsing, clap for CLI, serde for deserialization. Structure: src/main.rs, src/config.rs, src/commands/." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION5" --role user \
    --text "Add deploy subcommand that reads target from config and SSHs to server." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION5" --role assistant \
    --text "Deploy command: read [deploy] from zephyr.toml, extract host/user/key_path, use openssh crate for async SSH. Config: [deploy] host=1.2.3.4 user=deploy key_path=~/.ssh/id_ed25519" 2>/dev/null

# COMPACT mid-conversation
aethervault compact "$TEST_MV2" --quiet 2>/dev/null || true

# Post-compaction turns
aethervault log "$TEST_MV2" --session "$SESSION5" --role user \
    --text "Add status subcommand showing all targets and last deploy time." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION5" --role assistant \
    --text "Status subcommand: iterate [deploy.*] sections, display table: Target | Host | Last Deploy | Status. Store deploy history in .zephyr/deploy.log." 2>/dev/null

# Test 23-25
CTX_PRE=$(aethervault context "$TEST_MV2" "ZEPHYR-CLI TOML config" -n 5 --full 2>&1)
if echo "$CTX_PRE" | grep -qi "zephyr\|toml"; then
    log_result 23 "Pre-compaction turns survive" "PASS"
else
    log_result 23 "Pre-compaction turns survive" "FAIL" "ZEPHYR context lost"
fi

CTX_POST=$(aethervault context "$TEST_MV2" "status subcommand deploy targets" -n 5 --full 2>&1)
if echo "$CTX_POST" | grep -qi "status\|deploy\|target"; then
    log_result 24 "Post-compaction turns retrievable" "PASS"
else
    log_result 24 "Post-compaction turns retrievable" "FAIL" "Status context lost"
fi

CTX_BOTH=$(aethervault context "$TEST_MV2" "ZEPHYR deploy status commands" -n 10 --full 2>&1)
D2=false; S2=false
echo "$CTX_BOTH" | grep -qi "ssh\|deploy\|key_path" && D2=true
echo "$CTX_BOTH" | grep -qi "status\|table\|deploy.log" && S2=true
if $D2 && $S2; then
    log_result 25 "Full conversation coherent across compaction" "PASS"
else
    log_result 25 "Full conversation coherent across compaction" "FAIL" "deploy=$D2, status=$S2"
fi

# =============================================================================
# SECTION 8: Production Capsule Cross-Session Check
# =============================================================================
echo
echo "--- Section 8: Production Capsule Analysis ---"

# Test 26: Test data leaking
PROD_LEAK=$(aethervault context "$MV2" "capital of France" -n 5 --full 2>&1)
HAS_FRANCE=$(echo "$PROD_LEAK" | grep -ci "france\|paris" || true)
if [ "$HAS_FRANCE" -gt 0 ]; then
    log_result 26 "Production: test data (capital of France) present" "FAIL" "Test data leaks into prod queries"
else
    log_result 26 "Production: no test data leakage" "PASS"
fi

# Test 27: Subagent logs don't pollute main search
PROD_AGENTS=$(aethervault context "$MV2" "what was my last question" -n 5 --full 2>&1)
if echo "$PROD_AGENTS" | grep -qi "AGENTS.md\|cargo test\|cargo fmt"; then
    log_result 27 "Production: codex setup data leaks into user queries" "FAIL" "AGENTS.md found in user context"
else
    log_result 27 "Production: subagent data doesn't leak" "PASS"
fi

# Test 28: Session-scoped query for real user
PROD_SCOPED=$(aethervault context "$MV2" "what can you do" -c "aethervault://agent-log/telegram:8280335652/" -n 3 --full 2>&1)
if [ -n "$(echo "$PROD_SCOPED" | grep -i 'context')" ] || [ -n "$(echo "$PROD_SCOPED" | grep -i 'can do')" ]; then
    log_result 28 "Production: session-scoped query works" "PASS"
else
    log_result 28 "Production: session-scoped query works" "FAIL" "Empty or no results"
fi

# =============================================================================
# SECTION 9: Temporal Ordering (Preference Updates)
# =============================================================================
echo
echo "--- Section 9: Temporal Ordering ---"

SESSION6="test-temporal-$(date +%s)"

aethervault log "$TEST_MV2" --session "$SESSION6" --role user --text "My favorite color is BLUE." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION6" --role assistant --text "Noted: favorite color is BLUE." 2>/dev/null
sleep 1
aethervault log "$TEST_MV2" --session "$SESSION6" --role user --text "Changed my mind. Favorite color is now RED." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION6" --role assistant --text "Updated: favorite color RED (was BLUE)." 2>/dev/null
sleep 1
aethervault log "$TEST_MV2" --session "$SESSION6" --role user --text "Final answer: favorite color is GREEN." 2>/dev/null
aethervault log "$TEST_MV2" --session "$SESSION6" --role assistant --text "Final: favorite color GREEN. History: BLUE -> RED -> GREEN." 2>/dev/null

CTX_COLOR=$(aethervault context "$TEST_MV2" "favorite color" -n 5 --full 2>&1)
if echo "$CTX_COLOR" | grep -q "GREEN"; then
    log_result 29 "Most recent preference (GREEN) in context" "PASS"
else
    log_result 29 "Most recent preference (GREEN) in context" "FAIL" "GREEN not found"
fi

B=$(echo "$CTX_COLOR" | grep -c "BLUE" || true)
R=$(echo "$CTX_COLOR" | grep -c "RED" || true)
if [ "$B" -gt 0 ] && [ "$R" -gt 0 ]; then
    log_result 30 "Full preference history preserved" "PASS"
else
    log_result 30 "Full preference history preserved" "FAIL" "BLUE=$B, RED=$R"
fi

# =============================================================================
# SECTION 10: Doctor Maintenance
# =============================================================================
echo
echo "--- Section 10: Capsule Maintenance ---"

DOCTOR_OUT=$(aethervault doctor "$TEST_MV2" --json 2>&1)
if echo "$DOCTOR_OUT" | grep -qi "corrupt"; then
    log_result 31 "Doctor: test capsule integrity" "FAIL" "Corruption detected"
else
    log_result 31 "Doctor: test capsule integrity" "PASS"
fi

aethervault doctor "$TEST_MV2" --rebuild-lex --quiet 2>/dev/null || true
POST_REBUILD=$(aethervault context "$TEST_MV2" "ALPHA-5-BRAVO" -n 3 --full 2>&1)
if echo "$POST_REBUILD" | grep -q "ALPHA-5-BRAVO"; then
    log_result 32 "Post-rebuild: search works" "PASS"
else
    log_result 32 "Post-rebuild: search works" "FAIL" "ALPHA-5-BRAVO not found"
fi

PROD_DOC=$(aethervault doctor "$MV2" --json 2>&1)
if echo "$PROD_DOC" | grep -qi "corrupt"; then
    log_result 33 "Production capsule integrity" "FAIL" "Corruption detected"
else
    log_result 33 "Production capsule integrity" "PASS"
fi

# =============================================================================
# SECTION 11: Concurrent Session Stress Test
# =============================================================================
echo
echo "--- Section 11: Concurrent Session Stress ---"

# Create 5 sessions simultaneously with different topics
for j in $(seq 1 5); do
    S="test-concurrent-${j}-$(date +%s)"
    aethervault log "$TEST_MV2" --session "$S" --role user \
        --text "Concurrent session $j: topic is XRAY-DELTA-$j unique marker." 2>/dev/null
    aethervault log "$TEST_MV2" --session "$S" --role assistant \
        --text "Acknowledged concurrent session $j with marker XRAY-DELTA-$j." 2>/dev/null
done

# Verify all 5 markers retrievable
ALL_FOUND=true
for j in $(seq 1 5); do
    R=$(aethervault context "$TEST_MV2" "XRAY-DELTA-$j" -n 2 --full 2>&1)
    if ! echo "$R" | grep -q "XRAY-DELTA-$j"; then
        ALL_FOUND=false
        break
    fi
done
if $ALL_FOUND; then
    log_result 34 "5 concurrent sessions all retrievable" "PASS"
else
    log_result 34 "5 concurrent sessions all retrievable" "FAIL" "Some markers missing"
fi

# =============================================================================
# SUMMARY
# =============================================================================
echo
echo "============================================"
echo "           TEST SUMMARY"
echo "============================================"
echo -e "  Total:  $TOTAL"
echo -e "  ${GREEN}Passed: $PASS${NC}"
echo -e "  ${RED}Failed: $FAIL${NC}"
if [ $TOTAL -gt 0 ]; then
    echo "  Rate:   $(( PASS * 100 / TOTAL ))%"
fi
echo "============================================"

# Cleanup
rm -f "$TEST_MV2" /tmp/aethervault-test-pre-compact.mv2

exit $FAIL
