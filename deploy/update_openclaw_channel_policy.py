#!/usr/bin/env python3
from __future__ import annotations

from datetime import datetime
from pathlib import Path


ROOT = Path("/root/.openclaw/workspace")


def replace_once(text: str, old: str, new: str) -> str:
    if new in text:
        return text
    if old not in text:
        raise RuntimeError(f"expected text not found: {old[:80]!r}")
    return text.replace(old, new, 1)


def main() -> int:
    targets = [
        ROOT / "SOUL.md",
        ROOT / "AGENTS.md",
        ROOT / "TOOLS.md",
        ROOT / "STATE.md",
        ROOT / "STATE.json",
        ROOT / "skills" / "ds9-triage" / "SKILL.md",
        ROOT / "skills" / "ds9-triage" / "triage-handler.md",
    ]

    stamp = datetime.utcnow().strftime("%Y%m%d-%H%M%S")
    backup_dir = Path(f"/root/backups/openclaw-channel-policy-{stamp}")
    backup_dir.mkdir(parents=True, exist_ok=True)

    for path in targets:
        backup_path = backup_dir / path.name
        backup_path.write_text(path.read_text())

    soul_path = ROOT / "SOUL.md"
    soul_text = soul_path.read_text()
    soul_text = replace_once(
        soul_text,
        "- You're not the user's voice — be careful in group chats.\n"
        "- **NEVER post infrastructure details externally** — no real IPs, hostnames, ports, paths, or credentials to ANY public platform. Use placeholders. Read `SECURITY_PROTOCOL.md` before external posts. This is non-negotiable.\n",
        "- You're not the user's voice — be careful in group chats.\n"
        "- **NEVER post infrastructure details externally** — no real IPs, hostnames, ports, paths, or credentials to ANY public platform. Use placeholders. Read `SECURITY_PROTOCOL.md` before external posts. This is non-negotiable.\n"
        "- In shared Slack channels, group chats, or any audience beyond Sunil, never mention internal machine names, repo locations, branches, commit hashes, worker names, model names, tool names, session ids, local paths, or deployment mechanics unless Sunil explicitly asks for that detail in a private/operator context.\n"
        "- In DMs with Sunil, be transparent about internals when it is useful. In group contexts, summarize outcomes and next steps without exposing implementation details.\n",
    )
    soul_path.write_text(soul_text)

    agents_path = ROOT / "AGENTS.md"
    agents_text = agents_path.read_text()
    agents_text = replace_once(
        agents_text,
        "## Channel Behavior\n"
        "- In direct chats, optimize for follow-through and clarity\n"
        "- In group channels, speak only when directly asked or when there is clear incremental value\n"
        "- Use reactions when lightweight acknowledgement is enough\n",
        "## Channel Behavior\n"
        "- In direct chats, optimize for follow-through, clarity, and transparent operational detail when useful to Sunil.\n"
        "- In group channels, speak only when directly asked or when there is clear incremental value.\n"
        "- In group channels and shared Slack threads, never reveal internal hostnames, machine names, repo paths, branch names, commit hashes, worker labels, tool/vendor names, ports, tokens, or detailed infrastructure topology. Describe outcomes in neutral terms like \"I checked the codebase\", \"I prepared a fix locally\", or \"I can open a PR once access is ready.\"\n"
        "- In shared Slack threads, do not narrate every step. Use milestone updates only: optional brief acknowledgement, one blocker update if genuinely stuck, and one final evidence-backed summary.\n"
        "- In shared Slack threads, never say a fix was \"tested\" unless a real local stack, browser flow, or staging validation actually ran. Use precise labels like \"reviewed\", \"build passed\", \"typecheck passed\", \"locally tested\", or \"staging-tested\".\n"
        "- Treat Slack channels as external surfaces unless the conversation is clearly a private operator DM with Sunil.\n"
        "- Use reactions when lightweight acknowledgement is enough.\n",
    )
    agents_path.write_text(agents_text)

    tools_path = ROOT / "TOOLS.md"
    tools_text = tools_path.read_text()
    tools_text = replace_once(
        tools_text,
        "## Messaging Tool Rules\n"
        "- When sending media, QR codes, or images through Telegram or WhatsApp, always include a short non-empty message/caption. Do not attempt an empty media-only send.\n"
        "- For QR or setup messages, use a caption like: `Scan this QR code to link WhatsApp.`\n"
        "- If a delivery tool errors, stop retrying the same malformed payload and switch to a valid captioned send or a plain-text fallback.\n",
        "## Messaging Tool Rules\n"
        "- When sending media, QR codes, or images through Telegram or WhatsApp, always include a short non-empty message/caption. Do not attempt an empty media-only send.\n"
        "- For QR or setup messages, use a caption like: `Scan this QR code to link WhatsApp.`\n"
        "- If a delivery tool errors, stop retrying the same malformed payload and switch to a valid captioned send or a plain-text fallback.\n"
        "- In shared/group channels, keep status updates implementation-opaque: no hostnames, local paths, branch names, commit hashes, model/tool names, worker labels, or infrastructure details unless Sunil explicitly asks in a private context.\n"
        "- In shared/group channels, do not emit running commentary like \"let me check\" or \"now I will\". Prefer milestone summaries with concrete evidence.\n"
        "- In shared/group channels, reserve the word `tested` for real end-to-end validation. If only code review or compilation happened, say that plainly.\n",
    )
    tools_path.write_text(tools_text)

    skill_path = ROOT / "skills" / "ds9-triage" / "SKILL.md"
    skill_text = skill_path.read_text()
    skill_text = replace_once(
        skill_text,
        "## DS9 Codebase Location\n\n"
        "- **Path**: `~/ds9` on raoDesktop (ssh -p 2222 sunil@localhost)\n"
        "- **Type**: Turbo monorepo (TypeScript + Python)\n",
        "## DS9 Codebase Location\n\n"
        "- Internal working copy exists on Linus's development environment. Do not mention machine names or paths in shared Slack channels unless Sunil explicitly asks in a private/operator context.\n"
        "- **Type**: Turbo monorepo (TypeScript + Python)\n",
    )
    skill_text = replace_once(
        skill_text,
        "## Response Format\n\n"
        "Post solution as a Slack thread reply:\n\n"
        "```\n"
        "🔧 **Triage Analysis**\n\n"
        "**Issue**: {summary}\n\n"
        "**Root Cause**: {diagnosis}\n\n"
        "**Proposed Fix**:\n"
        "{code_changes}\n\n"
        "**Files to modify**:\n"
        "- path/to/file1.ts\n"
        "- path/to/file2.py\n\n"
        "**Testing**: {test_recommendations}\n\n"
        "---\n"
        "_Auto-generated by Linus via Codex. Review before implementing._\n"
        "```\n",
        "## Response Format\n\n"
        "Post solution as a Slack thread reply. In shared channels, keep it group-safe:\n"
        "- never mention machine names, repo paths, branch names, commit hashes, ports, or tool/vendor names\n"
        "- summarize implementation work as \"checked the codebase\", \"prepared a fix locally\", or \"ready to open a PR\"\n"
        "- do not post stream-of-consciousness progress messages; use one acknowledgement, optional blocker, and one final summary\n"
        "- use precise evidence labels: `reviewed`, `build passed`, `typecheck passed`, `locally tested`, `staging-tested`\n"
        "- reserve concrete infra details for private/operator DMs with Sunil\n\n"
        "```\n"
        "🔧 **Triage Analysis**\n\n"
        "**Status**: {reviewed|build passed|typecheck passed|locally tested|staging-tested}\n\n"
        "**Issue**: {summary}\n\n"
        "**Root Cause**: {diagnosis}\n\n"
        "**Proposed Fix**:\n"
        "{code_changes}\n\n"
        "**Files to modify**:\n"
        "- relevant file A\n"
        "- relevant file B\n\n"
        "**Testing**: {test_recommendations}\n\n"
        "---\n"
        "_Auto-generated by Linus. Review before implementing._\n"
        "```\n",
    )
    skill_path.write_text(skill_text)

    handler_path = ROOT / "skills" / "ds9-triage" / "triage-handler.md"
    handler_text = handler_path.read_text()
    handler_text = replace_once(
        handler_text,
        "## Step 4: Post Solution to Slack\n\n"
        "Use `message` tool:\n"
        "```\n"
        "message(\n"
        "  action=\"send\",\n"
        "  channel=\"slack\",\n"
        "  target=\"#engineering-triage\",\n"
        "  threadId=thread_ts,\n"
        "  message=formatted_solution\n"
        ")\n"
        "```\n",
        "## Step 4: Post Solution to Slack\n\n"
        "Before replying in a shared Slack thread, scrub internal implementation details. Do **not** include machine names, repo paths, branch names, commit hashes, ports, worker/tool names, or deployment mechanics unless Sunil explicitly asked in a private/operator context.\n\n"
        "Do not drip-feed step-by-step narration into the thread. Use at most:\n"
        "- one short acknowledgement when work starts\n"
        "- one blocker update if the work is genuinely stuck or waiting on access\n"
        "- one final summary with precise evidence labels (`reviewed`, `build passed`, `typecheck passed`, `locally tested`, `staging-tested`)\n\n"
        "Use `message` tool:\n"
        "```\n"
        "message(\n"
        "  action=\"send\",\n"
        "  channel=\"slack\",\n"
        "  target=\"#engineering-triage\",\n"
        "  threadId=thread_ts,\n"
        "  message=formatted_solution\n"
        ")\n"
        "```\n",
    )
    handler_path.write_text(handler_text)

    state_md_path = ROOT / "STATE.md"
    state_md_text = state_md_path.read_text()
    state_md_text = state_md_text.replace(
        "- raoDesktop tunnel is a key capability for GPU work and remote code execution.\n",
        "- A remote GPU workstation is available for heavy compute and remote code execution.\n",
    )
    state_md_path.write_text(state_md_text)

    state_json_path = ROOT / "STATE.json"
    state_json_text = state_json_path.read_text()
    state_json_text = state_json_text.replace(
        "Direct Telegram interface to subLinus (Grok-4-fast on raoDesktop via OpenClaw)",
        "Direct Telegram interface to subLinus (fast secondary model via OpenClaw)",
    )
    state_json_text = state_json_text.replace(
        "Verified: getMe API returns ok, process healthy, allowed user ID 8280335652, Deepgram key set, SSH tunnel to raoDesktop up, openclaw-agent.sh exists.",
        "Verified: getMe API returns ok, process healthy, allowed user configured, transcription configured, remote workstation link healthy, agent wrapper present.",
    )
    state_json_path.write_text(state_json_text)

    print(backup_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
