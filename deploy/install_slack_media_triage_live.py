#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path


ROOT = Path("/root/.openclaw/workspace")


def ensure_after_prefix(path: Path, anchor_prefix: str, addition: str) -> None:
    text = path.read_text()
    if addition in text:
        return
    lines = text.splitlines(keepends=True)
    for idx, line in enumerate(lines):
        if line.startswith(anchor_prefix):
            lines.insert(idx + 1, addition)
            path.write_text("".join(lines))
            return
    raise RuntimeError(f"missing anchor prefix in {path}: {anchor_prefix}")


def main() -> int:
    agents = ROOT / "AGENTS.md"
    agents_text = agents.read_text()
    if "slack-media-triage" not in agents_text and "Slack thread includes MP4s" not in agents_text:
        ensure_after_prefix(
            agents,
            "- Anyone in the company Slack workspace may speak to Linus",
            "- If Sunil directly @mentions Linus in a shared Slack channel asking about screenshots, videos, MP4s, or audio notes, route through the slack-media-triage skill instead of answering from memory or staying silent.\n",
        )
    tools = ROOT / "TOOLS.md"
    tools_text = tools.read_text().replace(
        "- For Slack screenshots, videos, MP4s, and audio notes, build a packet with `/root/.openclaw/workspace/skills/slack-media-triage/scripts/build_slack_media_packet.py` before reasoning or replying.\n",
        "",
    )
    tools.write_text(tools_text)
    ensure_after_prefix(
        tools,
        "- Anyone in the company Slack workspace may speak to Linus",
        "- For Slack screenshots, videos, MP4s, and audio notes, build a packet with `/root/.openclaw/workspace/skills/slack-media-analysis/scripts/build_slack_media_packet.py` before reasoning or replying.\n",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
