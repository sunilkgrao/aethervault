#!/bin/bash
# Start Kimi K2.5 server using llama.cpp following the unsloth.ai guide
# https://unsloth.ai/blog/kimi-k2

MODEL_DIR="/mnt/a/llm-models/Kimi-K2.5-GGUF/UD-TQ1_0"
LLAMA_DIR="/mnt/a/llm-models/llama.cpp"
MODEL_FILE="$MODEL_DIR/Kimi-K2.5-UD-TQ1_0-00001-of-00005.gguf"
PORT=${1:-11434}

# Check if model files exist
if [ ! -f "$MODEL_FILE" ]; then
    echo "Error: Model file not found: $MODEL_FILE"
    exit 1
fi

# Check if llama-server exists
if [ ! -f "$LLAMA_DIR/llama-server.exe" ]; then
    echo "Error: llama-server.exe not found in $LLAMA_DIR"
    echo "Please extract llama.cpp first"
    exit 1
fi

cd "$LLAMA_DIR"

echo "Starting Kimi K2.5 server on port $PORT..."
echo "Model: $MODEL_FILE"
echo "Configuration:"
echo "  - Context size: 16384"
echo "  - Temperature: 1.0"
echo "  - Top-P: 0.95"
echo "  - Min-P: 0.01"
echo "  - MoE layers offloaded to CPU: .ffn_.*_exps."
echo ""

# Start the server with unsloth recommended settings
# LLAMA_SET_ROWS=1 optimizes memory access patterns
# -ot ".ffn_.*_exps.=CPU" offloads MoE expert layers to CPU
# --fit on enables automatic fitting of model to available memory
# --jinja enables Jinja2 template support
# --kv-unified uses unified KV cache

LLAMA_SET_ROWS=1 ./llama-server.exe \
    --model "$MODEL_FILE" \
    --temp 1.0 \
    --min_p 0.01 \
    --top-p 0.95 \
    --ctx-size 16384 \
    --fit on \
    --jinja \
    --kv-unified \
    -ot ".ffn_.*_exps.=CPU" \
    --port $PORT \
    --host 0.0.0.0

# If llama-server.exe doesn't work, try llama-server (no .exe)
if [ $? -ne 0 ]; then
    echo "Trying without .exe extension..."
    LLAMA_SET_ROWS=1 ./llama-server \
        --model "$MODEL_FILE" \
        --temp 1.0 \
        --min_p 0.01 \
        --top-p 0.95 \
        --ctx-size 16384 \
        --fit on \
        --jinja \
        --kv-unified \
        -ot ".ffn_.*_exps.=CPU" \
        --port $PORT \
        --host 0.0.0.0
fi
