# Contributing to AetherVault

## Development Setup

1. Clone the repository and create a virtual environment:
   ```bash
   git clone <repo-url> aethervault
   cd aethervault
   python3 -m venv .venv
   source .venv/bin/activate
   ```

2. Install dependencies:
   ```bash
   pip install -r requirements-core.txt
   ```

3. Copy the environment template and fill in your keys:
   ```bash
   cp config/env.example ~/.aethervault/.env
   ```

4. For the Django superclustered app (separate):
   ```bash
   pip install -r requirements.txt
   ```

## Code Style

- **Python**: Follow the patterns established in existing scripts. Use `os.environ.get()`
  with sensible defaults for all configuration. Use `urllib.request` from the standard
  library for HTTP calls in core scripts (proxy servers, lifecycle scripts). The `requests`
  library is only used in the battle test runner.
- **Shell scripts**: Use `set -e` at the top. Quote variables. Use functions for reusable
  logic.
- **Configuration**: All tunables should be environment variables with defaults. Never
  hardcode API keys or secrets.

## Pull Request Process

1. Create a feature branch from the main branch.
2. Keep changes focused -- one feature or fix per PR.
3. Test your changes locally. For proxy servers, verify they start and respond to
   requests. For lifecycle scripts, use `--dry-run` where available.
4. Update `config/env.example` if you add new environment variables.
5. Open a pull request with a clear description of what changed and why.

## Reporting Issues

Open a GitHub issue with:
- What you expected to happen
- What actually happened
- Steps to reproduce
- Relevant log output (redact any API keys or tokens)
