# Deployment Guide

This document covers local, Docker, and cloud deployments (including DigitalOcean droplet) with minimal setup.

## Local (native)

```bash
cargo build --release
./target/release/aethervault init ./data/knowledge.mv2
./target/release/aethervault mcp ./data/knowledge.mv2
```

## Docker (single container)

```bash
docker build -t aethervault .
mkdir -p data

docker run --rm -it -v "$(pwd)/data:/data" aethervault init /data/knowledge.mv2
docker run --rm -it -v "$(pwd)/data:/data" aethervault mcp /data/knowledge.mv2
```

## Docker Compose (local or server)

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export ANTHROPIC_MODEL=claude-<model>

docker compose up --build
```

## DigitalOcean Droplet (Ubuntu, Docker)

1. Create a droplet (Ubuntu LTS).
2. SSH in and install Docker:

```bash
sudo apt-get update
sudo apt-get install -y docker.io docker-compose-plugin
sudo usermod -aG docker $USER
newgrp docker
```

3. Clone and build:

```bash
git clone <your-repo-url>
cd aethervault
mkdir -p data

docker build -t aethervault .
```

4. Run the service:

```bash
docker run -d --restart unless-stopped \
  --name aethervault \
  -v "$(pwd)/data:/data" \
  aethervault mcp /data/knowledge.mv2
```

5. (Optional) Use systemd to manage the container if you prefer.

## Kubernetes / other cloud

Use the Docker image and mount a persistent volume to `/data`. The CLI is stateless; the `.mv2` capsule is the only state you need to persist.

## Production tuning

- Build/run in `--release` (Docker already does this).
- Keep the capsule on SSD-backed storage.
- Use `aethervault compact` during low-traffic windows to keep indexes tight.
- For audit-grade logs set `--log-commit-interval 1` (or `agent.log_commit_interval=1` in config).
- For higher throughput set `--log-commit-interval 8` or higher.

## Chat connectors

Rust-native Telegram + WhatsApp bridges are built in. See `docs/CONNECTORS.md` for full setup.

Minimal Docker example (Telegram):

```bash
docker run --rm -it \
  -e TELEGRAM_BOT_TOKEN=123456:ABC \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e ANTHROPIC_MODEL=claude-<model> \
  -v "$(pwd)/data:/data" \
  aethervault bridge telegram --mv2 /data/knowledge.mv2
```

Minimal Docker example (WhatsApp + Twilio):

```bash
docker run --rm -it -p 8080:8080 \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e ANTHROPIC_MODEL=claude-<model> \
  -v "$(pwd)/data:/data" \
  aethervault bridge whatsapp --mv2 /data/knowledge.mv2 --bind 0.0.0.0 --port 8080
```
