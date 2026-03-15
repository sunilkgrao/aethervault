#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re


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

SLACK_APP_MENTION_SKIP_OLD = (
    'if (channelType === "im" || channelType === "mpim") return;'
)
SLACK_APP_MENTION_SKIP_NEW = (
    'if (channelType === "im") return;'
)

SLACK_MESSAGE_HANDLER_RE = re.compile(
    r"""const handleIncomingMessageEvent = async \(\{ event, body \}\) => \{\n"""
    r"""\t\ttry \{\n"""
    r"""\t\t\tif \(ctx\.shouldDropMismatchedSlackEvent\(body\)\) return;\n"""
    r"""\t\t\t(?:const|let) message = event;\n"""
    r"""(?:.|\n)*?"""
    r"""\t\t\tconst subtypeHandler = resolveSlackMessageSubtypeHandler\(message\);\n"""
    r"""\t\t\tif \(subtypeHandler\) \{"""
)

SLACK_MESSAGE_POLLER_RE = re.compile(
    r"""(ctx\.app\.event\("app_mention", async \(\{ event, body \}\) => \{\n"""
    r"""(?:.|\n)*?"""
    r"""\t\}\);\n)"""
    r"""\}"""
)

SLACK_MESSAGE_HANDLER_NEW = """const handleIncomingMessageEvent = async ({ event, body }) => {
\t\ttry {
\t\t\tif (ctx.shouldDropMismatchedSlackEvent(body)) return;
\t\t\tlet message = event;
\t\t\tif ((!message.user || !message.text) && message.message && typeof message.message === "object" && message.channel) {
\t\t\t\tconst nested = message.message;
\t\t\t\tconst rootThreadTs = typeof nested.thread_ts === "string" && nested.thread_ts ? nested.thread_ts : typeof message.thread_ts === "string" && message.thread_ts ? message.thread_ts : void 0;
\t\t\t\tconst nestedTs = typeof nested.ts === "string" && nested.ts ? nested.ts : void 0;
\t\t\t\tconst latestReplyTs = typeof nested.latest_reply === "string" && nested.latest_reply ? nested.latest_reply : Array.isArray(nested.replies) ? nested.replies.at(-1)?.ts : void 0;
\t\t\t\tif (nested.user && nested.text && nestedTs && (!rootThreadTs || nestedTs !== rootThreadTs)) {
\t\t\t\t\tmessage = {
\t\t\t\t\t\t...nested,
\t\t\t\t\t\tchannel: message.channel
\t\t\t\t\t};
\t\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack inbound: normalized direct nested reply channel=${message.channel} thread_ts=${rootThreadTs ?? "unknown"} reply_ts=${nestedTs}`);
\t\t\t\t} else if (rootThreadTs && latestReplyTs && latestReplyTs !== rootThreadTs) try {
\t\t\t\t\tconst replyResponse = await ctx.app.client.conversations.replies({
\t\t\t\t\t\tchannel: message.channel,
\t\t\t\t\t\tts: rootThreadTs,
\t\t\t\t\t\tlatest: latestReplyTs,
\t\t\t\t\t\toldest: latestReplyTs,
\t\t\t\t\t\tinclusive: true,
\t\t\t\t\t\tlimit: 1
\t\t\t\t\t});
\t\t\t\t\tconst resolvedReply = replyResponse.messages?.find((entry) => entry.ts === latestReplyTs) ?? replyResponse.messages?.at(-1);
\t\t\t\t\tif (resolvedReply && typeof resolvedReply === "object") {
\t\t\t\t\t\tmessage = {
\t\t\t\t\t\t\t...resolvedReply,
\t\t\t\t\t\t\tchannel: message.channel
\t\t\t\t\t\t};
\t\t\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack inbound: normalized threaded reply channel=${message.channel} thread_ts=${rootThreadTs} reply_ts=${latestReplyTs}`);
\t\t\t\t\t}
\t\t\t\t} catch (err) {
\t\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack inbound: failed to normalize threaded reply channel=${message.channel} thread_ts=${rootThreadTs} reply_ts=${latestReplyTs}: ${String(err)}`);
\t\t\t\t}
\t\t\t}
\t\t\tconst subtypeHandler = resolveSlackMessageSubtypeHandler(message);
\t\t\tif (subtypeHandler) {"""

SLACK_MESSAGE_POLLER_NEW = """\\1\tconst threadMentionPollSeen = new Map();
\tlet threadMentionPollInFlight = false;
\tconst threadMentionPoll = async () => {
\t\tif (threadMentionPollInFlight) return;
\t\tthreadMentionPollInFlight = true;
\t\ttry {
\t\t\tconst botUserId = ctx.botUserId;
\t\t\tif (!botUserId) return;
\t\t\tconst nowSeconds = Date.now() / 1e3;
\t\t\tconst configuredChannelIds = (ctx.channelsConfigKeys ?? []).filter((id) => id && id !== "*" && (id.startsWith("C") || id.startsWith("G")));
\t\t\tconst persistedChannelIds = [];
\t\t\ttry {
\t\t\t\tconst fs = await import("node:fs/promises");
\t\t\t\tconst rawSessions = await fs.readFile("/root/.openclaw/agents/main/sessions/sessions.json", "utf8");
\t\t\t\tconst parsedSessions = JSON.parse(rawSessions);
\t\t\t\tfor (const meta of Object.values(parsedSessions?.sessions ?? {})) {
\t\t\t\t\tfor (const value of [meta?.lastTo, meta?.lastRoute?.to, meta?.route?.to]) {
\t\t\t\t\t\tif (typeof value === "string" && value.startsWith("channel:")) {
\t\t\t\t\t\t\tconst channelId = value.slice(8);
\t\t\t\t\t\t\tif (channelId) persistedChannelIds.push(channelId);
\t\t\t\t\t\t}
\t\t\t\t\t}
\t\t\t\t}
\t\t\t} catch (err) {
\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack thread poll failed to read session channels: ${String(err)}`);
\t\t\t}
\t\t\tconst discoveredChannelIds = [];
\t\t\ttry {
\t\t\t\tlet cursor = void 0;
\t\t\t\tfor (let i = 0; i < 10; i += 1) {
\t\t\t\t\tconst listed = await ctx.app.client.conversations.list({
\t\t\t\t\t\ttypes: "public_channel,private_channel",
\t\t\t\t\t\texclude_archived: true,
\t\t\t\t\t\tlimit: 200,
\t\t\t\t\t\t...cursor ? { cursor } : {}
\t\t\t\t\t});
\t\t\t\t\tfor (const channel of listed.channels ?? []) {
\t\t\t\t\t\tif (channel?.is_member && typeof channel.id === "string" && channel.id) discoveredChannelIds.push(channel.id);
\t\t\t\t\t}
\t\t\t\t\tcursor = listed.response_metadata?.next_cursor || void 0;
\t\t\t\t\tif (!cursor) break;
\t\t\t\t}
\t\t\t} catch (err) {
\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack thread poll failed to list channels: ${String(err)}`);
\t\t\t}
\t\t\tconst channelIds = [...new Set([...configuredChannelIds, ...persistedChannelIds, ...discoveredChannelIds])];
\t\t\tfor (const channelId of channelIds) try {
\t\t\t\tconst history = await ctx.app.client.conversations.history({
\t\t\t\t\tchannel: channelId,
\t\t\t\t\tlimit: 20
\t\t\t\t});
\t\t\t\tfor (const root of history.messages ?? []) {
\t\t\t\t\tconst rootTs = typeof root?.ts === "string" && root.ts ? root.ts : void 0;
\t\t\t\t\tconst latestReplyTs = typeof root?.latest_reply === "string" && root.latest_reply ? root.latest_reply : void 0;
\t\t\t\t\tif (!rootTs || !latestReplyTs || latestReplyTs === rootTs) continue;
\t\t\t\t\tif (Number(latestReplyTs) < nowSeconds - 900) continue;
\t\t\t\t\tconst seenKey = `${channelId}:${rootTs}`;
\t\t\t\t\tif (threadMentionPollSeen.get(seenKey) === latestReplyTs) continue;
\t\t\t\t\tthreadMentionPollSeen.set(seenKey, latestReplyTs);
\t\t\t\t\tconst replies = await ctx.app.client.conversations.replies({
\t\t\t\t\t\tchannel: channelId,
\t\t\t\t\t\tts: rootTs,
\t\t\t\t\t\tlatest: latestReplyTs,
\t\t\t\t\t\toldest: latestReplyTs,
\t\t\t\t\t\tinclusive: true,
\t\t\t\t\t\tlimit: 1
\t\t\t\t\t});
\t\t\t\t\tconst reply = replies.messages?.find((entry) => entry.ts === latestReplyTs) ?? replies.messages?.at(-1);
\t\t\t\t\tif (!reply || typeof reply !== "object") continue;
\t\t\t\t\tif (reply.user === botUserId || reply.bot_id) continue;
\t\t\t\t\tconst replyText = typeof reply.text === "string" ? reply.text : "";
\t\t\t\t\tif (!replyText.includes(`<@${botUserId}>`)) continue;
\t\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack thread poll: recovered mention channel=${channelId} thread_ts=${rootTs} reply_ts=${latestReplyTs}`);
\t\t\t\t\tawait handleSlackMessage({
\t\t\t\t\t\t...reply,
\t\t\t\t\t\tchannel: channelId
\t\t\t\t\t}, {
\t\t\t\t\t\tsource: "message",
\t\t\t\t\t\twasMentioned: true
\t\t\t\t\t});
\t\t\t\t}
\t\t\t} catch (err) {
\t\t\t\tif (shouldLogVerbose()) logVerbose(`slack thread poll failed channel=${channelId}: ${String(err)}`);
\t\t\t}
\t\t} finally {
\t\t\tthreadMentionPollInFlight = false;
\t\t}
\t};
\tconst threadMentionPollTimer = setInterval(() => {
\t\tthreadMentionPoll().catch(() => void 0);
\t}, 15000);
\tif (typeof threadMentionPollTimer?.unref === "function") threadMentionPollTimer.unref();
\tctx.runtime.log?.("slack thread mention poll armed");
\tthreadMentionPoll().catch(() => void 0);
}"""


def patch_file(path: Path) -> bool:
    original = path.read_text()
    updated = original
    updated = updated.replace(HELPER_VALIDATE_OLD, HELPER_VALIDATE_NEW)
    updated = updated.replace(ANTHROPIC_TOOL_ID_OLD, ANTHROPIC_TOOL_ID_NEW)
    updated = updated.replace(STALE_THRESHOLD_OLD, STALE_THRESHOLD_NEW)
    updated = updated.replace(STALE_INLINE_OLD, STALE_INLINE_NEW)
    updated = updated.replace(BROWSER_USER_DATA_OLD, BROWSER_USER_DATA_NEW)
    updated = updated.replace(SLACK_APP_MENTION_SKIP_OLD, SLACK_APP_MENTION_SKIP_NEW)
    updated = SLACK_MESSAGE_HANDLER_RE.sub(SLACK_MESSAGE_HANDLER_NEW, updated, count=1)
    updated = SLACK_MESSAGE_POLLER_RE.sub(SLACK_MESSAGE_POLLER_NEW, updated, count=1)
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
    targets.extend(sorted(DIST.glob("discord-*.js")))
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
