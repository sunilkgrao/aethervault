# Embedding Service

Standalone embedding service using `node-llama-cpp` with `embeddinggemma-300M`.

## Features

- ✅ **No daemon dependencies** - runs in-process (no Ollama crashes)
- ✅ **OpenAI-compatible API** - drop-in replacement for OpenAI embeddings
- ✅ **Auto-downloads model** from HuggingFace on first run
- ✅ **Small footprint** - ~300MB model, low memory usage
- ✅ **Rock-solid reliability** - no TCP connections, no restarts
- ✅ **Inactivity timeout** - auto-disposes context after 5min idle (keeps model warm)
- ✅ **Batch processing** - parallel embedding with activity tracking
- ✅ **Metrics endpoint** - monitor performance and usage
- ✅ **Concurrent load protection** - prevents duplicate model loads
- ✅ **Production-ready** - enhanced logging, health checks, graceful shutdown

Based on [qmd](https://github.com/tobi/qmd)'s battle-tested patterns.

## Quick Start

```bash
# Install dependencies
npm install

# Development (with auto-reload)
npm run dev

# Production build
npm run build
npm start
```

## API Usage

### OpenAI-Compatible Endpoint

```bash
curl -X POST http://localhost:11435/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{
    "input": "Your text here",
    "model": "embeddinggemma-300M"
  }'
```

Response:
```json
{
  "object": "list",
  "data": [
    {
      "object": "embedding",
      "embedding": [0.123, -0.456, ...],
      "index": 0
    }
  ],
  "model": "embeddinggemma-300M",
  "usage": {
    "prompt_tokens": 4,
    "total_tokens": 4
  }
}
```

### Batch Embeddings

```bash
curl -X POST http://localhost:11435/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{
    "input": ["Text one", "Text two", "Text three"]
  }'
```

### Health Check

```bash
curl http://localhost:11435/health
```

Response:
```json
{
  "status": "ready",
  "model": "hf:ggml-org/embeddinggemma-300M-GGUF/embeddinggemma-300M-Q8_0.gguf",
  "uptime": 45230,
  "lastActivity": 1523,
  "contextLoaded": true,
  "modelLoaded": true
}
```

### Metrics

```bash
curl http://localhost:11435/metrics
```

Response:
```json
{
  "totalRequests": 142,
  "totalEmbeddings": 387,
  "errors": 0,
  "averageLatencyMs": 65,
  "modelLoads": 1,
  "contextReloads": 1,
  "startTime": 1769989167863,
  "uptime": 45230,
  "requestsPerSecond": 3.14,
  "lastActivity": 1523,
  "contextLoaded": true,
  "modelLoaded": true
}
```

## AetherVault Integration

Update your `aethervault.json`:

```json
{
  "agents": {
    "defaults": {
      "memorySearch": {
        "enabled": true,
        "provider": "remote",
        "remote": {
          "baseUrl": "http://localhost:11435/v1",
          "apiKey": "not-needed"
        },
        "model": "embeddinggemma-300M"
      }
    }
  }
}
```

Then restart AetherVault:
```bash
aethervault gateway restart
```

## Systemd Service

Install as a systemd service for automatic startup:

```bash
# Copy service file
sudo cp embedding-service.service /etc/systemd/system/

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable embedding-service
sudo systemctl start embedding-service

# Check status
sudo systemctl status embedding-service

# View logs
sudo journalctl -u embedding-service -f
```

## Configuration

Environment variables:

- `PORT` - Server port (default: 11435)
- `MODEL_CACHE_DIR` - Model download directory (default: `~/.cache/embedding-service/models`)
- `EMBED_MODEL` - HuggingFace model URI (default: `hf:ggml-org/embeddinggemma-300M-GGUF/embeddinggemma-300M-Q8_0.gguf`)
- `INACTIVITY_TIMEOUT_MS` - Context disposal timeout in ms (default: 300000 = 5 minutes, 0 to disable)

### Memory Management

The service uses a two-tier memory management strategy:

1. **Model** (~300MB) - Expensive to load, stays resident
2. **Context** (~100MB) - Cheap to recreate, disposed after inactivity

After 5 minutes of no requests, the context is disposed to save memory (~100MB freed), while the model stays loaded. Next request recreates the context in ~100ms vs ~3s for full model reload.

**Result:** Low memory during idle, fast response on resume.

## Model

Uses **embeddinggemma-300M** (same as qmd):
- Size: ~300MB
- Dimensions: 768
- Format: GGUF (quantized)
- Source: HuggingFace GGML org

Auto-downloads on first request.

## Performance

- **Startup time**: ~2-5 seconds (loads model into memory)
- **First request**: May take 5-10 seconds (downloads model if needed)
- **Subsequent requests**: ~50-200ms per embedding
- **Memory usage**: ~500MB (model + runtime)

## Troubleshooting

### Model not downloading

Check disk space and ensure `~/.cache/embedding-service/models` is writable.

### Service won't start

```bash
# Check logs
sudo journalctl -u embedding-service -n 50

# Test manually
npm run build && npm start
```

### AetherVault not connecting

1. Verify service is running: `curl http://localhost:11435/health`
2. Check AetherVault config has correct `baseUrl`
3. Restart AetherVault: `aethervault gateway restart`
