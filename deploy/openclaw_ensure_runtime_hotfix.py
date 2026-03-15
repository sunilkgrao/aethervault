#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path


DIST = Path("/usr/lib/node_modules/openclaw/dist")


HELPER_VALIDATE_OLD = """function validateAnthropicTurns(messages) {
\treturn validateTurnsWithConsecutiveMerge({
\t\tmessages: stripDanglingAnthropicToolUses(messages),
\t\trole: "user",
\t\tmerge: mergeConsecutiveUserTurns
\t});
}"""


HELPER_VALIDATE_NEW = """function validateAnthropicTurns(messages) {
\tconst filteredMessages = Array.isArray(messages) ? messages.filter((msg) => {
\t\tif (!msg || typeof msg !== "object") return true;
\t\tif (msg.role !== "assistant") return true;
\t\treturn msg.stopReason !== "error" && msg.stopReason !== "aborted";
\t}) : messages;
\treturn validateTurnsWithConsecutiveMerge({
\t\tmessages: stripDanglingAnthropicToolUses(filteredMessages),
\t\trole: "user",
\t\tmerge: mergeConsecutiveUserTurns
\t});
}"""


ANTHROPIC_TOOL_ID_OLD = (
    "const sanitizeToolCallIds = isGoogle || isMistral || isAnthropic || "
    "requiresOpenAiCompatibleToolIdSanitization;"
)
ANTHROPIC_TOOL_ID_NEW = (
    "const sanitizeToolCallIds = isGoogle || isMistral || "
    "requiresOpenAiCompatibleToolIdSanitization;"
)


STALE_THRESHOLD_OLD = "const DEFAULT_CHANNEL_STALE_EVENT_THRESHOLD_MS = 30 * 6e4;"
STALE_THRESHOLD_NEW = "const DEFAULT_CHANNEL_STALE_EVENT_THRESHOLD_MS = 120 * 6e4;"
STALE_INLINE_OLD = (
    "staleEventThresholdMs: deps.timing?.staleEventThresholdMs ?? "
    "deps.staleEventThresholdMs ?? 18e5"
)
STALE_INLINE_NEW = (
    "staleEventThresholdMs: deps.timing?.staleEventThresholdMs ?? "
    "deps.staleEventThresholdMs ?? 72e5"
)


BROWSER_USER_DATA_OLD = 'return path.join(CONFIG_DIR, "browser", profileName, "user-data");'
BROWSER_USER_DATA_NEW = 'return path.join("/tmp", "openclaw-browser", profileName, "user-data");'


def patch_file(path: Path) -> bool:
    original = path.read_text()
    updated = original
    updated = updated.replace(HELPER_VALIDATE_OLD, HELPER_VALIDATE_NEW)
    updated = updated.replace(ANTHROPIC_TOOL_ID_OLD, ANTHROPIC_TOOL_ID_NEW)
    updated = updated.replace(STALE_THRESHOLD_OLD, STALE_THRESHOLD_NEW)
    updated = updated.replace(STALE_INLINE_OLD, STALE_INLINE_NEW)
    updated = updated.replace(BROWSER_USER_DATA_OLD, BROWSER_USER_DATA_NEW)
    if updated == original:
        return False
    path.write_text(updated)
    return True


def iter_targets() -> list[Path]:
    targets: list[Path] = []
    targets.extend(sorted(DIST.glob("pi-embedded-helpers-*.js")))
    targets.extend(sorted(DIST.glob("pi-embedded-*.js")))
    targets.extend(sorted(DIST.glob("compact-*.js")))
    targets.extend(sorted(DIST.glob("gateway-cli-*.js")))
    targets.extend(sorted(DIST.glob("chrome-*.js")))
    targets.extend(sorted(DIST.glob("server-context-*.js")))
    targets.extend(sorted((DIST / "plugin-sdk").glob("chrome-*.js")))
    targets.extend(sorted((DIST / "plugin-sdk").glob("pi-embedded-helpers-*.js")))
    return [path for path in targets if path.is_file()]


def main() -> int:
    patched = 0
    for path in iter_targets():
        if patch_file(path):
            patched += 1
    print(f"patched={patched}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
