#!/usr/bin/env python3
from pathlib import Path


def ensure_after(path: str, anchor: str, addition: str) -> None:
    p = Path(path)
    text = p.read_text()
    if addition in text:
        return
    if anchor not in text:
        raise RuntimeError(f"missing expected anchor in {path}")
    p.write_text(text.replace(anchor, anchor + addition, 1))


ensure_after(
    "/root/.openclaw/workspace/AGENTS.md",
    '- In group channels and shared Slack threads, never reveal internal hostnames, machine names, repo paths, branch names, commit hashes, worker labels, tool/vendor names, ports, tokens, or detailed infrastructure topology. Describe outcomes in neutral terms like "I checked the codebase", "I prepared a fix locally", or "I can open a PR once access is ready."\n',
    '- On Slack, only respond when the triggering sender is Sunil Rao (`sunil@tribble.ai`, Slack user `U0528KFHAE8`). If the sender is anyone else or identity is ambiguous, stay silent unless Sunil explicitly directs a response.\n'
    '- In shared Slack threads, do not narrate every step. Use milestone updates only: one short acknowledgement if useful, one blocker update if genuinely stuck, and one final evidence-backed summary.\n'
    '- In shared Slack threads, never say a fix was "tested" unless a real local stack, browser flow, or staging validation actually ran. Use precise labels like "reviewed", "build passed", "typecheck passed", "locally tested", or "staging-tested".\n',
)

ensure_after(
    "/root/.openclaw/workspace/TOOLS.md",
    "- In shared/group channels, keep status updates implementation-opaque: no hostnames, local paths, branch names, commit hashes, model/tool names, worker labels, or infrastructure details unless Sunil explicitly asks in a private context.\n",
    "- On Slack, treat `sunil@tribble.ai` / `U0528KFHAE8` as the only user Linus should answer by default. If a message is from someone else, do not reply unless Sunil explicitly asked Linus to engage.\n"
    '- In shared/group channels, do not emit running commentary like "let me check" or "now I will". Prefer milestone summaries with concrete evidence.\n'
    "- In shared/group channels, reserve the word `tested` for real end-to-end validation. If only code review or compilation happened, say that plainly.\n",
)

ensure_after(
    "/root/.openclaw/workspace/skills/ds9-triage/SKILL.md",
    "1. **Detect**: New message in #engineering-triage channel\n",
    "   - Only continue if the sender is Sunil Rao (`sunil@tribble.ai`, Slack user `U0528KFHAE8`). Otherwise do nothing.\n",
)

ensure_after(
    "/root/.openclaw/workspace/skills/ds9-triage/SKILL.md",
    '- summarize implementation work as "checked the codebase", "prepared a fix locally", or "ready to open a PR"\n',
    "- do not post stream-of-consciousness progress messages; use one acknowledgement, optional blocker, and one final summary\n"
    "- use precise evidence labels: `reviewed`, `build passed`, `typecheck passed`, `locally tested`, `staging-tested`\n",
)

ensure_after(
    "/root/.openclaw/workspace/skills/ds9-triage/SKILL.md",
    "```\n"
    "🔧 **Triage Analysis**\n\n",
    "**Status**: {reviewed|build passed|typecheck passed|locally tested|staging-tested}\n\n",
)

ensure_after(
    "/root/.openclaw/workspace/skills/ds9-triage/triage-handler.md",
    "thread_ts = message.get(\"ts\")  # For threading reply\n",
    "sender_email = message.get(\"user_email\") or message.get(\"email\") or \"\"\n",
)

ensure_after(
    "/root/.openclaw/workspace/skills/ds9-triage/triage-handler.md",
    "## Step 2: Analyze Screenshots (if present)\n\n",
    "## Step 1.5: Enforce Sender Gate\n\n"
    "Only proceed if the Slack sender is Sunil Rao. Accept any of these as identity matches:\n"
    "- `sender == \"U0528KFHAE8\"`\n"
    "- `sender_email == \"sunil@tribble.ai\"`\n"
    "- message metadata clearly identifies Sunil Rao\n\n"
    "If the sender is anyone else or identity is ambiguous, do not reply in the thread.\n\n",
)

ensure_after(
    "/root/.openclaw/workspace/skills/ds9-triage/triage-handler.md",
    "Before replying in a shared Slack thread, scrub internal implementation details. Do **not** include machine names, repo paths, branch names, commit hashes, ports, worker/tool names, or deployment mechanics unless Sunil explicitly asked in a private/operator context.\n\n",
    "Do not drip-feed step-by-step narration into the thread. Use at most:\n"
    "- one short acknowledgement when work starts\n"
    "- one blocker update if the work is genuinely stuck or waiting on access\n"
    "- one final summary with precise evidence labels (`reviewed`, `build passed`, `typecheck passed`, `locally tested`, `staging-tested`)\n\n",
)

print("patched")
