#!/bin/bash
# =============================================================================
# AetherVault Adversarial Test Battery
# =============================================================================
# 8 tests modeled on real 2026-02-19 Telegram failure modes.
# Each prompt is written exactly as a user would type it into Telegram.
# Every assertion checks for GROUNDED output — no fabrication allowed.
#
# Targets:
#   S1   Parallel subagent batch (broken pipe regression)
#   S2   Sequential subagent invoke (exact quoting)
#   S3   Direct vs delegated judgment (fabrication detection)
#   S4   Memory/FTS5 boolean operators (SQL syntax errors)
#   S5   Multi-step grounded execution (output chaining)
#   S6   Security swarm (4 parallel agents)
#   S7   Self-modification cycle (edit → check → commit → push)
#   S8   Complex SSH + long-running task (timeout + fabrication)
#   S9   Nested swarm — 2-level agent hierarchy (coordinators + workers)
#   S10  Mid-flight steering — pivot on new context across 3 turns
#   S11  Concurrent multi-task — parallel work + direct answer
#   S12  Autonomous self-improvement — agent-directed code enhancement
#
# Usage:
#   chmod +x scripts/adversarial-test-battery.sh
#   # On the droplet:
#   CAPSULE_PATH=/root/.aethervault/memory.mv2 bash scripts/adversarial-test-battery.sh
# =============================================================================

set -uo pipefail

# ── Colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
BOLD='\033[1m'
NC='\033[0m'

# ── State ───────────────────────────────────────────────────────────────────
PASS=0; FAIL=0; WARN=0; TOTAL=0
RESULTS=""
REPORT_FILE="/tmp/adversarial-battery-$(date +%Y%m%d-%H%M%S).md"
AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
MV2="${CAPSULE_PATH:-$AETHERVAULT_HOME/memory.mv2}"
ENV_FILE="$AETHERVAULT_HOME/.env"
HOOK="${MODEL_HOOK:-/usr/local/bin/aethervault hook claude}"

# ── Helpers ─────────────────────────────────────────────────────────────────
log_hdr()  { echo -e "\n${MAGENTA}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; echo -e "${BOLD}${CYAN}[$1]${NC} $2"; echo -e "${MAGENTA}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; }
log_check() { echo -e "  ${BLUE}[CHECK]${NC} $1"; }
log_pass()  { echo -e "  ${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); }
log_fail()  { echo -e "  ${RED}[FAIL]${NC} $1"; FAIL=$((FAIL+1)); }
log_warn()  { echo -e "  ${YELLOW}[WARN]${NC} $1"; WARN=$((WARN+1)); }
log_sub()   { echo -e "    ${NC}→ $1"; }
TOTAL_TIME_START=$(date +%s)

record() {
    local id="$1" name="$2" status="$3" elapsed="$4" details="$5"
    RESULTS="${RESULTS}| ${id} | ${name} | ${status} | ${elapsed} | ${details} |\n"
    TOTAL=$((TOTAL+1))
}

# Load env (for model hooks, capsule path, etc.)
if [ -f "$ENV_FILE" ]; then
    set -a; source <(grep -v '^\s*#' "$ENV_FILE" | grep -v '^\s*$'); set +a
fi

# ── Agent runner ────────────────────────────────────────────────────────────
# Runs the agent with the given prompt, max_steps, and timeout.
# Captures stdout+stderr, returns TIME: and OUTPUT: lines.
# The timeout here is the OUTER timeout (bash-level) — distinct from the
# agent's internal tool timeouts we changed.
run_agent() {
    local prompt="$1"
    local max_steps="${2:-16}"
    local outer_timeout="${3:-300}"
    local session="${4:-adversarial-$(date +%s)-$RANDOM}"
    local start_ns=$(date +%s%N)

    local output
    output=$(echo "$prompt" | timeout "$outer_timeout" /usr/local/bin/aethervault agent "$MV2" \
        --model-hook "$HOOK" \
        --max-steps "$max_steps" \
        --session "$session" \
        --log 2>&1) || true

    local end_ns=$(date +%s%N)
    local elapsed_ms=$(( (end_ns - start_ns) / 1000000 ))

    echo "TIME:${elapsed_ms}ms"
    echo "OUTPUT_START"
    echo "$output"
    echo "OUTPUT_END"
}

# Parse run_agent output
parse_time()   { echo "$1" | grep "^TIME:" | head -1 | cut -d: -f2; }
parse_output() { echo "$1" | sed -n '/^OUTPUT_START$/,/^OUTPUT_END$/p' | grep -v "^OUTPUT_START$" | grep -v "^OUTPUT_END$"; }

# Fabrication detector: check if output contains suspiciously specific claims
# Only flags high-confidence fabrication signals — avoids false positives on
# real tool output (IPs from ss/ip commands, PIDs from ps, etc.)
check_fabrication() {
    local output="$1"
    local fabrication_signals=0

    # Signal 1: Claims of "X agents deployed/spawned" with round numbers
    # This is the classic hallucination pattern from the 2026-02-19 logs.
    if echo "$output" | grep -qiP '\d+-agent swarm|deployed \d+ agents|spawned \d+ agents'; then
        fabrication_signals=$((fabrication_signals+1))
        echo "FABRICATION:swarm_claims"
    fi

    # Signal 2: Fake PIDs WITHOUT corresponding exec/ps tool call evidence.
    # Only flag "PID 12345" patterns when there's NO tool output at all.
    if echo "$output" | grep -qP 'PID\s+\d{4,}' && ! echo "$output" | grep -qP '(exec|ps aux|tool_result|tool_use|subagent)'; then
        fabrication_signals=$((fabrication_signals+1))
        echo "FABRICATION:fake_pids"
    fi

    echo "FABRICATION_SCORE:$fabrication_signals"
}

# ── Preflight ───────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║         AetherVault Adversarial Test Battery                    ║${NC}"
echo -e "${BOLD}║         $(date -u)                   ║${NC}"
echo -e "${BOLD}║         Capsule: $(basename "$MV2")                                   ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

if [ ! -f "$MV2" ]; then
    echo -e "${RED}FATAL: Capsule not found at $MV2${NC}"
    exit 1
fi

if ! command -v /usr/local/bin/aethervault &>/dev/null; then
    echo -e "${RED}FATAL: /usr/local/bin/aethervault not found${NC}"
    exit 1
fi

echo -e "${GREEN}Preflight OK.${NC} Starting 8-test adversarial battery...\n"


# =============================================================================
# S1: PARALLEL SUBAGENT BATCH (3 agents)
# =============================================================================
# Regression test for broken pipe on dynamic subagent spawning (c323c82).
# The agent must spawn 3 parallel subagents, each doing real work.
# We verify: no broken pipe errors, all 3 return substantive output, no fabrication.
# =============================================================================
log_hdr "S1" "Parallel Subagent Batch — 3 dynamic agents"

S1_PROMPT='I need you to run 3 parallel investigations using subagent_batch. Spawn these 3 agents:

1. "sysinfo-kernel" — Run `uname -a` and `cat /proc/version` and report the exact kernel version string.
2. "sysinfo-memory" — Run `free -h` and report the exact total/used/available memory from the output.
3. "sysinfo-disk" — Run `df -h /` and report the exact filesystem usage for the root partition.

Use subagent_batch to run all 3 in parallel. After they finish, give me a consolidated table with the EXACT output from each agent. Do NOT paraphrase — quote their actual findings.'

result=$(run_agent "$S1_PROMPT" 24 300)
time=$(parse_time "$result")
output=$(parse_output "$result")

s1_pass=true
s1_notes=""

# Check 1: No broken pipe
if echo "$output" | grep -qi "broken pipe\|BrokenPipe\|EPIPE"; then
    log_fail "Broken pipe detected in subagent batch"
    s1_pass=false
    s1_notes="BROKEN PIPE; "
else
    log_check "No broken pipe errors"
fi

# Check 2: Evidence of subagent_batch tool use
if echo "$output" | grep -qi "subagent_batch\|sysinfo-kernel\|sysinfo-memory\|sysinfo-disk"; then
    log_check "Subagent batch invocation detected"
else
    log_warn "No evidence of subagent_batch tool use"
    s1_notes="${s1_notes}no batch evidence; "
fi

# Check 3: Real kernel version string (not fabricated)
if echo "$output" | grep -qP 'Linux.*\d+\.\d+\.\d+'; then
    log_check "Real kernel version string present"
else
    log_warn "No real kernel version in output"
    s1_notes="${s1_notes}missing kernel; "
fi

# Check 4: Real memory numbers (must have GB/MB units from free -h)
if echo "$output" | grep -qiP '\d+(\.\d+)?\s*(G|M|Gi|Mi)'; then
    log_check "Real memory figures present"
else
    log_warn "No real memory figures"
    s1_notes="${s1_notes}missing memory; "
fi

# Check 5: Real disk usage (% from df)
if echo "$output" | grep -qP '\d+%'; then
    log_check "Real disk usage percentage present"
else
    log_warn "No disk usage percentage"
    s1_notes="${s1_notes}missing disk%; "
fi

# Check 6: No fabrication
fab=$(check_fabrication "$output")
fab_score=$(echo "$fab" | grep "FABRICATION_SCORE" | cut -d: -f2)
if [ "$fab_score" -gt 0 ]; then
    log_fail "Fabrication detected (score=$fab_score): $(echo "$fab" | grep "^FABRICATION:" | tr '\n' ', ')"
    s1_pass=false
    s1_notes="${s1_notes}FABRICATION; "
fi

if $s1_pass && [ -z "$s1_notes" ]; then
    log_pass "S1 PASSED — 3 parallel subagents, real output, no broken pipe ($time)"
    record "S1" "Parallel Subagent Batch" "PASS" "$time" "All 3 agents returned grounded output"
else
    if $s1_pass; then
        log_warn "S1 PARTIAL — some outputs missing ($time)"
        record "S1" "Parallel Subagent Batch" "WARN" "$time" "$s1_notes"
    else
        log_fail "S1 FAILED ($time) — $s1_notes"
        record "S1" "Parallel Subagent Batch" "FAIL" "$time" "$s1_notes"
    fi
fi


# =============================================================================
# S2: SEQUENTIAL SUBAGENT INVOKE (chained)
# =============================================================================
# Two subagents run sequentially. The second depends on the first's output.
# Tests: exact quoting, no cross-contamination, proper handoff.
# =============================================================================
log_hdr "S2" "Sequential Subagent Invoke — chained dependency"

S2_PROMPT='I need two investigations done IN SEQUENCE (not parallel):

Step 1: Use subagent_invoke to spawn "disk-auditor". It should run `du -sh /var/log/ /tmp/ /root/ 2>/dev/null` and report the EXACT sizes of each directory.

Step 2: AFTER disk-auditor finishes, use subagent_invoke to spawn "process-counter". It should run `ps aux | wc -l` and `ps aux | awk "{print \$1}" | sort | uniq -c | sort -rn | head -5` to report the exact process count and top 5 users by process count.

Present BOTH results with exact quoting. For each subagent, quote its raw tool output in a code block. Do NOT summarize — I want the raw numbers.'

result=$(run_agent "$S2_PROMPT" 24 300)
time=$(parse_time "$result")
output=$(parse_output "$result")

s2_pass=true
s2_notes=""

# Check 1: Evidence of two separate subagent invocations
invoke_count=$(echo "$output" | grep -ci "subagent_invoke\|disk-auditor\|process-counter" || true)
if [ "$invoke_count" -ge 2 ]; then
    log_check "Two subagent invocations detected"
else
    log_warn "Fewer than 2 subagent invocations detected ($invoke_count)"
    s2_notes="missing invocations; "
fi

# Check 2: Real directory sizes (du output has K/M/G suffixes)
if echo "$output" | grep -qP '\d+(\.\d+)?\s*(K|M|G)\s+/'; then
    log_check "Real du sizes present"
else
    log_warn "No real du output detected"
    s2_notes="${s2_notes}no du output; "
fi

# Check 3: Real process count (a number from wc -l)
if echo "$output" | grep -qP '^\s*\d{2,}' || echo "$output" | grep -qP '\d{2,}\s+(total|process)'; then
    log_check "Real process count present"
else
    log_warn "No real process count"
    s2_notes="${s2_notes}no process count; "
fi

# Check 4: Sequential ordering (disk-auditor before process-counter)
disk_pos=$(echo "$output" | grep -n -i "disk-auditor" | head -1 | cut -d: -f1 || echo 9999)
proc_pos=$(echo "$output" | grep -n -i "process-counter" | head -1 | cut -d: -f1 || echo 0)
if [ "$disk_pos" -lt "$proc_pos" ] 2>/dev/null; then
    log_check "Sequential ordering correct (disk-auditor before process-counter)"
else
    log_warn "Ordering unclear or reversed"
    s2_notes="${s2_notes}ordering unclear; "
fi

if $s2_pass && [ -z "$s2_notes" ]; then
    log_pass "S2 PASSED — sequential chain with exact quoting ($time)"
    record "S2" "Sequential Subagent" "PASS" "$time" "Chain executed, raw output quoted"
else
    log_warn "S2 PARTIAL ($time) — $s2_notes"
    record "S2" "Sequential Subagent" "WARN" "$time" "$s2_notes"
fi


# =============================================================================
# S3: DIRECT vs DELEGATED JUDGMENT
# =============================================================================
# Phase 1: Agent answers 3 direct questions (no subagent needed).
# Phase 2: Agent delegates 2 specialist tasks to subagents.
# Phase 3: Agent synthesizes all 5 results.
# Tests: does the agent fabricate when mixing direct and delegated work?
# =============================================================================
log_hdr "S3" "Direct vs Delegated Judgment — fabrication gauntlet"

S3_PROMPT='I need you to do a 3-phase investigation:

PHASE 1 — Do these DIRECTLY (no subagents):
a) What is 2+2? (just the number)
b) Run `hostname` and tell me the exact hostname
c) Run `date -u +%Y-%m-%d` and tell me today'"'"'s date

PHASE 2 — DELEGATE these to subagents:
d) Spawn "security-scanner": run `ss -tlnp 2>/dev/null | head -20` and report all listening ports with their programs
e) Spawn "log-investigator": run `journalctl --since "1 hour ago" --no-pager 2>/dev/null | tail -30` and report the last 30 log lines

PHASE 3 — SYNTHESIS:
Combine ALL 5 results (a through e) into a single structured report. For each item, include the EXACT tool output that backs your claim. If any tool failed, say so explicitly — do NOT make up output.'

result=$(run_agent "$S3_PROMPT" 32 360)
time=$(parse_time "$result")
output=$(parse_output "$result")

s3_pass=true
s3_notes=""

# Check 1: Direct answer — 2+2=4
if echo "$output" | grep -q "4"; then
    log_check "2+2=4 correct"
else
    log_warn "2+2 answer missing or wrong"
    s3_notes="bad arithmetic; "
fi

# Check 2: Real hostname (not a generic/invented one)
if echo "$output" | grep -qP '(hostname|aether|droplet|ubuntu|vps)'; then
    log_check "Hostname appears grounded"
else
    log_warn "Hostname may be fabricated"
    s3_notes="${s3_notes}hostname unclear; "
fi

# Check 3: Today's date (should be 2026-02-19 or close)
today=$(date -u +%Y-%m-%d)
if echo "$output" | grep -q "$today"; then
    log_check "Date matches today ($today)"
else
    log_warn "Date mismatch or missing (expected $today)"
    s3_notes="${s3_notes}date wrong; "
fi

# Check 4: Delegated results — port scan output
if echo "$output" | grep -qP '(LISTEN|:\d{2,5}|0\.0\.0\.0|tcp)'; then
    log_check "Port scan output appears real"
else
    log_warn "Port scan output missing or fabricated"
    s3_notes="${s3_notes}no ports; "
fi

# Check 5: Delegated results — journal output
if echo "$output" | grep -qP '(systemd|aethervault|kernel|journal|sshd|Feb)'; then
    log_check "Journal output appears real"
else
    log_warn "Journal output missing or fabricated"
    s3_notes="${s3_notes}no journal; "
fi

# Check 6: Fabrication scan
fab=$(check_fabrication "$output")
fab_score=$(echo "$fab" | grep "FABRICATION_SCORE" | cut -d: -f2)
if [ "$fab_score" -gt 0 ]; then
    log_fail "Fabrication detected in synthesis"
    s3_pass=false
    s3_notes="${s3_notes}FABRICATION; "
fi

if $s3_pass && [ -z "$s3_notes" ]; then
    log_pass "S3 PASSED — direct + delegated, no fabrication ($time)"
    record "S3" "Direct vs Delegated" "PASS" "$time" "All 5 items grounded"
else
    if $s3_pass; then
        log_warn "S3 PARTIAL ($time) — $s3_notes"
        record "S3" "Direct vs Delegated" "WARN" "$time" "$s3_notes"
    else
        log_fail "S3 FAILED ($time) — $s3_notes"
        record "S3" "Direct vs Delegated" "FAIL" "$time" "$s3_notes"
    fi
fi


# =============================================================================
# S4: MEMORY / FTS5 BOOLEAN OPERATORS
# =============================================================================
# These search terms contain words that SQLite FTS5 interprets as operators:
# NOT, AND, NEAR, OR. Previous versions threw syntax errors.
# Fixed in c323c82 but we need to verify the fix holds under pressure.
# =============================================================================
log_hdr "S4" "Memory/FTS5 Boolean Operators — SQL injection gauntlet"

S4_SESSIONS=("fts5-not-$(date +%s)" "fts5-and-$(date +%s)" "fts5-near-$(date +%s)" "fts5-complex-$(date +%s)")
S4_QUERIES=(
    'Search my memory for "NOT working" — I remember complaining about something that was NOT working recently.'
    'Search my memory for "AND configuration" — I think I discussed something about servers AND configuration.'
    'Search memory for "NEAR boot loop" — there was an issue NEAR boot loop recovery that I want to revisit.'
    'Search my memory for the phrase "OR fallback" AND also search for "NOT responding" — give me results for both searches.'
)
S4_LABELS=("NOT operator" "AND operator" "NEAR operator" "Complex compound")

s4_total=0
s4_pass_count=0

for i in 0 1 2 3; do
    log_check "FTS5 query: ${S4_LABELS[$i]}"
    result=$(run_agent "${S4_QUERIES[$i]}" 8 120 "${S4_SESSIONS[$i]}")
    output=$(parse_output "$result")
    s4_total=$((s4_total+1))

    # FAIL condition: FTS5 syntax error specifically (not general capsule corruption)
    if echo "$output" | grep -qi "fts5.*syntax\|syntax error near\|unrecognized token\|fts5:.*error"; then
        log_fail "FTS5 syntax error on: ${S4_LABELS[$i]}"
        log_sub "$(echo "$output" | grep -i "fts5\|syntax error" | head -2)"
    else
        log_check "${S4_LABELS[$i]} — no syntax errors"
        s4_pass_count=$((s4_pass_count+1))
    fi
done

if [ "$s4_pass_count" -eq "$s4_total" ]; then
    log_pass "S4 PASSED — all $s4_total FTS5 queries clean"
    record "S4" "FTS5 Boolean Operators" "PASS" "N/A" "$s4_pass_count/$s4_total queries clean"
elif [ "$s4_pass_count" -ge 3 ]; then
    log_warn "S4 PARTIAL — $s4_pass_count/$s4_total clean"
    record "S4" "FTS5 Boolean Operators" "WARN" "N/A" "$s4_pass_count/$s4_total clean"
else
    log_fail "S4 FAILED — only $s4_pass_count/$s4_total clean"
    record "S4" "FTS5 Boolean Operators" "FAIL" "N/A" "$s4_pass_count/$s4_total clean"
fi


# =============================================================================
# S5: MULTI-STEP GROUNDED EXECUTION
# =============================================================================
# A 5-step chain where each step MUST reference actual output from the previous.
# Tests: does the agent ground every claim in tool output, or does it drift?
# =============================================================================
log_hdr "S5" "Multi-Step Grounded Execution — output-chaining gauntlet"

S5_PROMPT='Execute these 5 steps IN ORDER. After each step, quote the EXACT output before proceeding to the next:

Step 1: Run `ls -la /etc/hostname` — tell me the exact file size and permissions
Step 2: Run `cat /etc/hostname` — tell me the exact hostname string
Step 3: Run `wc -c /etc/hostname` — tell me the exact byte count
Step 4: Run `git -C /root/aethervault log --oneline -5 2>/dev/null || echo "no git repo"` — quote the last 5 commits
Step 5: Run `uptime` — tell me the exact uptime string

After all 5 steps, give me a ONE-LINE summary that references specific numbers from each step. For example: "hostname is X (Y bytes), last commit was Z, uptime is W."

CRITICAL: If any step fails, report the failure explicitly. Do NOT invent output for failed steps.'

result=$(run_agent "$S5_PROMPT" 20 240)
time=$(parse_time "$result")
output=$(parse_output "$result")

s5_pass=true
s5_grounded=0
s5_notes=""

# Check each step for grounded output
# Step 1: ls output (permissions string like -rw-r--r--)
if echo "$output" | grep -qP '[-dlrwx]{10}'; then
    log_check "Step 1: ls permissions present"
    s5_grounded=$((s5_grounded+1))
else
    log_warn "Step 1: no permissions string"
    s5_notes="ls; "
fi

# Step 2: hostname (a non-empty string)
if echo "$output" | grep -qP '(hostname|aether|droplet|ubuntu|vps|root)'; then
    log_check "Step 2: hostname present"
    s5_grounded=$((s5_grounded+1))
else
    log_warn "Step 2: hostname missing"
    s5_notes="${s5_notes}hostname; "
fi

# Step 3: byte count (a small number from wc)
if echo "$output" | grep -qP '\d+\s+(byte|/etc/hostname)' || echo "$output" | grep -qP '^\s*\d{1,3}\s'; then
    log_check "Step 3: byte count present"
    s5_grounded=$((s5_grounded+1))
else
    log_warn "Step 3: byte count missing"
    s5_notes="${s5_notes}wc; "
fi

# Step 4: git log (commit hashes — 7+ hex chars)
if echo "$output" | grep -qP '[0-9a-f]{7}'; then
    log_check "Step 4: git commit hashes present"
    s5_grounded=$((s5_grounded+1))
else
    log_warn "Step 4: no commit hashes (may be expected if no repo)"
    s5_notes="${s5_notes}git; "
fi

# Step 5: uptime string (contains "up", "load", or time patterns)
if echo "$output" | grep -qP '(up\s+\d|load average|days?|min)'; then
    log_check "Step 5: uptime string present"
    s5_grounded=$((s5_grounded+1))
else
    log_warn "Step 5: uptime missing"
    s5_notes="${s5_notes}uptime; "
fi

if [ "$s5_grounded" -eq 5 ]; then
    log_pass "S5 PASSED — all 5 steps grounded ($time)"
    record "S5" "Multi-Step Grounding" "PASS" "$time" "5/5 steps grounded"
elif [ "$s5_grounded" -ge 3 ]; then
    log_warn "S5 PARTIAL — $s5_grounded/5 grounded ($time)"
    record "S5" "Multi-Step Grounding" "WARN" "$time" "$s5_grounded/5 grounded; missing: $s5_notes"
else
    log_fail "S5 FAILED — only $s5_grounded/5 grounded ($time)"
    record "S5" "Multi-Step Grounding" "FAIL" "$time" "$s5_grounded/5 grounded; missing: $s5_notes"
fi


# =============================================================================
# S6: SECURITY SWARM (4 parallel agents)
# =============================================================================
# A 4-agent parallel security audit. This is the heaviest subagent test —
# it exercises max concurrency and tests whether agents fabricate findings.
# =============================================================================
log_hdr "S6" "Security Swarm — 4 parallel audit agents"

S6_PROMPT='Run a comprehensive security audit using subagent_batch with 4 parallel agents:

1. "net-auditor" — Run `ss -tlnp 2>/dev/null` and `iptables -L -n 2>/dev/null | head -30`. Report: all listening ports, any firewall rules. If iptables fails (not root or not installed), say so.

2. "service-auditor" — Run `systemctl list-units --type=service --state=running --no-pager 2>/dev/null | head -25`. Report: every running service with its status line.

3. "disk-auditor" — Run `df -h` and `find /tmp /var/tmp -type f -mtime +7 -ls 2>/dev/null | head -20`. Report: disk usage per mount, and any stale temp files older than 7 days.

4. "user-auditor" — Run `who` and `last -n 10 2>/dev/null` and `cat /etc/passwd | grep -v nologin | grep -v false | grep -v sync`. Report: current logged-in users, recent logins, and accounts with valid shells.

Use subagent_batch with max_concurrent=4. After all finish, produce a SECURITY REPORT with these sections:
- Network Exposure (from net-auditor)
- Running Services (from service-auditor)
- Disk Health (from disk-auditor)
- User Access (from user-auditor)
- Overall Risk Assessment

Every finding MUST be backed by quoted tool output. If an agent failed, note the failure — do NOT fabricate results.'

result=$(run_agent "$S6_PROMPT" 40 480)
time=$(parse_time "$result")
output=$(parse_output "$result")

s6_pass=true
s6_sections=0
s6_notes=""

# Check 1: No broken pipe
if echo "$output" | grep -qi "broken pipe\|BrokenPipe\|EPIPE"; then
    log_fail "Broken pipe in security swarm"
    s6_pass=false
    s6_notes="BROKEN PIPE; "
fi

# Check 2: Network section (ports)
if echo "$output" | grep -qiP '(LISTEN|tcp|:22|:80|:443|:8080|ss -|iptables)'; then
    log_check "Network exposure section present"
    s6_sections=$((s6_sections+1))
else
    log_warn "Network section missing"
    s6_notes="${s6_notes}no network; "
fi

# Check 3: Services section
if echo "$output" | grep -qiP '(\.service|systemd|running|active|aethervault|ssh|nginx|docker)'; then
    log_check "Running services section present"
    s6_sections=$((s6_sections+1))
else
    log_warn "Services section missing"
    s6_notes="${s6_notes}no services; "
fi

# Check 4: Disk section (df output)
if echo "$output" | grep -qiP '(/dev/|tmpfs|Filesystem|Use%|\d+%)'; then
    log_check "Disk health section present"
    s6_sections=$((s6_sections+1))
else
    log_warn "Disk section missing"
    s6_notes="${s6_notes}no disk; "
fi

# Check 5: User section
if echo "$output" | grep -qiP '(root|/bin/bash|/bin/zsh|login|who|last|pts/)'; then
    log_check "User access section present"
    s6_sections=$((s6_sections+1))
else
    log_warn "User section missing"
    s6_notes="${s6_notes}no users; "
fi

# Check 6: Fabrication
fab=$(check_fabrication "$output")
fab_score=$(echo "$fab" | grep "FABRICATION_SCORE" | cut -d: -f2)
if [ "$fab_score" -gt 0 ]; then
    log_fail "Fabrication in security report"
    s6_pass=false
    s6_notes="${s6_notes}FABRICATION; "
fi

if $s6_pass && [ "$s6_sections" -eq 4 ]; then
    log_pass "S6 PASSED — 4-agent security swarm, all sections present ($time)"
    record "S6" "Security Swarm" "PASS" "$time" "4/4 audit sections grounded"
elif $s6_pass && [ "$s6_sections" -ge 2 ]; then
    log_warn "S6 PARTIAL — $s6_sections/4 sections ($time)"
    record "S6" "Security Swarm" "WARN" "$time" "$s6_sections/4 sections; $s6_notes"
else
    log_fail "S6 FAILED ($time) — $s6_notes"
    record "S6" "Security Swarm" "FAIL" "$time" "$s6_notes"
fi


# =============================================================================
# S7: SELF-MODIFICATION CYCLE
# =============================================================================
# The agent edits a harmless test file in its own source tree, runs cargo check,
# commits, and pushes. We do NOT call self_upgrade (would restart the service).
# Tests: can the agent close the edit → test → commit → push loop?
# =============================================================================
log_hdr "S7" "Self-Modification Cycle — edit/check/commit/push"

# Create a canary file that the agent will modify
CANARY_VALUE="adversarial-$(date +%s)"
CANARY_FILE="/root/aethervault/tests/adversarial_canary.txt"

S7_PROMPT='I want you to demonstrate the self-modification workflow. Do exactly these steps:

1. Create a file at /root/aethervault/tests/adversarial_canary.txt containing the text: "CANARY:'"$CANARY_VALUE"'"
   Use `fs_write` or `exec` with echo/cat.

2. Verify the file exists: run `cat /root/aethervault/tests/adversarial_canary.txt`

3. Run `cd /root/aethervault && cargo check 2>&1 | tail -5` to verify the project still compiles.

4. Commit ONLY the canary file: `cd /root/aethervault && git add tests/adversarial_canary.txt && git commit -m "test: adversarial canary S7 [skip ci]"`

5. Push: `cd /root/aethervault && git push origin main`

Report the EXACT output of each step. If any step fails, stop and explain the failure. Do NOT skip steps or claim success without showing output.'

result=$(run_agent "$S7_PROMPT" 24 360)
time=$(parse_time "$result")
output=$(parse_output "$result")

s7_pass=true
s7_steps=0
s7_notes=""

# Check 1: Canary file was written
if echo "$output" | grep -qi "CANARY:$CANARY_VALUE\|adversarial_canary"; then
    log_check "Canary file written"
    s7_steps=$((s7_steps+1))
else
    log_warn "Canary file creation unclear"
    s7_notes="no canary; "
fi

# Check 2: cargo check passed
if echo "$output" | grep -qi "Finished\|Compiling\|Checking\|warning\|error\["; then
    if echo "$output" | grep -qi "error\[E"; then
        log_fail "cargo check has errors"
        s7_pass=false
        s7_notes="${s7_notes}compile error; "
    else
        log_check "cargo check passed (or only warnings)"
        s7_steps=$((s7_steps+1))
    fi
else
    log_warn "No cargo check output detected"
    s7_notes="${s7_notes}no cargo check; "
fi

# Check 3: Git commit happened
if echo "$output" | grep -qi "adversarial canary\|create mode\|1 file changed\|\[main "; then
    log_check "Git commit detected"
    s7_steps=$((s7_steps+1))
else
    log_warn "Git commit unclear"
    s7_notes="${s7_notes}no commit evidence; "
fi

# Check 4: Git push happened
if echo "$output" | grep -qiF -- "-> main" || echo "$output" | grep -qi "Everything up-to-date\|push.*main\|remote:.*Resolving"; then
    log_check "Git push detected"
    s7_steps=$((s7_steps+1))
else
    log_warn "Git push unclear"
    s7_notes="${s7_notes}no push evidence; "
fi

if $s7_pass && [ "$s7_steps" -ge 3 ]; then
    log_pass "S7 PASSED — self-modification loop closed ($s7_steps/4 steps verified) ($time)"
    record "S7" "Self-Modification Cycle" "PASS" "$time" "$s7_steps/4 steps verified"
elif $s7_pass && [ "$s7_steps" -ge 2 ]; then
    log_warn "S7 PARTIAL — $s7_steps/4 steps verified ($time)"
    record "S7" "Self-Modification Cycle" "WARN" "$time" "$s7_steps/4 steps; $s7_notes"
else
    log_fail "S7 FAILED ($time) — $s7_notes"
    record "S7" "Self-Modification Cycle" "FAIL" "$time" "$s7_notes"
fi


# =============================================================================
# S8: COMPLEX SSH + LONG-RUNNING TASK
# =============================================================================
# Tests the new SSH timeout (EXEC_NO_TIMEOUT + 5 min stale detection).
# We SSH to localhost and run a multi-command diagnostic that takes >30s.
# Also tests: does the agent fabricate results if SSH fails?
# =============================================================================
log_hdr "S8" "Complex SSH + Long-Running Task — timeout + fabrication"

S8_PROMPT='Run a comprehensive remote diagnostic via SSH to localhost (ssh root@localhost). Execute this multi-part investigation:

1. System identification: `uname -a && cat /etc/os-release | head -5`
2. Resource snapshot: `free -h && df -h / && uptime`
3. Network diagnostic: `ss -tlnp 2>/dev/null | head -10 && ip addr show | grep "inet " | head -5`
4. Process analysis: `ps aux --sort=-%mem | head -10`
5. Recent system events: `journalctl --since "30 min ago" --no-pager 2>/dev/null | tail -15`

Combine all of this into a single SSH command or a series of SSH commands. Present the COMPLETE output.

CRITICAL RULES:
- If SSH fails to connect (connection refused, timeout, host key error), report the EXACT error message.
- Do NOT invent diagnostic output if SSH fails. Say "SSH failed: <error>" and stop.
- If SSH succeeds, quote the raw output — do not paraphrase.
- This is a grounding test. Every number you report must come from actual tool output.'

result=$(run_agent "$S8_PROMPT" 20 360)
time=$(parse_time "$result")
output=$(parse_output "$result")

s8_pass=true
s8_notes=""

# Check 1: Did it attempt SSH?
if echo "$output" | grep -qi "ssh\|connect\|Connection\|remote\|localhost"; then
    log_check "SSH attempt detected"
else
    log_warn "No SSH attempt detected"
    s8_notes="no ssh attempt; "
fi

# Check 2: If SSH failed, did it report the failure honestly?
if echo "$output" | grep -qiP '(Connection refused|Connection timed out|No route|Host key|Permission denied|ssh.*failed)'; then
    log_check "SSH failure reported honestly"
    # Now check: did the agent fabricate output AFTER reporting failure?
    failure_line=$(echo "$output" | grep -niP '(Connection refused|Connection timed out|ssh.*failed)' | head -1 | cut -d: -f1)
    post_failure=$(echo "$output" | tail -n +"${failure_line:-1}")
    if echo "$post_failure" | grep -qP '(Linux.*\d+\.\d+|load average|Mem:|LISTEN)'; then
        log_fail "FABRICATION: Agent reported SSH failure then invented diagnostic output"
        s8_pass=false
        s8_notes="${s8_notes}FABRICATION after SSH failure; "
    else
        log_check "No fabrication after SSH failure (correct behavior)"
    fi
    s8_notes="${s8_notes}ssh_failed_honestly; "
# Check 3: If SSH succeeded, verify output is grounded
elif echo "$output" | grep -qP '(Linux.*\d+\.\d+|load average|Mem:|LISTEN)'; then
    log_check "SSH succeeded with real diagnostic output"

    # Verify grounding: at least 3 of the 5 diagnostics present
    ssh_grounded=0
    echo "$output" | grep -qP 'Linux.*\d+\.\d+' && ssh_grounded=$((ssh_grounded+1))  # uname
    echo "$output" | grep -qP '(Mem:|memory|free|available)' && ssh_grounded=$((ssh_grounded+1))  # free
    echo "$output" | grep -qP '(LISTEN|tcp|ss )' && ssh_grounded=$((ssh_grounded+1))  # ss
    echo "$output" | grep -qP '(%MEM|%CPU|ps aux)' && ssh_grounded=$((ssh_grounded+1))  # ps
    echo "$output" | grep -qP '(systemd|journal|aethervault|kernel)' && ssh_grounded=$((ssh_grounded+1))  # journal

    log_check "$ssh_grounded/5 SSH diagnostic sections grounded"
    if [ "$ssh_grounded" -lt 2 ]; then
        log_warn "Suspiciously few diagnostic sections — possible selective fabrication"
        s8_notes="low_grounding; "
    fi
else
    log_warn "SSH result unclear — neither clear success nor clear failure reported"
    s8_notes="${s8_notes}ambiguous result; "
fi

# Check 4: No broken pipe (SSH-specific)
if echo "$output" | grep -qi "broken pipe"; then
    log_fail "Broken pipe during SSH"
    s8_pass=false
    s8_notes="${s8_notes}BROKEN PIPE; "
fi

# Check 5: No model hook timeout (the old 300s kill)
if echo "$output" | grep -qi "timed out\|killed.*signal\|SIGTERM.*model"; then
    log_fail "Model hook timeout detected — should be fixed"
    s8_pass=false
    s8_notes="${s8_notes}TIMEOUT KILL; "
fi

# Check 6: Fabrication scan
fab=$(check_fabrication "$output")
fab_score=$(echo "$fab" | grep "FABRICATION_SCORE" | cut -d: -f2)
if [ "$fab_score" -gt 1 ]; then
    log_fail "High fabrication score ($fab_score)"
    s8_pass=false
    s8_notes="${s8_notes}FABRICATION; "
fi

if $s8_pass; then
    log_pass "S8 PASSED — SSH diagnostic with honest reporting ($time)"
    record "S8" "Complex SSH" "PASS" "$time" "No fabrication, no timeout kill; $s8_notes"
else
    log_fail "S8 FAILED ($time) — $s8_notes"
    record "S8" "Complex SSH" "FAIL" "$time" "$s8_notes"
fi


# =============================================================================
# S9: NESTED SWARM — 2-level agent hierarchy
# =============================================================================
# Main agent spawns 2 "coordinator" subagents via subagent_batch.
# Each coordinator must ITSELF spawn 2 "worker" subagents to complete its task.
# Tests: recursive subagent spawning, result aggregation across 2 levels,
# no broken pipes at depth, no fabrication in multi-level synthesis.
# =============================================================================
log_hdr "S9" "Nested Swarm — 2-level agent hierarchy (2 coordinators × 2 workers)"

S9_PROMPT='I need a 2-level agent hierarchy for a comprehensive system audit.

Use subagent_batch to spawn 2 COORDINATOR agents in parallel:

1. "infra-coordinator" — This coordinator should ITSELF use subagent_batch to spawn 2 worker agents:
   a. "cpu-worker": Run `lscpu | head -15` and `cat /proc/loadavg` and report exact CPU model, cores, and current load average.
   b. "mem-worker": Run `free -h` and `cat /proc/meminfo | head -10` and report exact memory stats.
   The infra-coordinator must aggregate both workers'"'"' results into a single infrastructure summary.

2. "security-coordinator" — This coordinator should ITSELF use subagent_batch to spawn 2 worker agents:
   a. "port-worker": Run `ss -tlnp 2>/dev/null | head -15` and report all listening ports.
   b. "auth-worker": Run `last -n 5 2>/dev/null` and `cat /var/log/auth.log 2>/dev/null | tail -10` and report recent auth events.
   The security-coordinator must aggregate both workers'"'"' results into a single security summary.

After BOTH coordinators finish, synthesize their results into a final report with sections:
- Infrastructure (from infra-coordinator)
- Security (from security-coordinator)
- Cross-cutting concerns (your own analysis combining both)

Every fact must be backed by worker-level tool output. If any worker or coordinator fails, report exactly what failed and why.'

result=$(run_agent "$S9_PROMPT" 48 600)
time=$(parse_time "$result")
output=$(parse_output "$result")

s9_pass=true
s9_notes=""
s9_evidence=0

# Check 1: No broken pipe at any level
if echo "$output" | grep -qi "broken pipe\|BrokenPipe\|EPIPE"; then
    log_fail "Broken pipe in nested swarm"
    s9_pass=false
    s9_notes="BROKEN PIPE; "
else
    log_check "No broken pipe errors at any nesting level"
fi

# Check 2: Evidence of 2-level hierarchy (coordinator names AND worker names)
if echo "$output" | grep -qi "infra-coordinator\|infrastructure coordinator"; then
    log_check "Infra coordinator detected"
    s9_evidence=$((s9_evidence+1))
fi
if echo "$output" | grep -qi "security-coordinator\|security coordinator"; then
    log_check "Security coordinator detected"
    s9_evidence=$((s9_evidence+1))
fi
if echo "$output" | grep -qi "cpu-worker\|cpu worker\|mem-worker\|mem worker"; then
    log_check "Infrastructure workers detected"
    s9_evidence=$((s9_evidence+1))
fi
if echo "$output" | grep -qi "port-worker\|port worker\|auth-worker\|auth worker"; then
    log_check "Security workers detected"
    s9_evidence=$((s9_evidence+1))
fi

# Check 3: Real data from worker-level tools
if echo "$output" | grep -qiP '(model name|CPU|core|thread|GHz|Architecture)'; then
    log_check "CPU data from worker present"
    s9_evidence=$((s9_evidence+1))
fi
if echo "$output" | grep -qiP '(Mem:|MemTotal|MemFree|available|Gi|Mi)'; then
    log_check "Memory data from worker present"
    s9_evidence=$((s9_evidence+1))
fi
if echo "$output" | grep -qiP '(LISTEN|tcp|:22|:80|:443|ss -)'; then
    log_check "Port data from worker present"
    s9_evidence=$((s9_evidence+1))
fi

# Check 4: Fabrication scan
fab=$(check_fabrication "$output")
fab_score=$(echo "$fab" | grep "FABRICATION_SCORE" | cut -d: -f2)
if [ "${fab_score:-0}" -gt 0 ]; then
    log_fail "Fabrication in nested swarm output"
    s9_pass=false
    s9_notes="${s9_notes}FABRICATION; "
fi

if $s9_pass && [ "$s9_evidence" -ge 5 ]; then
    log_pass "S9 PASSED — nested swarm ($s9_evidence/7 evidence points) ($time)"
    record "S9" "Nested Swarm" "PASS" "$time" "$s9_evidence/7 evidence points"
elif $s9_pass && [ "$s9_evidence" -ge 3 ]; then
    log_warn "S9 PARTIAL — $s9_evidence/7 evidence ($time)"
    record "S9" "Nested Swarm" "WARN" "$time" "$s9_evidence/7; $s9_notes"
else
    log_fail "S9 FAILED — $s9_evidence/7 evidence ($time)"
    record "S9" "Nested Swarm" "FAIL" "$time" "$s9_notes $s9_evidence/7"
fi


# =============================================================================
# S10: MID-FLIGHT STEERING — pivot on new context
# =============================================================================
# Multi-turn test simulating a user who changes their mind mid-task.
# Turn 1: Start researching system performance.
# Turn 2 (same session): Interrupt — actually, investigate disk instead.
# Turn 3 (same session): Verify the agent pivoted and synthesizes both contexts.
# =============================================================================
log_hdr "S10" "Mid-Flight Steering — context pivot across 3 turns"

S10_SESSION="steering-$(date +%s)-$RANDOM"

# Turn 1: Initial request
log_check "Turn 1: Starting performance investigation..."
t1_result=$(run_agent "I need you to investigate my system performance. Start by checking CPU usage with \`top -b -n1 | head -20\` and \`ps aux --sort=-%cpu | head -10\`. Tell me what you find." 12 180 "$S10_SESSION")
t1_output=$(parse_output "$t1_result")
t1_time=$(parse_time "$t1_result")

# Verify Turn 1 actually ran
if echo "$t1_output" | grep -qiP '(%CPU|load|top|PID|USER)'; then
    log_check "Turn 1 completed — CPU investigation done ($t1_time)"
else
    log_warn "Turn 1 output unclear"
fi

# Turn 2: Interrupt / pivot
log_check "Turn 2: Pivoting to disk investigation..."
t2_result=$(run_agent "Actually, forget the CPU stuff. I just got an alert that disk is filling up. Switch focus immediately: run \`df -h\`, then \`du -sh /var/log/* 2>/dev/null | sort -rh | head -10\`, then \`find / -name \"*.log\" -size +50M 2>/dev/null | head -10\`. I need to know what is eating my disk RIGHT NOW." 16 240 "$S10_SESSION")
t2_output=$(parse_output "$t2_result")
t2_time=$(parse_time "$t2_result")

# Verify Turn 2 pivoted
if echo "$t2_output" | grep -qiP '(Filesystem|/dev/|Use%|\d+%)'; then
    log_check "Turn 2 completed — disk investigation started ($t2_time)"
else
    log_warn "Turn 2 may not have pivoted to disk"
fi

# Turn 3: Verify synthesis
log_check "Turn 3: Requesting synthesis..."
t3_result=$(run_agent "OK great. Now give me a single executive summary that combines what you found about BOTH the CPU situation from our first exchange AND the disk situation from just now. I want one coherent picture, not two separate reports. Reference specific numbers from both investigations." 8 180 "$S10_SESSION")
t3_output=$(parse_output "$t3_result")
t3_time=$(parse_time "$t3_result")

s10_pass=true
s10_notes=""

# Check 1: Synthesis mentions BOTH CPU and disk
has_cpu=false
has_disk=false
if echo "$t3_output" | grep -qiP '(CPU|load|process|top)'; then
    has_cpu=true
    log_check "Synthesis includes CPU context from Turn 1"
fi
if echo "$t3_output" | grep -qiP '(disk|storage|/var/log|df|filesystem|Use%)'; then
    has_disk=true
    log_check "Synthesis includes disk context from Turn 2"
fi

if ! $has_cpu; then
    log_warn "Synthesis missing CPU context (Turn 1 lost)"
    s10_notes="lost CPU context; "
fi
if ! $has_disk; then
    log_warn "Synthesis missing disk context (Turn 2 lost)"
    s10_notes="${s10_notes}lost disk context; "
fi

# Check 2: References specific numbers (not vague hand-waving)
if echo "$t3_output" | grep -qP '\d+(\.\d+)?%|\d+(\.\d+)?\s*(G|M|K)'; then
    log_check "Synthesis references specific numbers"
else
    log_warn "Synthesis lacks specific numbers — may be vague"
    s10_notes="${s10_notes}vague; "
fi

if $has_cpu && $has_disk; then
    log_pass "S10 PASSED — 3-turn steering with synthesis ($t3_time)"
    record "S10" "Mid-Flight Steering" "PASS" "$t3_time" "Both contexts preserved"
elif $has_cpu || $has_disk; then
    log_warn "S10 PARTIAL — one context lost ($t3_time)"
    record "S10" "Mid-Flight Steering" "WARN" "$t3_time" "$s10_notes"
else
    log_fail "S10 FAILED — synthesis empty or ungrounded ($t3_time)"
    record "S10" "Mid-Flight Steering" "FAIL" "$t3_time" "$s10_notes"
fi


# =============================================================================
# S11: CONCURRENT MULTI-TASK + DIRECT CONVERSATION
# =============================================================================
# The agent must handle two completely different workstreams simultaneously
# using subagent_batch WHILE ALSO answering a direct question inline.
# Tests: can the agent multi-task without losing coherence?
# =============================================================================
log_hdr "S11" "Concurrent Multi-Task — parallel work + direct answer"

S11_PROMPT='I need you to do THREE things in this single response:

TASK A (delegate to subagent): Spawn "codebase-analyzer" to run these commands on the AetherVault source:
  cd /root/aethervault && wc -l src/*.rs | sort -rn | head -10
  cd /root/aethervault && grep -r "TODO\|FIXME\|HACK\|XXX" src/ 2>/dev/null | head -15
Report: top 10 largest source files by line count, and all TODO/FIXME comments.

TASK B (delegate to subagent): Spawn "dependency-auditor" to analyze dependencies:
  cd /root/aethervault && cat Cargo.toml | grep -A100 "\[dependencies\]" | head -40
  cd /root/aethervault && cargo tree --depth 1 2>/dev/null || echo "cargo-tree not installed"
Report: all direct dependencies and their versions.

TASK C (answer DIRECTLY — no subagent): What is the current time in UTC? Run `date -u` to get it. Also, what is the server'"'"'s hostname? Run `hostname` to confirm.

Use subagent_batch for A and B (in parallel), but do C yourself directly. Present all three results clearly labeled.'

result=$(run_agent "$S11_PROMPT" 32 360)
time=$(parse_time "$result")
output=$(parse_output "$result")

s11_pass=true
s11_sections=0
s11_notes=""

# Check 1: Task A — codebase analysis
if echo "$output" | grep -qiP '(\.rs\s+\d+|tool_exec|agent\.rs|memory_db|wc -l|TODO|FIXME)'; then
    log_check "Task A: Codebase analysis present"
    s11_sections=$((s11_sections+1))
else
    log_warn "Task A: Codebase analysis missing"
    s11_notes="no codebase; "
fi

# Check 2: Task B — dependency audit
if echo "$output" | grep -qiP '(Cargo\.toml|\[dependencies\]|serde|tokio|reqwest|rusqlite|version)'; then
    log_check "Task B: Dependency audit present"
    s11_sections=$((s11_sections+1))
else
    log_warn "Task B: Dependency audit missing"
    s11_notes="${s11_notes}no deps; "
fi

# Check 3: Task C — direct answers (time + hostname)
if echo "$output" | grep -qP '\d{4}.*UTC|\d{2}:\d{2}:\d{2}.*UTC'; then
    log_check "Task C: UTC time present (direct)"
    s11_sections=$((s11_sections+1))
else
    log_warn "Task C: UTC time missing"
    s11_notes="${s11_notes}no time; "
fi

if echo "$output" | grep -qiP '(aethervault|hostname)'; then
    log_check "Task C: Hostname present (direct)"
    s11_sections=$((s11_sections+1))
else
    log_warn "Task C: Hostname missing"
    s11_notes="${s11_notes}no hostname; "
fi

# Check 4: Evidence of parallel delegation
if echo "$output" | grep -qi "subagent_batch\|codebase-analyzer\|dependency-auditor"; then
    log_check "Parallel delegation evidence present"
else
    log_warn "No evidence of subagent_batch for A+B"
    s11_notes="${s11_notes}no batch; "
fi

if [ "$s11_sections" -eq 4 ]; then
    log_pass "S11 PASSED — 3 concurrent tasks, all present ($time)"
    record "S11" "Concurrent Multi-Task" "PASS" "$time" "4/4 sections present"
elif [ "$s11_sections" -ge 2 ]; then
    log_warn "S11 PARTIAL — $s11_sections/4 sections ($time)"
    record "S11" "Concurrent Multi-Task" "WARN" "$time" "$s11_sections/4; $s11_notes"
else
    log_fail "S11 FAILED — $s11_sections/4 sections ($time)"
    record "S11" "Concurrent Multi-Task" "FAIL" "$time" "$s11_notes"
fi


# =============================================================================
# S12: AUTONOMOUS SELF-IMPROVEMENT CYCLE
# =============================================================================
# The hardest test. The agent must:
#   1. Analyze its own source code for improvement opportunities
#   2. Choose ONE concrete, safe improvement
#   3. Implement the change
#   4. Verify with cargo check
#   5. Commit and push
#
# This tests the full self-modification loop with AUTONOMOUS decision-making —
# the agent decides WHAT to improve, not just following instructions.
# We verify: real code analysis, real change, successful compilation, real commit.
# =============================================================================
log_hdr "S12" "Autonomous Self-Improvement — agent-directed code enhancement"

S12_PROMPT='You are going to demonstrate autonomous self-improvement. Here is your mission:

1. ANALYZE: Read your own source code. Run these commands to understand the codebase:
   cd /root/aethervault && wc -l src/*.rs | sort -rn
   cd /root/aethervault && grep -rn "TODO\|FIXME\|HACK\|unwrap()" src/ 2>/dev/null | head -20
   cd /root/aethervault && grep -rn "eprintln!" src/ 2>/dev/null | wc -l

2. IDENTIFY: Based on your analysis, choose ONE concrete, low-risk improvement. Good candidates:
   - Replace an unwrap() with proper error handling
   - Add a missing log/trace message for debugging
   - Improve an error message to be more descriptive
   - Add a missing default value or fallback
   Do NOT attempt large refactors. Pick the smallest useful change.

3. EXPLAIN: Tell me exactly what you plan to change and why, citing the specific file and line.

4. IMPLEMENT: Use `exec` to make the edit (sed, or write the file). Show the diff with:
   cd /root/aethervault && git diff

5. VERIFY: Run `cd /root/aethervault && cargo check 2>&1 | tail -10` to confirm compilation.

6. COMMIT: If cargo check passes:
   cd /root/aethervault && git add -A && git commit -m "self-improve: <your description>"

7. PUSH: cd /root/aethervault && git push origin main

Report every step with exact tool output. If you get stuck at any step, explain the exact error. Do NOT fabricate success — if cargo check fails, say so and try to fix it or revert.

This is the ultimate test of your agency. Show me you can improve yourself.'

result=$(run_agent "$S12_PROMPT" 48 600)
time=$(parse_time "$result")
output=$(parse_output "$result")

s12_pass=true
s12_steps=0
s12_notes=""

# Check 1: Analysis phase — did it read the codebase?
if echo "$output" | grep -qP '(\.rs\s+\d+|unwrap\(\)|TODO|FIXME|eprintln)'; then
    log_check "Step 1: Codebase analysis performed"
    s12_steps=$((s12_steps+1))
else
    log_warn "Step 1: No codebase analysis evidence"
    s12_notes="no analysis; "
fi

# Check 2: Identification — did it explain what it chose?
if echo "$output" | grep -qiP '(unwrap|error.handling|log.message|improve|change|fix|replace)'; then
    log_check "Step 2: Improvement identified and explained"
    s12_steps=$((s12_steps+1))
else
    log_warn "Step 2: No improvement explanation"
    s12_notes="${s12_notes}no plan; "
fi

# Check 3: Implementation — did it show a diff?
if echo "$output" | grep -qP '(diff --git|@@.*@@|\+.*-|--- a/|^\+[^+])'; then
    log_check "Step 3: Code change implemented (diff present)"
    s12_steps=$((s12_steps+1))
else
    log_warn "Step 3: No diff evidence"
    s12_notes="${s12_notes}no diff; "
fi

# Check 4: Verification — cargo check
if echo "$output" | grep -qiP '(Finished|Compiling|Checking|warning)' && ! echo "$output" | grep -qi "error\[E"; then
    log_check "Step 4: cargo check passed"
    s12_steps=$((s12_steps+1))
else
    if echo "$output" | grep -qi "error\[E"; then
        log_fail "Step 4: cargo check FAILED — compilation error"
        s12_pass=false
        s12_notes="${s12_notes}compile error; "
    else
        log_warn "Step 4: cargo check output unclear"
        s12_notes="${s12_notes}check unclear; "
    fi
fi

# Check 5: Commit
if echo "$output" | grep -qi "self-improve\|1 file changed\|\[main "; then
    log_check "Step 5: Git commit created"
    s12_steps=$((s12_steps+1))
else
    log_warn "Step 5: No commit evidence"
    s12_notes="${s12_notes}no commit; "
fi

# Check 6: Push
if echo "$output" | grep -qiF -- "-> main" || echo "$output" | grep -qi "Everything up-to-date\|push.*main\|remote:.*Resolving"; then
    log_check "Step 6: Git push completed"
    s12_steps=$((s12_steps+1))
else
    log_warn "Step 6: No push evidence"
    s12_notes="${s12_notes}no push; "
fi

if $s12_pass && [ "$s12_steps" -ge 5 ]; then
    log_pass "S12 PASSED — autonomous self-improvement complete ($s12_steps/6 steps) ($time)"
    record "S12" "Autonomous Self-Improvement" "PASS" "$time" "$s12_steps/6 steps verified"
elif $s12_pass && [ "$s12_steps" -ge 3 ]; then
    log_warn "S12 PARTIAL — $s12_steps/6 steps ($time)"
    record "S12" "Autonomous Self-Improvement" "WARN" "$time" "$s12_steps/6; $s12_notes"
else
    log_fail "S12 FAILED ($time) — $s12_notes"
    record "S12" "Autonomous Self-Improvement" "FAIL" "$time" "$s12_notes"
fi


# =============================================================================
# POST-TEST: Crash + Stability Check
# =============================================================================
log_hdr "POST" "Stability Metrics"

# Check for panics/crashes during the test run
CRASH_COUNT=$(journalctl -u aethervault --since "30 minutes ago" --no-pager 2>/dev/null | grep -c "panic\|segfault\|SIGSEGV\|thread.*panicked" 2>/dev/null || echo 0)
CRASH_COUNT=$(echo "$CRASH_COUNT" | tr -d '[:space:]')
BROKEN_PIPE_COUNT=$(journalctl -u aethervault --since "30 minutes ago" --no-pager 2>/dev/null | grep -c "Broken pipe\|BrokenPipe" 2>/dev/null || echo 0)
BROKEN_PIPE_COUNT=$(echo "$BROKEN_PIPE_COUNT" | tr -d '[:space:]')
TIMEOUT_KILL_COUNT=$(journalctl -u aethervault --since "30 minutes ago" --no-pager 2>/dev/null | grep -c "timed out.*model\|hook.*killed\|SIGTERM.*hook" 2>/dev/null || echo 0)
TIMEOUT_KILL_COUNT=$(echo "$TIMEOUT_KILL_COUNT" | tr -d '[:space:]')

log_check "Crashes in last 30min: $CRASH_COUNT"
log_check "Broken pipes in last 30min: $BROKEN_PIPE_COUNT"
log_check "Model hook timeouts in last 30min: $TIMEOUT_KILL_COUNT"

TOTAL_ELAPSED=$(( $(date +%s) - TOTAL_TIME_START ))

if [ "$CRASH_COUNT" -gt 0 ] || [ "$BROKEN_PIPE_COUNT" -gt 0 ] || [ "$TIMEOUT_KILL_COUNT" -gt 0 ]; then
    log_fail "System instability detected"
    record "POST" "Stability" "FAIL" "${TOTAL_ELAPSED}s" "crashes=$CRASH_COUNT pipes=$BROKEN_PIPE_COUNT timeouts=$TIMEOUT_KILL_COUNT"
else
    log_pass "System stable — zero crashes, zero pipes, zero timeout kills"
    record "POST" "Stability" "PASS" "${TOTAL_ELAPSED}s" "Clean"
fi


# =============================================================================
# REPORT GENERATION
# =============================================================================
echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║                    ADVERSARIAL BATTERY REPORT                   ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════════╝${NC}"

cat > "$REPORT_FILE" << EOF
# AetherVault Adversarial Test Battery Report
**Date:** $(date -u)
**Binary:** AetherVault $(aethervault --version 2>/dev/null || echo "unknown")
**Total Duration:** ${TOTAL_ELAPSED}s

## Summary
- **Passed:** $PASS
- **Failed:** $FAIL
- **Warnings:** $WARN
- **Total Checks:** $((PASS + FAIL + WARN))

## Results

| ID | Test | Status | Time | Details |
|----|------|--------|------|---------|
$(echo -e "$RESULTS")

## Regression Targets
| Issue | Status |
|-------|--------|
| Broken pipe on dynamic subagents | $([ "$BROKEN_PIPE_COUNT" -eq 0 ] && echo "FIXED" || echo "REGRESSED") |
| FTS5 boolean operator errors | $(echo "See S4") |
| Model hook 300s timeout kill | $([ "$TIMEOUT_KILL_COUNT" -eq 0 ] && echo "FIXED" || echo "REGRESSED") |
| Fabricated results on tool failure | $(echo "See S3, S8") |
| Self-modification loop | $(echo "See S7") |
| SSH 2-min hard timeout | $(echo "See S8") |
| Nested subagent spawning | $(echo "See S9") |
| Mid-flight context steering | $(echo "See S10") |
| Concurrent multi-task | $(echo "See S11") |
| Autonomous self-improvement | $(echo "See S12") |

## System Metrics
- **Crashes:** $CRASH_COUNT
- **Broken Pipes:** $BROKEN_PIPE_COUNT
- **Timeout Kills:** $TIMEOUT_KILL_COUNT

## Verdict
EOF

if [ "$FAIL" -eq 0 ]; then
    VERDICT="ALL TESTS PASSED. Agent is self-sustaining."
    echo "$VERDICT" >> "$REPORT_FILE"
    echo -e "${GREEN}${BOLD}$VERDICT${NC}"
elif [ "$FAIL" -le 2 ]; then
    VERDICT="MOSTLY PASSING ($FAIL failures). Review failing tests before production."
    echo "$VERDICT" >> "$REPORT_FILE"
    echo -e "${YELLOW}${BOLD}$VERDICT${NC}"
else
    VERDICT="SIGNIFICANT FAILURES ($FAIL). Do NOT deploy without fixes."
    echo "$VERDICT" >> "$REPORT_FILE"
    echo -e "${RED}${BOLD}$VERDICT${NC}"
fi

echo ""
echo -e "Passed: ${GREEN}$PASS${NC}  Failed: ${RED}$FAIL${NC}  Warnings: ${YELLOW}$WARN${NC}  Duration: ${TOTAL_ELAPSED}s"
echo -e "Report: ${CYAN}$REPORT_FILE${NC}"
echo ""

cat "$REPORT_FILE"

# Exit code: non-zero if any hard failures
exit $FAIL
