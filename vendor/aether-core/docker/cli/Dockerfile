# Vault CLI Docker Image
FROM ubuntu:24.04

LABEL org.opencontainers.image.title="Vault CLI"
LABEL org.opencontainers.image.description="AI memory CLI with semantic search"
LABEL org.opencontainers.image.url="https://aethervault.ai"
LABEL org.opencontainers.image.source="https://github.com/vault/vault"
LABEL org.opencontainers.image.vendor="Vault"
LABEL org.opencontainers.image.version="2.0.131"

# Install system deps + Node.js 24
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    && curl -fsSL https://deb.nodesource.com/setup_24.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install vault CLI (pinned version)
RUN npm install -g vault-cli@latest \
    && npm cache clean --force

# Create non-root user (recommended)
RUN useradd -m vault
USER vault

# Working directory for memory files
WORKDIR /data

# Entrypoint
ENTRYPOINT ["vault"]
CMD ["--help"]
