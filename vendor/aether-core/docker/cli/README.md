# Vault CLI Docker Image

AI memory CLI with crash-safe, single-file storage and semantic search.

## Quick Start

```bash
# Pull the image
docker pull aethervault/cli

# Create a memory
docker run --rm -v $(pwd):/data aethervault/cli create my-memory.mv2

# Add documents
docker run --rm -v $(pwd):/data aethervault/cli put my-memory.mv2 --input doc.pdf

# Search
docker run --rm -v $(pwd):/data aethervault/cli find my-memory.mv2 --query "search"
```

## Basic Commands

```bash
# Show help
docker run --rm aethervault/cli

# Show version
docker run --rm aethervault/cli --version

# Create a memory file (mount local directory)
docker run --rm -v $(pwd):/data aethervault/cli create my-memory.mv2

# Ingest a document
docker run --rm -v $(pwd):/data aethervault/cli put my-memory.mv2 --input document.pdf

# Search the memory
docker run --rm -v $(pwd):/data aethervault/cli find my-memory.mv2 --query "search term"

# Ask questions (requires API key for LLM)
docker run --rm -v $(pwd):/data \
  -e OPENAI_API_KEY="sk-..." \
  aethervault/cli ask my-memory.mv2 "What is this about?" -m openai

# View stats
docker run --rm -v $(pwd):/data aethervault/cli stats my-memory.mv2
```

## With API Keys

```bash
# Pass Vault API key for cloud features
docker run --rm -v $(pwd):/data \
  -e AETHERVAULT_API_KEY="mv2_..." \
  -e OPENAI_API_KEY="sk-..." \
  aethervault/cli ask my-memory.mv2 "your question"
```

## Shell Alias (Recommended)

Add to `~/.bashrc` or `~/.zshrc`:

```bash
alias vault='docker run --rm -v $(pwd):/data -e AETHERVAULT_API_KEY -e OPENAI_API_KEY aethervault/cli'
```

Then use normally:

```bash
vault create my-memory.mv2
vault put my-memory.mv2 --input docs/
vault find my-memory.mv2 --query "hello"
```

## Docker Compose Example

```yaml
version: '3.8'
services:
  vault:
    image: aethervault/cli:latest
    volumes:
      - ./data:/data
    environment:
      - AETHERVAULT_API_KEY=${AETHERVAULT_API_KEY}
      - OPENAI_API_KEY=${OPENAI_API_KEY}
    entrypoint: ["vault"]
    command: ["stats", "my-memory.mv2"]
```

## Features

- Single-file `.mv2` storage
- Semantic + lexical search
- RAG question answering
- PDF, DOCX, images, audio support

## Links

- Website: https://aethervault.ai
- Docs: https://docs.aethervault.com
- GitHub: https://github.com/vault/vault
