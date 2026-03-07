#!/usr/bin/env python3
"""Shared executive-state helpers for AetherVault lifecycle scripts."""

from __future__ import annotations

import json
import os
import re
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Tuple

AETHERVAULT_HOME = os.environ.get("AETHERVAULT_HOME", os.path.expanduser("~/.aethervault"))
WORKSPACE_DIR = Path(
    os.environ.get("AETHERVAULT_WORKSPACE", str(Path(AETHERVAULT_HOME) / "workspace"))
)
STATE_JSON_PATH = WORKSPACE_DIR / "STATE.json"
STATE_MARKDOWN_PATH = WORKSPACE_DIR / "STATE.md"
LEGACY_STATE_JSON_PATH = WORKSPACE_DIR / "SSTATE.json"
LEGACY_STATE_MARKDOWN_PATH = WORKSPACE_DIR / "SSTATE.md"

_DEFAULT_STATE = {"items": [], "notes": [], "updated_at": None}
_ALLOWED_KINDS = {
    "priority",
    "task",
    "project",
    "follow_up",
    "waiting_on",
    "note",
    "meeting",
    "draft",
}
_ALLOWED_STATUSES = {"active", "pending", "waiting", "done", "archived"}


def _now_rfc3339() -> str:
    return datetime.now(timezone.utc).isoformat()


def _normalize_kind(value: Optional[str]) -> str:
    raw = (value or "task").strip().lower().replace("-", "_")
    return raw if raw in _ALLOWED_KINDS else "task"


def _normalize_status(value: Optional[str]) -> str:
    raw = (value or "active").strip().lower()
    return raw if raw in _ALLOWED_STATUSES else "active"


def _clean_optional(value: Optional[object]) -> Optional[str]:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _sanitize_item(raw: Dict[str, object]) -> Dict[str, object]:
    title = str(raw.get("title", "")).strip()
    item = {
        "id": _clean_optional(raw.get("id")) or "",
        "title": title,
        "kind": _normalize_kind(raw.get("kind")),
        "status": _normalize_status(raw.get("status")),
        "next_action": _clean_optional(raw.get("next_action")),
        "due": _clean_optional(raw.get("due")),
        "waiting_on": _clean_optional(raw.get("waiting_on")),
        "notes": [
            str(note).strip()
            for note in raw.get("notes", []) or []
            if str(note).strip()
        ],
        "source": _clean_optional(raw.get("source")),
        "session": _clean_optional(raw.get("session")),
        "updated_at": _clean_optional(raw.get("updated_at")) or _now_rfc3339(),
    }
    return item


def load_executive_state(json_path: Path = STATE_JSON_PATH) -> Dict[str, object]:
    if not json_path.exists() and LEGACY_STATE_JSON_PATH.exists():
        json_path = LEGACY_STATE_JSON_PATH
    if not json_path.exists():
        return dict(_DEFAULT_STATE)
    try:
        data = json.loads(json_path.read_text())
    except (json.JSONDecodeError, OSError):
        return dict(_DEFAULT_STATE)
    items = [
        _sanitize_item(item)
        for item in data.get("items", []) or []
        if isinstance(item, dict) and str(item.get("title", "")).strip()
    ]
    notes = [str(note).strip() for note in data.get("notes", []) or [] if str(note).strip()]
    return {
        "items": items,
        "notes": notes,
        "updated_at": _clean_optional(data.get("updated_at")),
    }


def _item_sort_key(item: Dict[str, object]) -> Tuple[int, str, str]:
    status = str(item.get("status", "active"))
    status_rank = {
        "active": 0,
        "pending": 1,
        "waiting": 2,
        "done": 3,
        "archived": 4,
    }.get(status, 5)
    due = str(item.get("due") or "9999-99-99")
    updated_at = str(item.get("updated_at") or "")
    return (status_rank, due, updated_at)


def _item_line(item: Dict[str, object]) -> str:
    parts = [f"[{item.get('status')}][{item.get('kind')}] {item.get('title')}"]
    next_action = _clean_optional(item.get("next_action"))
    waiting_on = _clean_optional(item.get("waiting_on"))
    due = _clean_optional(item.get("due"))
    if next_action:
        parts.append(f"next: {next_action}")
    if waiting_on:
        parts.append(f"waiting on: {waiting_on}")
    if due:
        parts.append(f"due: {due}")
    return " | ".join(parts)


def render_executive_state_markdown(state: Dict[str, object]) -> str:
    items = sorted(state.get("items", []), key=_item_sort_key)
    lines = ["# Executive State", "", "## Open Loops"]

    open_items = [
        item for item in items if item.get("status") not in {"done", "archived"}
    ]
    if not open_items:
        lines.append("- None currently tracked.")
    else:
        for item in open_items:
            lines.append(f"- {_item_line(item)}")
            notes = item.get("notes", []) or []
            if notes:
                lines.append(f"  note: {notes[-1]}")

    state_notes = state.get("notes", []) or []
    if state_notes:
        lines.extend(["", "## Notes"])
        for note in state_notes[-8:]:
            lines.append(f"- {note}")

    updated_at = _clean_optional(state.get("updated_at"))
    if updated_at:
        lines.extend(["", f"_Updated: {updated_at}_"])
    return "\n".join(lines) + "\n"


def save_executive_state(
    state: Dict[str, object],
    json_path: Path = STATE_JSON_PATH,
    markdown_path: Path = STATE_MARKDOWN_PATH,
) -> None:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    markdown_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "items": [_sanitize_item(item) for item in state.get("items", []) or []],
        "notes": [str(note).strip() for note in state.get("notes", []) or [] if str(note).strip()],
        "updated_at": _clean_optional(state.get("updated_at")) or _now_rfc3339(),
    }
    json_path.write_text(json.dumps(payload, indent=2) + "\n")
    markdown_path.write_text(render_executive_state_markdown(payload))
    if LEGACY_STATE_JSON_PATH.exists():
        LEGACY_STATE_JSON_PATH.unlink()
    if LEGACY_STATE_MARKDOWN_PATH.exists():
        LEGACY_STATE_MARKDOWN_PATH.unlink()


def _due_within_days(item: Dict[str, object], days: int) -> bool:
    due = _clean_optional(item.get("due"))
    if not due:
        return False
    try:
        due_date = date.fromisoformat(due[:10])
    except ValueError:
        return False
    delta = (due_date - datetime.now(timezone.utc).date()).days
    return 0 <= delta <= days


def render_state_focus_summary(
    state: Optional[Dict[str, object]] = None,
    limit: int = 8,
    include_notes: bool = True,
) -> str:
    state = state or load_executive_state()
    items = sorted(state.get("items", []), key=_item_sort_key)
    open_items = [
        item for item in items if item.get("status") not in {"done", "archived"}
    ]
    waiting_items = [
        item
        for item in open_items
        if item.get("status") == "waiting"
        or item.get("kind") in {"waiting_on", "follow_up"}
    ]
    due_items = [item for item in open_items if _due_within_days(item, 14)]

    lines: List[str] = []
    if open_items:
        lines.append("Top open loops:")
        for item in open_items[:limit]:
            lines.append(f"- {_item_line(item)}")

    if due_items:
        if lines:
            lines.append("")
        lines.append("Upcoming deadlines:")
        for item in due_items[: min(5, limit)]:
            lines.append(f"- {_item_line(item)}")

    if waiting_items:
        if lines:
            lines.append("")
        lines.append("Waiting on / follow-ups:")
        for item in waiting_items[: min(5, limit)]:
            lines.append(f"- {_item_line(item)}")

    notes = state.get("notes", []) or []
    if include_notes and notes:
        if lines:
            lines.append("")
        lines.append("Recent executive notes:")
        for note in notes[-4:]:
            lines.append(f"- {note}")

    return "\n".join(lines).strip()


def read_state_focus_summary(limit: int = 8) -> Tuple[str, bool]:
    state = load_executive_state()
    summary = render_state_focus_summary(state, limit=limit)
    return summary, bool(summary)


def _slugify(text: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return slug or "item"


def _find_existing_item(
    items: List[Dict[str, object]],
    item_id: Optional[str],
    kind: str,
    title: str,
) -> Optional[Dict[str, object]]:
    title_key = title.strip().lower()
    if item_id:
        for item in items:
            if item.get("id") == item_id:
                return item
    for item in items:
        if (
            str(item.get("kind")) == kind
            and str(item.get("title", "")).strip().lower() == title_key
        ):
            return item
    return None


def _allocate_item_id(items: List[Dict[str, object]], kind: str, title: str) -> str:
    base = f"{kind}-{_slugify(title)}"
    candidate = base
    existing_ids = {str(item.get("id", "")) for item in items}
    suffix = 2
    while candidate in existing_ids:
        candidate = f"{base}-{suffix}"
        suffix += 1
    return candidate


def apply_state_updates(
    updates: Iterable[Dict[str, object]],
    *,
    dry_run: bool = False,
    source_note: Optional[str] = None,
) -> Tuple[Dict[str, object], int]:
    state = load_executive_state()
    items = state.get("items", []) or []
    now = _now_rfc3339()
    changed = 0

    for raw in updates:
        if not isinstance(raw, dict):
            continue
        title = str(raw.get("title", "")).strip()
        if not title:
            continue
        kind = _normalize_kind(raw.get("kind"))
        status = _normalize_status(raw.get("status"))
        item_id = _clean_optional(raw.get("id"))
        existing = _find_existing_item(items, item_id, kind, title)
        note = _clean_optional(raw.get("note"))

        if existing is None:
            existing = _sanitize_item(
                {
                    "id": item_id or _allocate_item_id(items, kind, title),
                    "title": title,
                    "kind": kind,
                    "status": status,
                    "next_action": raw.get("next_action"),
                    "due": raw.get("due"),
                    "waiting_on": raw.get("waiting_on"),
                    "notes": [note] if note else [],
                    "source": raw.get("source"),
                    "session": raw.get("session"),
                    "updated_at": now,
                }
            )
            items.append(existing)
            changed += 1
            continue

        existing["title"] = title
        existing["kind"] = kind
        existing["status"] = status
        if "next_action" in raw:
            existing["next_action"] = _clean_optional(raw.get("next_action"))
        if "due" in raw:
            existing["due"] = _clean_optional(raw.get("due"))
        if "waiting_on" in raw:
            existing["waiting_on"] = _clean_optional(raw.get("waiting_on"))
        if "source" in raw:
            existing["source"] = _clean_optional(raw.get("source"))
        if "session" in raw:
            existing["session"] = _clean_optional(raw.get("session"))
        if note:
            notes = existing.setdefault("notes", [])
            if not notes or str(notes[-1]).strip() != note:
                notes.append(note)
        existing["updated_at"] = now
        changed += 1

    if changed:
        state["items"] = items
        state["updated_at"] = now
        if source_note:
            notes = state.setdefault("notes", [])
            if not notes or str(notes[-1]).strip() != source_note:
                notes.append(source_note)
        if not dry_run:
            save_executive_state(state)

    return state, changed
