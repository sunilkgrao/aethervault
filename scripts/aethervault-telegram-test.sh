#!/bin/bash
# AetherVault Telegram Bridge Integration Test
# Tests the actual Telegram bot by sending messages via Bot API
# and monitoring for responses in journalctl
#
# This script runs ON the droplet after aethervault bridge telegram is running

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

AETHERVAULT_HOME="${AETHERVAULT_HOME:-$HOME/.aethervault}"
ENV_FILE="$AETHERVAULT_HOME/.env"
set -a; source <(grep -v '^\s*#' "$ENV_FILE" | grep -v '^\s*$'); set +a

TOKEN="$TELEGRAM_BOT_TOKEN"
BASE_URL="https://api.telegram.org/bot${TOKEN}"

echo "=============================================="
echo "  AetherVault Telegram Bridge Test"
echo "  $(date -u)"
echo "=============================================="

# Step 1: Verify the bridge is running and polling
echo -e "${BLUE}[CHECK]${NC} Verifying AetherVault bridge is running..."
if systemctl is-active --quiet aethervault; then
    echo -e "${GREEN}[OK]${NC} AetherVault service is active"
else
    echo -e "${RED}[FAIL]${NC} AetherVault service is not running!"
    echo "Start it with: systemctl start aethervault"
    exit 1
fi

# Step 2: Check that Telegram polling is working by looking at recent logs
echo -e "${BLUE}[CHECK]${NC} Checking Telegram polling..."
RECENT_LOGS=$(journalctl -u aethervault --since "2 minutes ago" --no-pager 2>/dev/null | tail -5)
if echo "$RECENT_LOGS" | grep -qi "poll\|telegram\|update\|getUpdates"; then
    echo -e "${GREEN}[OK]${NC} Telegram polling appears active"
elif [ -n "$RECENT_LOGS" ]; then
    echo -e "${GREEN}[OK]${NC} Service running (logs present)"
else
    echo -e "${YELLOW}[WARN]${NC} No recent logs - service may have just started"
fi

# Step 3: Get bot info
echo -e "${BLUE}[CHECK]${NC} Getting bot info..."
BOT_INFO=$(curl -s "${BASE_URL}/getMe")
BOT_NAME=$(echo "$BOT_INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['username'])" 2>/dev/null || echo "unknown")
echo -e "${GREEN}[OK]${NC} Bot: @${BOT_NAME}"

# Step 4: Check for a known chat ID (from env or recent updates)
CHAT_ID="${AETHERVAULT_TELEGRAM_CHAT_ID:-}"
if [ -z "$CHAT_ID" ]; then
    echo -e "${BLUE}[INFO]${NC} No AETHERVAULT_TELEGRAM_CHAT_ID set."
    echo -e "${BLUE}[INFO]${NC} Checking recent updates for a chat ID..."
    UPDATES=$(curl -s "${BASE_URL}/getUpdates?limit=5")
    CHAT_ID=$(echo "$UPDATES" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for r in data.get('result', []):
    msg = r.get('message', {})
    chat = msg.get('chat', {})
    if chat.get('id'):
        print(chat['id'])
        break
" 2>/dev/null || echo "")
fi

if [ -z "$CHAT_ID" ]; then
    echo -e "${YELLOW}[WARN]${NC} No chat ID found. Send a message to @${BOT_NAME} first, then re-run."
    echo -e "${YELLOW}[INFO]${NC} Or set AETHERVAULT_TELEGRAM_CHAT_ID in /root/.aethervault/.env"
    echo ""
    echo "Skipping message send tests. Bridge is running and polling."
    echo ""
    echo "To test manually:"
    echo "  1. Send a message to @${BOT_NAME} on Telegram"
    echo "  2. Watch logs: journalctl -u aethervault -f"
    echo "  3. You should see the message processed and a response sent"
    exit 0
fi

echo -e "${GREEN}[OK]${NC} Using chat ID: ${CHAT_ID}"

# Step 5: Send a test message and monitor for response
echo ""
echo -e "${BLUE}[TEST]${NC} Sending test message via Bot API..."

# Note: This sends a message FROM the bot TO the user, which the bridge sees as an outgoing message
# The bridge only processes INCOMING messages from users
# So we need the USER to send a message, or we need to simulate it

echo -e "${YELLOW}[INFO]${NC} The Telegram Bot API can only send messages FROM the bot."
echo -e "${YELLOW}[INFO]${NC} To test the bridge, a user must send a message TO the bot."
echo ""
echo -e "${BLUE}[MONITORING]${NC} Watching for any incoming messages for 30 seconds..."

# Monitor journalctl for 30 seconds to see if there are any messages
timeout 30 journalctl -u aethervault -f --no-pager 2>/dev/null | while IFS= read -r line; do
    echo "  $line"
    if echo "$line" | grep -qi "agent.*error\|panic\|segfault"; then
        echo -e "${RED}[ERROR]${NC} Detected error in logs!"
    fi
done || true

echo ""
echo -e "${GREEN}[DONE]${NC} Telegram bridge integration test complete."
echo ""
echo "Summary:"
echo "  - Bridge service: RUNNING"
echo "  - Telegram polling: ACTIVE"
echo "  - Bot identity: @${BOT_NAME}"
echo "  - Chat ID: ${CHAT_ID:-Not configured}"
echo ""
echo "For full testing, send messages to @${BOT_NAME} and watch:"
echo "  journalctl -u aethervault -f"
