# AetherVault Operations — Infrastructure State

> This file is versioned with the code and deployed automatically via upgrade.sh.
> Last deploy: see `git log -1 --format=%H` on the droplet.

## Subagent Pool Routing (deployed 2026-02-22)

The binary handles all subagent routing natively in Rust (`src/pool_state.rs`).
No Python subprocess needed for backend selection.

### Backends (priority order):
1. **Codex CLI** — `codex exec -m gpt-5.3-codex-spark --json --skip-git-repo-check`
2. **Claude Code CLI** — `claude -p "prompt" --output-format json`

### Accounts:
| Account | Service | Auth Location | Email |
|---------|---------|--------------|-------|
| codex-primary | codex | /root/.codex/ | sunil@tribble.ai |
| codex-secondary | codex | /root/codex-alt/.codex/ | sunilkgrao@gmail.com |
| claude-code-max | claude-code | /root/.claude/ | (Max plan) |

### How routing works:
- `builtin:pool` (default) — tries codex-primary, then codex-secondary, then claude-code
- `builtin:codex` — codex only (skips claude-code)
- `builtin:claude-code` — claude-code only (skips codex)
- On rate limit: account is cooled down (codex: 300s, claude-code: 120s), next account tried
- Config: `/root/.aethervault/config/auth-profiles.json`
- The Rust binary sets HOME to the parent of each account's config dir (codex reads $HOME/.codex/)

### Subagent specs (assistant/config.json):
- `researcher` — read-only, builtin:pool
- `coder` — full access, builtin:pool
- `coder-codex` — full access, builtin:codex (explicit Codex)
- `coder-claude` — full access, builtin:claude-code (explicit Claude Code)
- `critic` — read-only, builtin:pool

### Rules:
- Codex is ALWAYS invoked via CLI, NEVER via API
- Model: ALWAYS gpt-5.3-codex-spark
- The binary handles rate limit detection and failover automatically
- Do NOT manually run `codex auth login` — accounts are pre-authenticated
- Do NOT tell the user to re-authenticate — the pool handles it

## Deployment

- Droplet: clawdbot (167.172.140.221)
- Binary: blue-green at /opt/aethervault/{blue,green}/
- Deploy: `cd /root/aethervault && git pull && bash deploy/upgrade.sh`
- Service: `systemctl restart aethervault`
- Self-improve: every 6h via systemd timer

## Hooks Architecture (consolidated 2026-02-22)

- `hooks/common.py` — shared utilities (send_telegram, load_env, call_claude, logging)
- All hook files import from common.py (no duplication)
- Python hooks remain as fallback for external hook invocation
- Main code path uses Rust-native builtin: hooks
