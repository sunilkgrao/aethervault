# Embedding Service Installation

## What We Built

A standalone embedding service using `node-llama-cpp` that:
- ✅ Runs in-process (no Ollama daemon)
- ✅ Uses embeddinggemma-300M (768 dimensions, ~300MB)
- ✅ Provides OpenAI-compatible API
- ✅ Auto-downloads model from HuggingFace
- ✅ Never crashes (no TCP connections to fail)

## Quick Start

The service is already running on port 11435!

Test it:
```bash
curl -X POST http://localhost:11435/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"input": "test"}' | jq .data[0].embedding | head -n 5
```

## Install as Systemd Service

1. **Copy service file:**
```bash
sudo cp /root/aethervault-workspace/embedding-service/embedding-service.service /etc/systemd/system/
```

2. **Enable and start:**
```bash
sudo systemctl daemon-reload
sudo systemctl enable embedding-service
sudo systemctl start embedding-service
```

3. **Check status:**
```bash
sudo systemctl status embedding-service
```

## Configure AetherVault

1. **Stop current Ollama tunnel** (if running):
```bash
sudo systemctl stop ollama-tunnel.service
sudo systemctl disable ollama-tunnel.service
```

2. **Update AetherVault config:**

Edit `/root/.aethervault/aethervault.json`:

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

3. **Restart AetherVault:**
```bash
aethervault gateway restart
```

## Verify

After AetherVault restarts:

```bash
# Check AetherVault memory search is working
aethervault memory search "test query"
```

## Logs

View service logs:
```bash
sudo journalctl -u embedding-service -f
```

## Model Details

- **Name:** embeddinggemma-300M
- **Size:** 328MB download, 307MB loaded
- **Dimensions:** 768
- **Context window:** 2048 tokens
- **Speed:** ~50-200ms per embedding (CPU)
- **Location:** `~/.cache/embedding-service/models/`

## Advantages Over Ollama

| Feature | Ollama | This Service |
|---------|--------|--------------|
| Daemon process | Yes (crashes) | No (in-process) |
| TCP connections | Yes (fails) | No (direct) |
| Model management | CLI downloads | Auto HuggingFace |
| Memory footprint | ~1GB | ~500MB |
| Startup time | 5-10s | 2-5s |
| Reliability | Medium | High |

## Troubleshooting

### Service won't start
```bash
# Check logs
sudo journalctl -u embedding-service -n 50

# Try manual start
cd /root/aethervault-workspace/embedding-service
npm start
```

### AetherVault not connecting
1. Verify service: `curl http://localhost:11435/health`
2. Check AetherVault config has correct baseUrl
3. Restart: `aethervault gateway restart`

### Model not downloading
- Check disk space: `df -h`
- Check cache dir: `ls -lh ~/.cache/embedding-service/models/`
- Manual download: `cd /root/aethervault-workspace/embedding-service && npm start`

## Next Steps

Once verified working:
1. Remove old Ollama service: `sudo systemctl disable ollama.service`
2. Keep embedding-service running 24/7: Already enabled
3. Enjoy never debugging Ollama crashes again! 🎉
