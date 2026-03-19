# Contributing to Linus Legacy Migration Utility

## Development Setup

Before making changes, read:
- `README.md`
- `docs/SOURCE-OF-TRUTH.md`
- `AGENTS.md`

1. Clone the repository:
   ```bash
   git clone <repo-url> clawdbot
   cd clawdbot
   ```

2. Install dependencies:
   ```bash
   cargo build --locked
   ```

## Code Style

- Keep diffs targeted and follow existing patterns.
- Prefer updating the authoritative docs when architecture or workflow assumptions change.
- Do not add new runtime ambitions to this repo. The long-term runtime target is upstream OpenClaw.
- Do not create a second state or memory doctrine in scattered markdown files.

## Pull Request Process

1. Create a feature branch from the main branch.
2. Keep changes focused: one feature, bug fix, or documentation correction per PR.
3. Run the relevant checks locally:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets --all-features
   cargo test
   ```
4. If you changed architecture, migration flow, state semantics, or operator guardrails, update the relevant authoritative docs in the same change.
5. Open a pull request with a clear description of what changed, why it changed, and what was validated.

## Reporting Issues

Open a GitHub issue with:
- What you expected to happen
- What actually happened
- Steps to reproduce
- Relevant log output or artifacts, with secrets redacted
