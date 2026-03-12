#!/usr/bin/env python3
"""Normalize decrypted iPhone WhatsApp databases into importer-ready NDJSON."""

from __future__ import annotations

import argparse
import json
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any


APPLE_EPOCH = datetime(2001, 1, 1, tzinfo=timezone.utc)


def normalize_text(value: Any) -> str:
    if value is None:
        return ""
    return " ".join(str(value).split()).strip()


def normalize_phone(value: Any) -> str:
    raw = normalize_text(value)
    if not raw:
        return ""
    digits = "".join(ch for ch in raw if ch.isdigit() or ch == "+")
    if digits.startswith("00"):
        digits = f"+{digits[2:]}"
    if digits and not digits.startswith("+") and digits[0].isdigit():
        digits = f"+{digits}"
    return digits


def jid_phone(value: Any) -> str:
    text = normalize_text(value).split(":")[0]
    if "@" not in text:
        return normalize_phone(text)
    local = text.split("@", 1)[0]
    if local.isdigit():
        return f"+{local}"
    return ""


def is_group_jid(value: Any) -> bool:
    return normalize_text(value).endswith("@g.us")


def is_status_jid(value: Any) -> bool:
    return normalize_text(value).endswith("@status") or normalize_text(value) == "status"


def apple_ts_to_iso(value: Any) -> str:
    if value in (None, "", 0):
        return ""
    try:
        seconds = float(value)
    except Exception:
        return ""
    if seconds < 0 or seconds > 60 * 60 * 24 * 365 * 200:
        return ""
    candidate = APPLE_EPOCH + timedelta(seconds=seconds)
    if candidate.year < 2001 or candidate.year > 2100:
        return ""
    return candidate.isoformat()


def safe_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=True, separators=(",", ":"))


@dataclass
class ChatSession:
    chat_pk: int
    chat_id: str
    chat_name: str
    is_group: bool
    last_message_at: str


def load_chat_sessions(chat_db: Path) -> dict[int, ChatSession]:
    con = sqlite3.connect(str(chat_db))
    con.row_factory = sqlite3.Row
    cur = con.cursor()
    sessions: dict[int, ChatSession] = {}
    for row in cur.execute(
        """
        SELECT
            Z_PK,
            ZCONTACTJID,
            ZPARTNERNAME,
            ZLASTMESSAGETEXT,
            ZLASTMESSAGEDATE
        FROM ZWACHATSESSION
        """
    ):
        chat_id = normalize_text(row["ZCONTACTJID"])
        chat_name = normalize_text(row["ZPARTNERNAME"] or row["ZLASTMESSAGETEXT"])
        sessions[row["Z_PK"]] = ChatSession(
            chat_pk=row["Z_PK"],
            chat_id=chat_id,
            chat_name=chat_name,
            is_group=is_group_jid(chat_id),
            last_message_at=apple_ts_to_iso(row["ZLASTMESSAGEDATE"]),
        )
    con.close()
    return sessions


def load_push_names(chat_db: Path) -> dict[str, str]:
    con = sqlite3.connect(str(chat_db))
    con.row_factory = sqlite3.Row
    cur = con.cursor()
    out: dict[str, str] = {}
    for row in cur.execute("SELECT ZJID, ZPUSHNAME FROM ZWAPROFILEPUSHNAME"):
        jid = normalize_text(row["ZJID"])
        name = normalize_text(row["ZPUSHNAME"])
        if jid and name:
            out[jid] = name
    con.close()
    return out


def load_group_members(chat_db: Path) -> dict[int, dict[str, str]]:
    con = sqlite3.connect(str(chat_db))
    con.row_factory = sqlite3.Row
    cur = con.cursor()
    out: dict[int, dict[str, str]] = {}
    for row in cur.execute(
        """
        SELECT Z_PK, ZMEMBERJID, ZCONTACTNAME, ZFIRSTNAME
        FROM ZWAGROUPMEMBER
        """
    ):
        out[row["Z_PK"]] = {
            "jid": normalize_text(row["ZMEMBERJID"]),
            "name": normalize_text(row["ZCONTACTNAME"] or row["ZFIRSTNAME"]),
        }
    con.close()
    return out


def load_contacts(contacts_db: Path) -> list[dict[str, Any]]:
    con = sqlite3.connect(str(contacts_db))
    con.row_factory = sqlite3.Row
    cur = con.cursor()
    out: list[dict[str, Any]] = []
    for row in cur.execute(
        """
        SELECT
            ZUNIQUEID,
            ZFULLNAME,
            ZGIVENNAME,
            ZPHONENUMBER,
            ZWHATSAPPID,
            ZABOUTTEXT,
            ZBUSINESSNAME,
            ZLASTUPDATED
        FROM ZWAADDRESSBOOKCONTACT
        """
    ):
        contact_id = normalize_text(row["ZWHATSAPPID"] or row["ZUNIQUEID"])
        display_name = normalize_text(row["ZFULLNAME"] or row["ZGIVENNAME"] or row["ZBUSINESSNAME"])
        phone = normalize_phone(row["ZPHONENUMBER"])
        if is_status_jid(contact_id):
            continue
        if not display_name and not phone:
            continue
        out.append(
            {
                "kind": "contact",
                "channel": "whatsapp",
                "account_id": "default",
                "contact_id": contact_id or phone or display_name,
                "display_name": display_name,
                "short_name": normalize_text(row["ZGIVENNAME"]),
                "phone": phone,
                "updated_at": apple_ts_to_iso(row["ZLASTUPDATED"]) or datetime.now(timezone.utc).isoformat(),
                "raw": {
                    "about": normalize_text(row["ZABOUTTEXT"]),
                    "business_name": normalize_text(row["ZBUSINESSNAME"]),
                },
            }
        )
    con.close()
    return out


def pick_message_type(message_type: Any, text: str, has_media: bool, group_event_type: Any) -> str:
    if text:
        return "text"
    if has_media:
        return "media"
    if group_event_type not in (None, 0):
        return "group-event"
    return f"type-{int(message_type)}" if message_type is not None else "unknown"


def load_media_message_ids(chat_db: Path) -> set[int]:
    con = sqlite3.connect(str(chat_db))
    cur = con.cursor()
    ids = {row[0] for row in cur.execute("SELECT DISTINCT ZMESSAGE FROM ZWAMEDIAITEM WHERE ZMESSAGE IS NOT NULL")}
    con.close()
    return ids


def build_chat_records(sessions: dict[int, ChatSession]) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for session in sessions.values():
        if not session.chat_id:
            continue
        if is_status_jid(session.chat_id):
            continue
        records.append(
            {
                "kind": "chat",
                "channel": "whatsapp",
                "account_id": "default",
                "chat_id": session.chat_id,
                "chat_name": session.chat_name,
                "chat_phone": jid_phone(session.chat_id),
                "is_group": session.is_group,
                "last_message_at": session.last_message_at,
                "updated_at": datetime.now(timezone.utc).isoformat(),
                "raw": {
                    "chat_pk": session.chat_pk,
                },
            }
        )
    return records


def build_message_records(
    chat_db: Path,
    sessions: dict[int, ChatSession],
    push_names: dict[str, str],
    group_members: dict[int, dict[str, str]],
    media_message_ids: set[int],
) -> list[dict[str, Any]]:
    con = sqlite3.connect(str(chat_db))
    con.row_factory = sqlite3.Row
    cur = con.cursor()
    rows = cur.execute(
        """
        SELECT
            Z_PK,
            ZISFROMME,
            ZMESSAGEDATE,
            ZSENTDATE,
            ZFROMJID,
            ZTOJID,
            ZPUSHNAME,
            ZSTANZAID,
            ZTEXT,
            ZMESSAGETYPE,
            ZGROUPEVENTTYPE,
            ZCHATSESSION,
            ZGROUPMEMBER
        FROM ZWAMESSAGE
        ORDER BY COALESCE(ZMESSAGEDATE, ZSENTDATE), Z_PK
        """
    )
    out: list[dict[str, Any]] = []
    for row in rows:
        session = sessions.get(row["ZCHATSESSION"])
        if not session or not session.chat_id:
            continue
        if is_status_jid(session.chat_id):
            continue
        is_from_me = bool(row["ZISFROMME"])
        group_member = group_members.get(row["ZGROUPMEMBER"], {})
        sender_id = normalize_text(group_member.get("jid") or row["ZFROMJID"])
        sender_name = normalize_text(group_member.get("name") or row["ZPUSHNAME"] or push_names.get(sender_id))
        chat_phone = jid_phone(session.chat_id)
        counterpart_phone = chat_phone
        counterpart_name = session.chat_name or push_names.get(session.chat_id, "")
        if session.is_group:
            counterpart_phone = ""
            counterpart_name = ""
            if is_from_me:
                sender_id = ""
                sender_name = ""
        else:
            direct_jid = normalize_text(row["ZFROMJID"] or row["ZTOJID"] or session.chat_id)
            counterpart_name = normalize_text(push_names.get(direct_jid) or session.chat_name or row["ZPUSHNAME"])
            counterpart_phone = jid_phone(direct_jid or session.chat_id)
            if not is_from_me:
                sender_id = direct_jid or session.chat_id
                sender_name = normalize_text(row["ZPUSHNAME"] or push_names.get(sender_id) or counterpart_name)

        text = normalize_text(row["ZTEXT"])
        message_type = pick_message_type(
            row["ZMESSAGETYPE"],
            text,
            row["Z_PK"] in media_message_ids,
            row["ZGROUPEVENTTYPE"],
        )
        sent_at = apple_ts_to_iso(row["ZMESSAGEDATE"]) or apple_ts_to_iso(row["ZSENTDATE"])
        message_id = normalize_text(row["ZSTANZAID"]) or f"ios-backup-{row['Z_PK']}"
        out.append(
            {
                "kind": "message",
                "channel": "whatsapp",
                "account_id": "default",
                "chat_id": session.chat_id,
                "chat_name": session.chat_name,
                "chat_phone": chat_phone,
                "is_group": session.is_group,
                "message_id": message_id,
                "sender_id": sender_id,
                "sender_name": sender_name,
                "sender_phone": jid_phone(sender_id),
                "counterpart_name": counterpart_name,
                "counterpart_phone": counterpart_phone,
                "direction": "outbound" if is_from_me else "inbound",
                "sent_at": sent_at or datetime.now(timezone.utc).isoformat(),
                "message_type": message_type,
                "text": text,
                "excerpt": (text[:277] + "...") if len(text) > 280 else text,
                "is_history": True,
                "chat_raw": {"chat_pk": session.chat_pk},
                "raw": {
                    "row_pk": row["Z_PK"],
                    "message_type": row["ZMESSAGETYPE"],
                    "group_event_type": row["ZGROUPEVENTTYPE"],
                    "from_jid": normalize_text(row["ZFROMJID"]),
                    "to_jid": normalize_text(row["ZTOJID"]),
                },
            }
        )
    con.close()
    return out


def write_ndjson(path: Path, records: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for record in records:
            handle.write(safe_json(record))
            handle.write("\n")


def main() -> int:
    parser = argparse.ArgumentParser(description="Normalize iPhone WhatsApp backup DBs into NDJSON")
    parser.add_argument("--chat-db", required=True)
    parser.add_argument("--contacts-db", required=True)
    parser.add_argument("--calls-db", required=False)
    parser.add_argument("--ext-db", required=False)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    chat_db = Path(args.chat_db).expanduser().resolve()
    contacts_db = Path(args.contacts_db).expanduser().resolve()
    out = Path(args.out).expanduser().resolve()

    sessions = load_chat_sessions(chat_db)
    push_names = load_push_names(chat_db)
    group_members = load_group_members(chat_db)
    media_message_ids = load_media_message_ids(chat_db)

    records: list[dict[str, Any]] = []
    records.extend(load_contacts(contacts_db))
    records.extend(build_chat_records(sessions))
    records.extend(build_message_records(chat_db, sessions, push_names, group_members, media_message_ids))
    write_ndjson(out, records)
    print(
        json.dumps(
            {
                "output": str(out),
                "records": len(records),
                "contacts": sum(1 for r in records if r["kind"] == "contact"),
                "chats": sum(1 for r in records if r["kind"] == "chat"),
                "messages": sum(1 for r in records if r["kind"] == "message"),
            },
            indent=2,
            ensure_ascii=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
