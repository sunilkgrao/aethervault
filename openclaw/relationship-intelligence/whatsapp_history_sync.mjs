#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

function parseArgs(argv) {
  const args = {
    "auth-dir": "/root/.openclaw/credentials/whatsapp/default",
    "output": "",
    "timeout-seconds": "180",
    "settle-ms": "6000",
    "idle-ms": "30000",
    "account-id": "default",
    "channel": "whatsapp",
    "allow-qr": "false",
    "qr-file": "",
  };
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (!token.startsWith("--")) continue;
    const key = token.slice(2);
    const next = argv[i + 1];
    if (next && !next.startsWith("--")) {
      args[key] = next;
      i += 1;
    } else {
      args[key] = "true";
    }
  }
  return args;
}

function resolveModule(candidates) {
  for (const candidate of candidates) {
    try {
      return require(candidate);
    } catch {}
  }
  throw new Error(`Unable to resolve module from candidates: ${candidates.join(", ")}`);
}

function createLogger() {
  const noop = () => {};
  const logger = {
    level: "silent",
    trace: noop,
    debug: noop,
    info: noop,
    warn: noop,
    error: noop,
    fatal: noop,
    child() {
      return logger;
    },
  };
  return logger;
}

function asBool(value) {
  return ["1", "true", "yes", "on"].includes(normalizeText(value).toLowerCase());
}

function normalizeText(value) {
  if (value === null || value === undefined) return "";
  return String(value).replace(/\s+/g, " ").trim();
}

function jidPhone(jid) {
  const clean = normalizeText(jid).split(":")[0];
  const match = clean.match(/^(\d{6,15})@/);
  return match ? `+${match[1]}` : "";
}

function isGroupJid(jid) {
  return normalizeText(jid).endsWith("@g.us");
}

function pickMessageEnvelope(message) {
  if (!message || typeof message !== "object") return null;
  if (message.ephemeralMessage?.message) return pickMessageEnvelope(message.ephemeralMessage.message);
  if (message.viewOnceMessage?.message) return pickMessageEnvelope(message.viewOnceMessage.message);
  if (message.viewOnceMessageV2?.message) return pickMessageEnvelope(message.viewOnceMessageV2.message);
  if (message.viewOnceMessageV2Extension?.message) return pickMessageEnvelope(message.viewOnceMessageV2Extension.message);
  if (message.documentWithCaptionMessage?.message) return pickMessageEnvelope(message.documentWithCaptionMessage.message);
  if (message.editedMessage?.message) return pickMessageEnvelope(message.editedMessage.message);
  const keys = Object.keys(message);
  if (!keys.length) return null;
  const type = keys[0];
  return { type, payload: message[type] };
}

function extractMessageText(message) {
  const envelope = pickMessageEnvelope(message);
  if (!envelope) return { type: "unknown", text: "" };
  const { type, payload } = envelope;
  switch (type) {
    case "conversation":
      return { type, text: normalizeText(payload) };
    case "extendedTextMessage":
      return { type, text: normalizeText(payload?.text) };
    case "imageMessage":
    case "videoMessage":
    case "documentMessage":
      return { type, text: normalizeText(payload?.caption) };
    case "documentWithCaptionMessage":
      return extractMessageText(payload?.message);
    case "buttonsResponseMessage":
      return { type, text: normalizeText(payload?.selectedDisplayText || payload?.selectedButtonId) };
    case "listResponseMessage":
      return { type, text: normalizeText(payload?.title || payload?.singleSelectReply?.selectedRowId) };
    case "templateButtonReplyMessage":
      return { type, text: normalizeText(payload?.selectedDisplayText || payload?.selectedId) };
    case "contactMessage":
      return { type, text: normalizeText(payload?.displayName) };
    case "locationMessage":
      return { type, text: normalizeText(payload?.name || payload?.address) };
    case "pollCreationMessage":
      return { type, text: normalizeText(payload?.name) };
    case "reactionMessage":
      return { type, text: normalizeText(payload?.text) };
    default:
      return { type, text: normalizeText(payload?.text || payload?.caption || payload?.title || payload?.displayName) };
  }
}

function excerpt(text) {
  const clean = normalizeText(text);
  if (clean.length <= 280) return clean;
  return `${clean.slice(0, 277)}...`;
}

function safeJson(value) {
  return JSON.parse(
    JSON.stringify(value, (_, candidate) => {
      if (typeof candidate === "bigint") return Number(candidate);
      if (candidate && typeof candidate === "object") {
        if (typeof candidate.toNumber === "function") {
          try {
            return candidate.toNumber();
          } catch {}
        }
      }
      return candidate;
    }),
  );
}

function timestampToIso(raw) {
  if (raw === null || raw === undefined) return "";
  let numeric;
  if (typeof raw === "number") {
    numeric = raw;
  } else if (typeof raw === "bigint") {
    numeric = Number(raw);
  } else if (typeof raw === "object" && raw !== null && typeof raw.toNumber === "function") {
    numeric = raw.toNumber();
  } else {
    numeric = Number(raw);
  }
  if (!Number.isFinite(numeric) || numeric <= 0) return "";
  if (numeric < 1e12) numeric *= 1000;
  return new Date(numeric).toISOString();
}

class HistoryWriter {
  constructor(outputPath) {
    this.outputPath = outputPath;
    this.seenContacts = new Set();
    this.seenChats = new Set();
    this.seenMessages = new Set();
    this.counts = { contacts: 0, chats: 0, messages: 0 };
    this.contactNames = new Map();
    this.chatNames = new Map();
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });
    fs.writeFileSync(outputPath, "", "utf8");
  }

  line(record) {
    fs.appendFileSync(this.outputPath, `${JSON.stringify(record)}\n`, "utf8");
  }

  rememberContact(contact) {
    const id = normalizeText(contact?.id);
    if (!id) return;
    const name = normalizeText(contact?.name || contact?.notify || contact?.verifiedName || contact?.short);
    if (name) this.contactNames.set(id, name);
    const phone = jidPhone(id);
    if (phone && name && !this.contactNames.has(phone)) this.contactNames.set(phone, name);
  }

  rememberChat(chat) {
    const id = normalizeText(chat?.id);
    if (!id) return;
    const name = normalizeText(chat?.name || chat?.subject || chat?.conversationTimestamp);
    if (name) this.chatNames.set(id, name);
  }

  writeContact(contact, meta = {}) {
    this.rememberContact(contact);
    const contactId = normalizeText(contact?.id);
    if (!contactId) return;
    const dedupeKey = `${meta.channel || "whatsapp"}:${meta.accountId || "default"}:${contactId}`;
    if (this.seenContacts.has(dedupeKey)) return;
    this.seenContacts.add(dedupeKey);
    this.counts.contacts += 1;
    this.line({
      kind: "contact",
      channel: meta.channel || "whatsapp",
      account_id: meta.accountId || "default",
      contact_id: contactId,
      display_name: normalizeText(contact?.name || contact?.notify || contact?.verifiedName),
      short_name: normalizeText(contact?.short),
      phone: jidPhone(contactId),
      updated_at: new Date().toISOString(),
      raw: safeJson(contact),
    });
  }

  writeChat(chat, meta = {}) {
    this.rememberChat(chat);
    const chatId = normalizeText(chat?.id);
    if (!chatId) return;
    const dedupeKey = `${meta.channel || "whatsapp"}:${meta.accountId || "default"}:${chatId}`;
    if (this.seenChats.has(dedupeKey)) return;
    this.seenChats.add(dedupeKey);
    this.counts.chats += 1;
    this.line({
      kind: "chat",
      channel: meta.channel || "whatsapp",
      account_id: meta.accountId || "default",
      chat_id: chatId,
      chat_name: normalizeText(chat?.name || chat?.subject || this.contactNames.get(chatId)),
      chat_phone: jidPhone(chatId),
      is_group: isGroupJid(chatId),
      last_message_at: timestampToIso(chat?.conversationTimestamp || chat?.lastMessageRecvTimestamp || chat?.lastMessageSendTimestamp),
      updated_at: new Date().toISOString(),
      raw: safeJson(chat),
    });
  }

  resolveName(jid, fallback = "") {
    const clean = normalizeText(jid);
    return (
      normalizeText(fallback) ||
      this.chatNames.get(clean) ||
      this.contactNames.get(clean) ||
      this.contactNames.get(jidPhone(clean)) ||
      ""
    );
  }

  writeMessage(message, meta = {}) {
    const key = message?.key || {};
    const messageId = normalizeText(key?.id);
    const chatId = normalizeText(key?.remoteJid);
    if (!messageId || !chatId) return;
    const dedupeKey = `${meta.channel || "whatsapp"}:${meta.accountId || "default"}:${chatId}:${messageId}`;
    if (this.seenMessages.has(dedupeKey)) return;
    this.seenMessages.add(dedupeKey);

    const { type, text } = extractMessageText(message?.message);
    const isGroup = isGroupJid(chatId);
    const senderId = normalizeText(message?.participant || key?.participant || (!key?.fromMe ? chatId : ""));
    const counterpartId = !isGroup ? chatId : "";
    const sentAt = timestampToIso(message?.messageTimestamp) || meta.sentAt || new Date().toISOString();
    const chatName = normalizeText(meta.chatName || this.resolveName(chatId));
    const senderName = normalizeText(meta.senderName || this.resolveName(senderId, message?.pushName));
    const counterpartName = normalizeText(meta.counterpartName || this.resolveName(counterpartId));

    this.counts.messages += 1;
    this.line({
      kind: "message",
      channel: meta.channel || "whatsapp",
      account_id: meta.accountId || "default",
      chat_id: chatId,
      chat_name: chatName,
      chat_phone: jidPhone(chatId),
      is_group: isGroup,
      message_id: messageId,
      sender_id: senderId,
      sender_name: senderName,
      sender_phone: jidPhone(senderId),
      counterpart_name: counterpartName,
      counterpart_phone: jidPhone(counterpartId),
      direction: key?.fromMe ? "outbound" : "inbound",
      sent_at: sentAt,
      message_type: type,
      text,
      excerpt: excerpt(text || type),
      is_history: !!meta.isHistory,
      raw: safeJson(message),
    });
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.output) {
    throw new Error("--output is required");
  }

  const baileys = resolveModule([
    "@whiskeysockets/baileys",
    "/usr/lib/node_modules/openclaw/node_modules/@whiskeysockets/baileys",
  ]);
  const makeWASocket = baileys.default || baileys.makeWASocket;
  const useMultiFileAuthState = baileys.useMultiFileAuthState;
  const makeCacheableSignalKeyStore = baileys.makeCacheableSignalKeyStore;
  const fetchLatestBaileysVersion = baileys.fetchLatestBaileysVersion;
  const qrcodeTerminal = resolveModule([
    "/usr/lib/node_modules/openclaw/node_modules/qrcode-terminal",
  ]);
  if (!makeWASocket || !useMultiFileAuthState || !makeCacheableSignalKeyStore) {
    throw new Error("Baileys exports missing makeWASocket/useMultiFileAuthState");
  }

  let openclawVersion = "2026.3.7";
  try {
    openclawVersion = require("/usr/lib/node_modules/openclaw/package.json").version || openclawVersion;
  } catch {}

  const writer = new HistoryWriter(path.resolve(args.output));
  const { state, saveCreds } = await useMultiFileAuthState(path.resolve(args["auth-dir"]));
  const timeoutMs = Number(args["timeout-seconds"] || 75) * 1000;
  const settleMs = Number(args["settle-ms"] || 6000);
  const idleMs = Number(args["idle-ms"] || 30000);
  const allowQr = asBool(args["allow-qr"]);
  const qrFile = normalizeText(args["qr-file"]);
  const logger = createLogger();
  const startedAt = Date.now();
  let finished = false;
  let settleTimer = null;
  let timeoutTimer = null;
  let lastEventAt = Date.now();

  const finish = (code, reason) => {
    if (finished) return;
    finished = true;
    if (settleTimer) clearTimeout(settleTimer);
    if (timeoutTimer) clearTimeout(timeoutTimer);
    const elapsedMs = Date.now() - startedAt;
    process.stderr.write(
      `whatsapp-history-sync finished code=${code} reason=${reason} messages=${writer.counts.messages} chats=${writer.counts.chats} contacts=${writer.counts.contacts} elapsedMs=${elapsedMs}\n`,
    );
    process.exit(code);
  };

  const scheduleSettle = (reason, delay = settleMs) => {
    if (settleTimer) clearTimeout(settleTimer);
    settleTimer = setTimeout(() => finish(0, reason), delay);
  };

  timeoutTimer = setTimeout(() => finish(0, "timeout"), timeoutMs);

  let version;
  try {
    version = (await fetchLatestBaileysVersion?.())?.version;
  } catch {}

  const sock = makeWASocket({
    auth: {
      creds: state.creds,
      keys: makeCacheableSignalKeyStore(state.keys, logger),
    },
    version,
    browser: ["openclaw", "cli", openclawVersion],
    syncFullHistory: true,
    fireInitQueries: true,
    shouldSyncHistoryMessage: () => true,
    markOnlineOnConnect: false,
    logger,
    printQRInTerminal: false,
  });

  sock.ev.on("creds.update", saveCreds);

  sock.ev.on("contacts.upsert", (contacts) => {
    lastEventAt = Date.now();
    for (const contact of contacts || []) writer.writeContact(contact, { channel: args.channel, accountId: args["account-id"] });
    scheduleSettle("contacts-upsert");
  });

  sock.ev.on("chats.upsert", (chats) => {
    lastEventAt = Date.now();
    for (const chat of chats || []) writer.writeChat(chat, { channel: args.channel, accountId: args["account-id"] });
    scheduleSettle("chats-upsert");
  });

  sock.ev.on("messaging-history.set", (payload) => {
    lastEventAt = Date.now();
    for (const contact of payload?.contacts || []) writer.writeContact(contact, { channel: args.channel, accountId: args["account-id"] });
    for (const chat of payload?.chats || []) writer.writeChat(chat, { channel: args.channel, accountId: args["account-id"] });
    for (const message of payload?.messages || []) {
      writer.writeMessage(message, {
        channel: args.channel,
        accountId: args["account-id"],
        isHistory: true,
      });
    }
    if (payload?.isLatest) {
      scheduleSettle("history-latest");
    } else {
      scheduleSettle("history-partial", idleMs);
    }
  });

  sock.ev.on("messages.upsert", (upsert) => {
    if (!upsert || (upsert.type !== "notify" && upsert.type !== "append")) return;
    lastEventAt = Date.now();
    for (const message of upsert.messages || []) {
      writer.writeMessage(message, {
        channel: args.channel,
        accountId: args["account-id"],
        isHistory: upsert.type === "append",
      });
    }
    scheduleSettle(upsert.type === "append" ? "messages-append" : "messages-notify", idleMs);
  });

  sock.ev.on("connection.update", (update) => {
    lastEventAt = Date.now();
    if (update?.qr) {
      if (qrFile) {
        fs.mkdirSync(path.dirname(path.resolve(qrFile)), { recursive: true });
        fs.writeFileSync(path.resolve(qrFile), `${update.qr}\n`, "utf8");
      }
      try {
        qrcodeTerminal.generate(update.qr, { small: true }, (rendered) => {
          process.stderr.write(`${rendered}\n`);
        });
      } catch {}
      if (!allowQr) {
        process.stderr.write("whatsapp-history-sync requires an already linked session; got QR instead.\n");
        finish(2, "qr-required");
      }
      return;
    }
    if (update?.connection === "open") {
      process.stderr.write("whatsapp-history-sync connected.\n");
      scheduleSettle("connected-idle", idleMs);
      return;
    }
    if (update?.connection === "close") {
      const rawStatus = update?.lastDisconnect?.error?.output?.statusCode ?? update?.lastDisconnect?.error?.statusCode;
      const inactiveFor = Date.now() - lastEventAt;
      if (writer.counts.messages > 0 && inactiveFor >= settleMs) {
        finish(0, `connection-closed-after-export:${rawStatus ?? "unknown"}`);
        return;
      }
      finish(1, `connection-closed:${rawStatus ?? "unknown"}`);
    }
  });
}

main().catch((error) => {
  process.stderr.write(`${error?.stack || error}\n`);
  process.exit(1);
});
