#!/bin/bash
# Apply patches to aethervault dependencies
# Run this after npm updates

PI_AI_ANTHROPIC="/usr/lib/node_modules/aethervault/node_modules/@mariozechner/pi-ai/dist/providers/anthropic.js"

if [ -f "$PI_AI_ANTHROPIC" ]; then
    if grep -q "Only update input if provided" "$PI_AI_ANTHROPIC" 2>/dev/null; then
        echo "[patches] pi-ai token tracking fix: already applied"
    else
        sed -i "s/output.usage.input = event.usage.input_tokens || 0;/if (event.usage.input_tokens) output.usage.input = event.usage.input_tokens;/" "$PI_AI_ANTHROPIC"
        if [ $? -eq 0 ]; then
            echo "[patches] pi-ai token tracking fix: applied successfully"
        else
            echo "[patches] pi-ai token tracking fix: FAILED to apply"
        fi
    fi
else
    echo "[patches] pi-ai not found at expected location, skipping"
fi
