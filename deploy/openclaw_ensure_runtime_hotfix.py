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

THREAD_MENTION_POLL_MARKED_RE = re.compile(
    r"""\t/\* openclaw-thread-mention-poll:start \*/\n"""
    r"""(?:.|\n)*?"""
    r"""\t/\* openclaw-thread-mention-poll:end \*/\n"""
)

THREAD_MENTION_POLL_LEGACY_RE = re.compile(
    r"""\tconst threadMentionPollSeen = new Map\(\);\n"""
    r"""(?:.|\n)*?"""
    r"""\tthreadMentionPoll\(\)\.catch\(.*?\);\n""",
    re.DOTALL,
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

SLACK_MESSAGE_POLLER_NEW = """\\1\t/* openclaw-thread-mention-poll:start */
\tconst threadMentionRecoveryMinAgeSeconds = 20;
\tconst threadMentionPollSeen = new Map();
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
\t\t\t\t\tif (Number(latestReplyTs) > nowSeconds - threadMentionRecoveryMinAgeSeconds) continue;
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
\t/* openclaw-thread-mention-poll:end */
}"""

FOLLOWUP_QUEUE_ACK_ANCHOR = """function resolveActiveRunQueueAction(params) {
\tif (!params.isActive) return "run-now";
\tif (params.isHeartbeat) return "drop";
\tif (params.shouldFollowup || params.queueMode === "steer") return "enqueue-followup";
\treturn "run-now";
}"""

FOLLOWUP_QUEUE_ACK_INSERT = """const RECENT_BUSY_QUEUE_ACKS = resolveGlobalSingleton(Symbol.for("openclaw.recentBusyQueueAcks"), () => createDedupeCache({
\tttlMs: 45 * 1e3,
\tmaxSize: 1e4
}));
async function maybeSendQueuedBusyAck(params) {
\tconst channel = resolveOriginMessageProvider({
\t\toriginatingChannel: params.followupRun.originatingChannel,
\t\tprovider: params.sessionCtx.Surface ?? params.sessionCtx.Provider
\t});
\tconst to = resolveOriginMessageTo({
\t\toriginatingTo: params.followupRun.originatingTo
\t});
\tconst accountId = resolveOriginAccountId({
\t\toriginatingAccountId: params.followupRun.originatingAccountId,
\t\taccountId: params.followupRun.run.agentAccountId
\t});
\tconst threadId = params.followupRun.originatingThreadId;
\tconst chatType = typeof params.sessionCtx.ChatType === "string" ? params.sessionCtx.ChatType.toLowerCase() : "";
\tconst isDirectish = chatType === "direct" || chatType === "dm" || chatType === "private" || channel === "telegram" || typeof to === "string" && to.startsWith("user:");
\tif (!channel || !to || !isDirectish) return;
\tconst ackKey = JSON.stringify([
\t\tparams.queueKey ?? params.sessionKey ?? "",
\t\tchannel,
\t\tto,
\t\taccountId ?? "",
\t\tthreadId == null ? "" : String(threadId)
\t]);
\tif (RECENT_BUSY_QUEUE_ACKS.check(ackKey)) return;
\tawait routeReply({
\t\tpayload: {
\t\t\ttext: "Still working on the current task. I queued your follow-up and will answer it next."
\t\t},
\t\tchannel,
\t\tto,
\t\taccountId,
\t\tthreadId,
\t\tcfg: params.cfg,
\t\tsessionKey: params.sessionKey,
\t\tmirror: false,
\t\tisGroup: false
\t});
}
function resolveActiveRunQueueAction(params) {
\tif (!params.isActive) return "run-now";
\tif (params.isHeartbeat) return "drop";
\tif (params.shouldFollowup || params.queueMode === "steer") return "enqueue-followup";
\treturn "run-now";
}"""

FOLLOWUP_QUEUE_BRANCH_OLD = """\tif (activeRunQueueAction === "enqueue-followup") {
\t\tenqueueFollowupRun(queueKey, followupRun, resolvedQueue);
\t\tawait touchActiveSessionEntry();
\t\ttyping.cleanup();
\t\treturn;
\t}"""

FOLLOWUP_QUEUE_BRANCH_NEW = """\tif (activeRunQueueAction === "enqueue-followup") {
\t\tconst enqueued = enqueueFollowupRun(queueKey, followupRun, resolvedQueue);
\t\tif (enqueued) await maybeSendQueuedBusyAck({
\t\t\tqueueKey,
\t\t\tfollowupRun,
\t\t\tsessionCtx,
\t\t\tcfg,
\t\t\tsessionKey
\t\t});
\t\tawait touchActiveSessionEntry();
\t\ttyping.cleanup();
\t\treturn;
\t}"""

ROUTE_REPLY_START_OLD = """async function routeReply(params) {
\tconst { payload, channel, to, accountId, threadId, cfg, abortSignal } = params;
\tif (shouldSuppressReasoningPayload(payload)) return { ok: true };
\tconst normalizedChannel = normalizeMessageChannel(channel);
\tconst resolvedAgentId = params.sessionKey ? resolveSessionAgentId({
\t\tsessionKey: params.sessionKey,
\t\tconfig: cfg
\t}) : void 0;
\tconst normalized = normalizeReplyPayload(payload, {
\t\tresponsePrefix: params.sessionKey ? resolveEffectiveMessagesConfig(cfg, resolvedAgentId ?? resolveSessionAgentId({ config: cfg }), {
\t\t\tchannel: normalizedChannel,
\t\t\taccountId
\t\t}).responsePrefix : cfg.messages?.responsePrefix === "auto" ? void 0 : cfg.messages?.responsePrefix,
\t\tenableSlackInteractiveReplies: channel === "slack" ? isSlackInteractiveRepliesEnabled({
\t\t\tcfg,
\t\t\taccountId
\t\t}) : false
\t});
\tif (!normalized) return { ok: true };
"""

ROUTE_REPLY_START_NEW = """const SHARED_SLACK_PRIVATE_ALLOWED_USERS = new Set(["U0528KFHAE8"]);
const SHARED_SLACK_TEXT_REDACTIONS = [
\t[/sunilkgrao@gmail\\.com/gi, "[redacted-private-email]"],
\t[/sunilrao\\.inc@gmail\\.com/gi, "[redacted-private-email]"],
\t[/angelicvendette@gmail\\.com/gi, "[redacted-private-email]"],
\t[/rhaine\\.arongat@tribble\\.ai/gi, "[redacted-private-email]"],
\t[/cleondelavega@guidepointglobal\\.com/gi, "[redacted-private-email]"],
\t[/\\+1\\s*646\\s*395\\s*9405/g, "[redacted-private-phone]"],
\t[/8239\\s+Oceanus\\s+Dr[^\\n]*/gi, "[redacted-private-address]"],
\t[/\\bAngelic\\b/g, "[redacted-private-name]"],
\t[/\\bEmile\\b/g, "[redacted-private-name]"],
\t[/\\bBali\\b/g, "[redacted-private-name]"],
\t[/\\bHachi\\b/g, "[redacted-private-name]"],
\t[/\\bCircle Surrogacy\\b/g, "[redacted-private-context]"],
\t[/\\bProgny\\b/g, "[redacted-private-context]"],
\t[/\\bGuidepoint\\b/g, "[redacted-private-context]"],
\t[/\\bBoca Raton\\b/g, "[redacted-private-location]"],
\t[/\\bLotus Community\\b/g, "[redacted-private-location]"],
\t[/\\bFort Lauderdale\\b/g, "[redacted-private-location]"],
\t[/\\bFLL\\b/g, "[redacted-private-location]"],
\t[/\\blipoma\\b/gi, "[redacted-private-health]"],
\t[/\\braoDesktop\\b/g, "local development environment"],
\t[/\\bclawdbot\\b/g, "live service environment"],
\t[/\\/root\\/[^\\s)\\]}]+/g, "[redacted-path]"],
\t[/\\/home\\/sunil\\/[^\\s)\\]}]+/g, "[redacted-path]"],
\t[/\\/Users\\/sunilrao\\/[^\\s)\\]}]+/g, "[redacted-path]"],
\t[/\\b[0-9a-f]{7,40}\\b/g, "[redacted-commit]"],
\t[/\\blinus\\/[a-z0-9._\\/-]+/gi, "working branch"],
\t[/\\bCodex\\b/g, "coding subagent"],
\t[/\\bClaude Code\\b/g, "coding subagent"],
\t[/\\bClaude\\b/g, "reasoning subagent"],
\t[/\\bOpenAI\\b/g, "model provider"],
\t[/\\bAnthropic\\b/g, "model provider"]
];
const SHARED_SLACK_LOW_SIGNAL_PATTERNS = [
\t/^\\s*(?:let me|now let me|good, now let me|ok, now let me|okay, now let me)\\b/im,
\t/^\\s*(?:i'm spending too much time|this approach is getting too convoluted|i need a different approach)\\b/im,
\t/^\\s*(?:wait\\b|actually\\b|interesting\\b|hmm\\b|good,\\b|ok,\\b|okay,\\b)\\s/im,
\t/^\\s*(?:on it\\b|understood\\b)\\s*[-—:]/im
];
const SHARED_SLACK_EVIDENCE_PATTERNS = [
\t/\\bStatus\\s*:/i,
\t/\\bBlocker\\s*:/i,
\t/\\breviewed\\b/i,
\t/\\bbuild passed\\b/i,
\t/\\btypecheck passed\\b/i,
\t/\\bbackend validated locally\\b/i,
\t/\\bfully locally tested\\b/i,
\t/\\bstaging-tested\\b/i,
\t/\\bhypothesis\\b/i
];
function normalizeSlackPrivacyTarget(to) {
\tif (typeof to !== "string") return "";
\tif (to.startsWith("channel:")) return to.slice(8);
\tif (to.startsWith("user:")) return to.slice(5);
\treturn to;
}
function isSharedSlackSurface(params) {
\tif (params.channel !== "slack") return false;
\tconst target = normalizeSlackPrivacyTarget(params.to);
\tif (!target) return true;
\tif (target.startsWith("D")) return false;
\tif (target.startsWith("U")) return !SHARED_SLACK_PRIVATE_ALLOWED_USERS.has(target);
\treturn true;
}
function sanitizeSharedSlackText(text) {
\tlet scrubbed = typeof text === "string" ? text : "";
\tfor (const [pattern, replacement] of SHARED_SLACK_TEXT_REDACTIONS) scrubbed = scrubbed.replace(pattern, replacement);
\treturn scrubbed;
}
function hasSharedSlackEvidenceSignal(text) {
\treturn SHARED_SLACK_EVIDENCE_PATTERNS.some((pattern) => pattern.test(text));
}
function isLowSignalSharedSlackUpdate(text) {
\tif (typeof text !== "string") return false;
\tconst trimmed = text.trim();
\tif (!trimmed) return false;
\tif (hasSharedSlackEvidenceSignal(trimmed)) return false;
\tconst lineCount = trimmed.split(/\\n+/).length;
\tconst patternHits = SHARED_SLACK_LOW_SIGNAL_PATTERNS.filter((pattern) => pattern.test(trimmed)).length;
\tif (patternHits >= 2) return true;
\tif (patternHits >= 1 && lineCount >= 3) return true;
\treturn false;
}
function sanitizeSharedSlackNormalizedPayload(params, normalized) {
\tif (!isSharedSlackSurface(params) || !normalized || typeof normalized !== "object") return normalized;
\tlet next = normalized;
\tconst originalText = typeof next.text === "string" ? next.text : "";
\tconst scrubbedText = sanitizeSharedSlackText(originalText);
\tif (isLowSignalSharedSlackUpdate(scrubbedText)) return null;
\tif (scrubbedText !== originalText) next = {
\t\t...next,
\t\ttext: scrubbedText
\t};
\tif (next.channelData?.slack && typeof next.channelData.slack === "object" && !Array.isArray(next.channelData.slack) && next.channelData.slack.blocks) next = {
\t\t...next,
\t\tchannelData: {
\t\t\t...next.channelData,
\t\t\tslack: {
\t\t\t\t...next.channelData.slack,
\t\t\t\tblocks: void 0
\t\t\t}
\t\t}
\t};
\treturn next;
}
async function routeReply(params) {
\tconst { payload, channel, to, accountId, threadId, cfg, abortSignal } = params;
\tif (shouldSuppressReasoningPayload(payload)) return { ok: true };
\tconst normalizedChannel = normalizeMessageChannel(channel);
\tconst resolvedAgentId = params.sessionKey ? resolveSessionAgentId({
\t\tsessionKey: params.sessionKey,
\t\tconfig: cfg
\t}) : void 0;
\tconst normalized = normalizeReplyPayload(payload, {
\t\tresponsePrefix: params.sessionKey ? resolveEffectiveMessagesConfig(cfg, resolvedAgentId ?? resolveSessionAgentId({ config: cfg }), {
\t\t\tchannel: normalizedChannel,
\t\t\taccountId
\t\t}).responsePrefix : cfg.messages?.responsePrefix === "auto" ? void 0 : cfg.messages?.responsePrefix,
\t\tenableSlackInteractiveReplies: channel === "slack" ? isSlackInteractiveRepliesEnabled({
\t\t\tcfg,
\t\t\taccountId
\t\t}) : false
\t});
\tconst routedNormalized = sanitizeSharedSlackNormalizedPayload(params, normalized);
\tif (!routedNormalized) return { ok: true };
"""

ROUTE_REPLY_USE_OLD = """\tlet text = normalized.text ?? "";
\tlet mediaUrls = (normalized.mediaUrls?.filter(Boolean) ?? []).length ? normalized.mediaUrls?.filter(Boolean) : normalized.mediaUrl ? [normalized.mediaUrl] : [];
\tconst replyToId = normalized.replyToId;
\tlet hasSlackBlocks = false;
\tif (channel === "slack" && normalized.channelData?.slack && typeof normalized.channelData.slack === "object" && !Array.isArray(normalized.channelData.slack)) try {
\t\thasSlackBlocks = Boolean(parseSlackBlocksInput(normalized.channelData.slack.blocks)?.length);
"""

ROUTE_REPLY_USE_NEW = """\tlet text = routedNormalized.text ?? "";
\tlet mediaUrls = (routedNormalized.mediaUrls?.filter(Boolean) ?? []).length ? routedNormalized.mediaUrls?.filter(Boolean) : routedNormalized.mediaUrl ? [routedNormalized.mediaUrl] : [];
\tconst replyToId = routedNormalized.replyToId;
\tlet hasSlackBlocks = false;
\tif (channel === "slack" && routedNormalized.channelData?.slack && typeof routedNormalized.channelData.slack === "object" && !Array.isArray(routedNormalized.channelData.slack)) try {
\t\thasSlackBlocks = Boolean(parseSlackBlocksInput(routedNormalized.channelData.slack.blocks)?.length);
"""

ROUTE_REPLY_PAYLOADS_OLD = """\t\t\t\tpayloads: [normalized],
"""

ROUTE_REPLY_PAYLOADS_NEW = """\t\t\t\tpayloads: [routedNormalized],
"""

ROUTE_REPLY_START_RE = re.compile(
    r"""async function routeReply\(params\) \{\n"""
    r"""\tconst \{ payload, channel, to, accountId, threadId, cfg, abortSignal \} = params;\n"""
    r"""\tif \(shouldSuppressReasoningPayload\(payload\)\) return \{ ok: true \};\n"""
    r"""\tconst normalizedChannel = normalizeMessageChannel\(channel\);\n"""
    r"""\tconst resolvedAgentId = params\.sessionKey \? resolveSessionAgentId\(\{\n"""
    r"""\t\tsessionKey: params\.sessionKey,\n"""
    r"""\t\tconfig: cfg\n"""
    r"""\t\}\) : void 0;\n"""
    r"""\tconst normalized = normalizeReplyPayload\(payload, \{\n"""
    r"""(?:.|\n)*?"""
    r"""\t\}\);\n"""
    r"""\tif \(!normalized\) return \{ ok: true \};\n"""
)

ROUTE_REPLY_PATCHED_START_RE = re.compile(
    r"""const SHARED_SLACK_PRIVATE_ALLOWED_USERS = new Set\(\["U0528KFHAE8"\]\);\n"""
    r"""(?:.|\n)*?"""
    r"""async function routeReply\(params\) \{\n"""
    r"""\tconst \{ payload, channel, to, accountId, threadId, cfg, abortSignal \} = params;\n"""
    r"""\tif \(shouldSuppressReasoningPayload\(payload\)\) return \{ ok: true \};\n"""
    r"""\tconst normalizedChannel = normalizeMessageChannel\(channel\);\n"""
    r"""\tconst resolvedAgentId = params\.sessionKey \? resolveSessionAgentId\(\{\n"""
    r"""\t\tsessionKey: params\.sessionKey,\n"""
    r"""\t\tconfig: cfg\n"""
    r"""\t\}\) : void 0;\n"""
    r"""\tconst normalized = normalizeReplyPayload\(payload, \{\n"""
    r"""(?:.|\n)*?"""
    r"""\t\}\);\n"""
    r"""(?:\tif \(!normalized\) return \{ ok: true \};\n|\tconst routedNormalized = sanitizeSharedSlackNormalizedPayload\(params, normalized\);\n\tif \(!routedNormalized\) return \{ ok: true \};\n)"""
)

ROUTE_REPLY_USE_RE = re.compile(
    r"""\tlet text = normalized\.text \?\? "";\n"""
    r"""\tlet mediaUrls = \(normalized\.mediaUrls\?\.filter\(Boolean\) \?\? \[\]\)\.length \? normalized\.mediaUrls\?\.filter\(Boolean\) : normalized\.mediaUrl \? \[normalized\.mediaUrl\] : \[\];\n"""
    r"""\tconst replyToId = normalized\.replyToId;\n"""
    r"""\tlet hasSlackBlocks = false;\n"""
    r"""\tif \(channel === "slack" && normalized\.channelData\?\.slack && typeof normalized\.channelData\.slack === "object" && !Array\.isArray\(normalized\.channelData\.slack\)\) try \{\n"""
    r"""\t\thasSlackBlocks = Boolean\(parseSlackBlocksInput\(normalized\.channelData\.slack\.blocks\)\?\.length\);\n"""
)

ROUTE_REPLY_PAYLOADS_RE = re.compile(r"""\t\t\t\tpayloads: \[normalized\],\n""")


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
    updated = THREAD_MENTION_POLL_MARKED_RE.sub("", updated)
    updated = THREAD_MENTION_POLL_LEGACY_RE.sub("", updated)
    updated = SLACK_MESSAGE_POLLER_RE.sub(SLACK_MESSAGE_POLLER_NEW, updated, count=1)
    updated = updated.replace(FOLLOWUP_QUEUE_ACK_ANCHOR, FOLLOWUP_QUEUE_ACK_INSERT)
    updated = updated.replace(FOLLOWUP_QUEUE_BRANCH_OLD, FOLLOWUP_QUEUE_BRANCH_NEW)
    updated = updated.replace(ROUTE_REPLY_START_OLD, ROUTE_REPLY_START_NEW)
    updated = updated.replace(ROUTE_REPLY_USE_OLD, ROUTE_REPLY_USE_NEW)
    updated = updated.replace(ROUTE_REPLY_PAYLOADS_OLD, ROUTE_REPLY_PAYLOADS_NEW)
    updated = ROUTE_REPLY_START_RE.sub(lambda _: ROUTE_REPLY_START_NEW, updated, count=1)
    updated = ROUTE_REPLY_PATCHED_START_RE.sub(lambda _: ROUTE_REPLY_START_NEW, updated, count=1)
    updated = ROUTE_REPLY_USE_RE.sub(lambda _: ROUTE_REPLY_USE_NEW, updated, count=1)
    updated = ROUTE_REPLY_PAYLOADS_RE.sub(lambda _: ROUTE_REPLY_PAYLOADS_NEW, updated, count=1)
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
