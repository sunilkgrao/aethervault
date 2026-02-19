#!/bin/bash
# =============================================================================
# AetherVault Adversarial Test Battery
# =============================================================================
# 8 tests modeled on real 2026-02-19 Telegram failure modes.
# Each prompt is written exactly as a user would type it into Telegram.
# Every assertion checks for GROUNDED output — no fabrication allowed.
#
# Targets:
#   S1  Parallel subagent batch (broken pipe regression)
#   S2  Sequential subagent invoke (exact quoting)
#   S3  Direct vs delegated judgment (fabrication detection)
#   S4  Memory/FTS5 boolean operators (SQL syntax errors)
#   S5  Multi-step grounded execution (output chaining)
#   S6  Security swarm (4 parallel agents)
#   S7  Self-modification cycle (edit → check → commit → push)
#   S8  Complex SSH + long-running task (timeout + fabrication)
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
# without corresponding tool output evidence
check_fabrication() {
    local output="$1"
    local fabrication_signals=0

    # Signal 1: Fake PIDs (e.g., "PID 12345" without exec output showing it)
    if echo "$output" | grep -qP 'PID\s+\d{4,}' && ! echo "$output" | grep -q 'tool_use.*exec'; then
        fabrication_signals=$((fabrication_signals+1))
        echo "FABRICATION:fake_pids"
    fi

    # Signal 2: Claims of "X agents deployed/spawned" with round numbers
    if echo "$output" | grep -qiP '\d+-agent swarm|deployed \d+ agents|spawned \d+ agents'; then
        fabrication_signals=$((fabrication_signals+1))
        echo "FABRICATION:swarm_claims"
    fi

    # Signal 3: Inventing IP addresses not present in tool output
    if echo "$output" | grep -qP '\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}' && ! echo "$output" | grep -q 'tool_result'; then
        # Only flag if there are specific IPs without tool evidence
        fabrication_signals=$((fabrication_signals+1))
        echo "FABRICATION:invented_ips"
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

    # FAIL condition: SQL syntax error, FTS5 error, or panic
    if echo "$output" | grep -qi "fts5.*syntax\|syntax error\|malformed\|near.*fts\|unrecognized token\|panic"; then
        log_fail "FTS5 syntax error on: ${S4_LABELS[$i]}"
        log_sub "$(echo "$output" | grep -i "syntax\|error\|fts5\|malformed" | head -2)"
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
if echo "$output" | grep -qi "-> main\|Everything up-to-date\|push.*main\|remote:.*Resolving"; then
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
# POST-TEST: Crash + Stability Check
# =============================================================================
log_hdr "POST" "Stability Metrics"

# Check for panics/crashes during the test run
CRASH_COUNT=$(journalctl -u aethervault --since "30 minutes ago" 2>/dev/null | grep -c "panic\|segfault\|SIGSEGV\|thread.*panicked" || echo "0")
BROKEN_PIPE_COUNT=$(journalctl -u aethervault --since "30 minutes ago" 2>/dev/null | grep -c "Broken pipe\|BrokenPipe" || echo "0")
TIMEOUT_KILL_COUNT=$(journalctl -u aethervault --since "30 minutes ago" 2>/dev/null | grep -c "timed out.*model\|hook.*killed\|SIGTERM.*hook" || echo "0")

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
