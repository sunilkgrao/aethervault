#!/bin/bash
# Usage: ./set-chat-id.sh <CHAT_ID>
# Sets the chat_id in all config files that reference it.
#
# To discover your chat_id:
# 1. Stop aethervault: systemctl stop aethervault
# 2. Send any message to your bot in Telegram
# 3. Run: curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getUpdates" | python3 -m json.tool
# 4. Look for .result[].message.chat.id
# 5. Run: ./set-chat-id.sh <that_id>
# 6. Restart: systemctl start aethervault

set -e
CHAT_ID="$1"
if [ -z "$CHAT_ID" ]; then
    echo "Usage: $0 <chat_id>"
    echo "Example: $0 123456789"
    exit 1
fi

CONFIG_DIR="/root/.aethervault/config"
updated=0

for f in "$CONFIG_DIR"/*.json; do
    [ -f "$f" ] || continue
    if python3 -c "import json; c=json.load(open()); chat_id in c" 2>/dev/null; then
        python3 -c "
import json
with open() as fh:
    c = json.load(fh)
c[chat_id] = int()
with open(, w) as fh:
    json.dump(c, fh, indent=2)
print(fUpdated with chat_id=)
"
        updated=$((updated + 1))
    fi
done

echo "Updated $updated config files. Restart services: systemctl restart aethervault"
