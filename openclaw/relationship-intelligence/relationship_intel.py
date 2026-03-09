#!/usr/bin/env python3
"""Linus relationship-intelligence builder and query tool.

This script turns legacy relationship artifacts into a cleaner people graph,
renders searchable Markdown summaries for OpenClaw memory, and supports a few
live maintenance/query operations on the resulting SQLite store.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sqlite3
import sys
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterable


SPECIAL_CONTACTS = [
    {
        "canonical": "Prasad Rao",
        "aliases": ["prasad rao", "baba", "dad", "father"],
        "phones": ["+14167092606"],
        "emails": ["dprao28@gmail.com"],
        "relationship_label": "father",
        "category": "family",
        "cadence_days": 14,
        "notes": "Sunil's father. Toronto-based family context.",
    },
    {
        "canonical": "Uma Rao",
        "aliases": ["uma rao", "annu", "mom", "mother", "ma"],
        "phones": ["+14165001687"],
        "emails": ["umamrao@gmail.com"],
        "relationship_label": "mother",
        "category": "family",
        "cadence_days": 14,
        "notes": "Sunil's mother. Toronto-based family context.",
    },
    {
        "canonical": "Marie-Angelic Vendette",
        "aliases": ["angelic vendette", "angelic", "marie-angelic vendette"],
        "phones": ["+14156943567"],
        "emails": ["angelicvendette@gmail.com", "angelicvendette@icloud.com"],
        "relationship_label": "wife",
        "category": "family",
        "cadence_days": 7,
        "notes": "Sunil's wife. Preferred name: Angelic.",
    },
    {
        "canonical": "Rhaine Arongat",
        "aliases": ["rhaine arongat", "rhaine"],
        "emails": ["rhaine.arongat@tribble.ai"],
        "relationship_label": "executive assistant",
        "category": "operations",
        "cadence_days": 3,
        "notes": "Sunil's EA and real-world execution partner.",
    },
]

TIER_WEIGHT = {
    "family": 10.0,
    "inner_circle": 9.5,
    "close": 8.0,
    "active": 6.5,
    "peripheral": 4.0,
    "dormant": 2.0,
    "service_provider": 3.0,
    "operations": 8.0,
    "unassigned": 3.5,
    "unknown": 3.5,
}

CADENCE_BY_TIER = {
    "family": 14,
    "inner_circle": 21,
    "close": 45,
    "active": 90,
    "peripheral": 180,
    "dormant": 365,
    "service_provider": 180,
    "operations": 7,
    "unassigned": 180,
    "unknown": 180,
}

BAD_NAME_PATTERNS = [
    re.compile(r"^\[unknown\]", re.IGNORECASE),
    re.compile(r"^\d{3,}$"),
    re.compile(r"^(hi|hey|hello|yes|no)\b", re.IGNORECASE),
    re.compile(r"\b(don['’]t|important|context|excited|received|collaboration)\b", re.IGNORECASE),
]

DISPLAY_NAME_FALLBACK = "Unknown Contact"
TOPIC_ALLOWED_CATEGORIES = {
    "",
    "Business",
    "Finance",
    "Health",
    "Legal",
    "Personal",
    "Real Estate",
    "Technology",
    "Travel",
}
NOISY_NOTE_FRAGMENTS = (
    "source=whatsapp",
    "whatsapp_relationship=",
    "http://",
    "https://",
)
ORG_STOPWORDS = {
    "airport",
    "breakfast",
    "chicago",
    "denver",
    "dinner",
    "lunch",
    "micheal",
    "super bowl",
    "sunils",
    "the airport",
    "this stage",
}
LOW_SIGNAL_WORDS = {
    "a",
    "all",
    "am",
    "and",
    "any",
    "are",
    "at",
    "back",
    "by",
    "call",
    "coming",
    "did",
    "do",
    "done",
    "evening",
    "for",
    "free",
    "good",
    "hello",
    "hey",
    "hi",
    "how",
    "i",
    "if",
    "is",
    "it",
    "just",
    "know",
    "let",
    "look",
    "me",
    "message",
    "morning",
    "needed",
    "no",
    "not",
    "now",
    "ok",
    "okay",
    "soon",
    "speak",
    "sunil",
    "thank",
    "thanks",
    "that",
    "the",
    "this",
    "to",
    "tomorrow",
    "unfortunately",
    "up",
    "was",
    "we",
    "weekend",
    "well",
    "when",
    "very",
    "happy",
    "proud",
    "safe",
    "sleep",
    "same",
    "app",
    "meeting",
    "arrived",
    "looking",
    "food",
    "airport",
    "home",
    "you",
}
CORPORATE_MARKERS = {
    "ai",
    "aws",
    "capital",
    "company",
    "corp",
    "corporation",
    "fund",
    "group",
    "inc",
    "io",
    "labs",
    "llc",
    "lp",
    "partners",
    "studio",
    "systems",
    "technologies",
    "ventures",
}
KINSHIP_CONTRADICTIONS = {
    "father": {"daughter", "son", "sister", "brother", "wife", "husband"},
    "mother": {"daughter", "son", "sister", "brother", "wife", "husband"},
    "wife": {"daughter", "son", "mother", "father", "sister", "brother"},
    "executive assistant": {"daughter", "son", "mother", "father", "wife", "husband"},
}

CHANNEL_BY_PREFERENCE = {
    "sms": "text",
    "email": "email",
    "unknown": "the strongest existing channel",
}


@dataclass
class PersonRecord:
    person_id: str
    display_name: str
    canonical_name: str
    aliases: set[str] = field(default_factory=set)
    phones: set[str] = field(default_factory=set)
    emails: set[str] = field(default_factory=set)
    organizations: set[str] = field(default_factory=set)
    roles: set[str] = field(default_factory=set)
    tier: str = "unassigned"
    relationship_score: float = 0.0
    category: str = "network"
    relationship_label: str = ""
    preferred_channel: str = ""
    last_touch_date: str | None = None
    cadence_days: int = 180
    notes: list[str] = field(default_factory=list)
    dossier_excerpt: str = ""
    topics: list[str] = field(default_factory=list)
    open_actions: list[dict[str, Any]] = field(default_factory=list)
    important_dates: list[dict[str, Any]] = field(default_factory=list)
    source_contact_ids: list[int] = field(default_factory=list)
    importance: float = 0.0


def now_utc() -> datetime:
    return datetime.now(tz=timezone.utc).replace(microsecond=0)


def slugify(value: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    return slug or "contact"


def normalize_text(value: str | None) -> str:
    if not value:
        return ""
    return re.sub(r"\s+", " ", value).strip()


def normalize_name(name: str | None) -> str:
    return normalize_text(name).casefold()


def normalize_phone(value: str | None) -> str:
    if not value:
        return ""
    raw = re.sub(r"[^\d+]", "", value.strip())
    if raw.startswith("00"):
        raw = f"+{raw[2:]}"
    return raw


def parse_date(value: str | None) -> datetime | None:
    if not value:
        return None
    text = value.strip()
    for candidate in (text, text.replace("Z", "+00:00")):
        try:
            parsed = datetime.fromisoformat(candidate)
            if parsed.tzinfo is None:
                return parsed.replace(tzinfo=timezone.utc)
            return parsed.astimezone(timezone.utc)
        except ValueError:
            continue
    for fmt in ("%Y-%m-%d", "%Y/%m/%d"):
        try:
            return datetime.strptime(text, fmt).replace(tzinfo=timezone.utc)
        except ValueError:
            continue
    return None


def best_iso_date(existing: str | None, candidate: str | None) -> str | None:
    current_dt = parse_date(existing)
    new_dt = parse_date(candidate)
    if current_dt and new_dt:
        return candidate if new_dt >= current_dt else existing
    return candidate or existing


def dedupe_preserve(values: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for value in values:
        clean = normalize_text(value)
        key = clean.casefold()
        if clean and key not in seen:
            seen.add(key)
            ordered.append(clean)
    return ordered


def is_probably_noise_name(name: str | None) -> bool:
    clean = normalize_text(name)
    if not clean:
        return False
    if not re.search(r"[A-Za-z]", clean):
        return True
    for pattern in BAD_NAME_PATTERNS:
        if pattern.search(clean):
            return True
    words = clean.split()
    if len(words) >= 4 and clean.endswith(".") and sum(word[:1].isupper() for word in words) <= 1:
        return True
    if len(clean) > 40 and not re.search(r"[A-Z][a-z]+", clean):
        return True
    return False


def choose_display_name(current: str, candidate: str) -> str:
    current_clean = normalize_text(current)
    candidate_clean = normalize_text(candidate)
    if not current_clean:
        return candidate_clean or DISPLAY_NAME_FALLBACK
    if not candidate_clean:
        return current_clean
    current_noise = is_probably_noise_name(current_clean)
    candidate_noise = is_probably_noise_name(candidate_clean)
    if current_noise and not candidate_noise:
        return candidate_clean
    if candidate_noise and not current_noise:
        return current_clean
    if len(candidate_clean) > len(current_clean) and candidate_clean.count(" ") <= current_clean.count(" ") + 2:
        return candidate_clean
    return current_clean


def tier_rank(tier: str) -> int:
    order = [
        "family",
        "operations",
        "inner_circle",
        "close",
        "active",
        "peripheral",
        "dormant",
        "service_provider",
        "unassigned",
        "unknown",
    ]
    try:
        return order.index(tier)
    except ValueError:
        return len(order)


def normalize_tier(value: str | None) -> str:
    clean = normalize_text(value).lower().replace(" ", "_")
    if not clean:
        return "unassigned"
    aliases = {
        "ea": "operations",
        "exec_assistant": "operations",
        "inner": "inner_circle",
        "very_close": "inner_circle",
        "service": "service_provider",
        "archive": "dormant",
        "warm": "active",
        "friend": "close",
        "business": "active",
        "vendor": "service_provider",
        "acquaintance": "peripheral",
    }
    return aliases.get(clean, clean)


def preferred_channel(phones: set[str], emails: set[str]) -> str:
    if phones:
        return "sms"
    if emails:
        return "email"
    return "unknown"


def safe_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=True, sort_keys=False)


def words(value: str) -> list[str]:
    return re.findall(r"[A-Za-z][A-Za-z'&.-]*", value)


def has_corporate_shape(value: str) -> bool:
    tokens = words(value)
    if not tokens:
        return False
    lower_tokens = [token.lower() for token in tokens]
    if any(token in CORPORATE_MARKERS for token in lower_tokens):
        return True
    titled = sum(token[:1].isupper() or token.isupper() for token in tokens)
    return titled >= max(1, len(tokens) - 1)


def clean_org(value: str, category: str) -> str:
    clean = normalize_text(value)
    if not clean:
        return ""
    lowered = clean.casefold()
    if lowered in ORG_STOPWORDS:
        return ""
    if lowered.startswith(("the ", "this ", "that ", "latest ")):
        return ""
    if re.search(r"\b(he|she|they|him|her|them)\b", clean, re.IGNORECASE):
        return ""
    if "." in clean and not has_corporate_shape(clean):
        return ""
    if any(char.isdigit() for char in clean):
        return ""
    token_list = words(clean)
    if not token_list:
        return ""
    if len(token_list) > 4 and not has_corporate_shape(clean):
        return ""
    if category == "family" and not has_corporate_shape(clean):
        return ""
    if is_probably_noise_name(clean):
        return ""
    return clean


def clean_topic(name: str, category: str) -> str:
    clean = normalize_text(name)
    if not clean:
        return ""
    if category not in TOPIC_ALLOWED_CATEGORIES:
        return ""
    if "http" in clean.casefold():
        return ""
    if any(char.isdigit() for char in clean):
        return ""
    token_list = words(clean)
    if len(token_list) < 2 or len(token_list) > 5:
        return ""
    lowered = [token.casefold() for token in token_list]
    if lowered[0] in LOW_SIGNAL_WORDS:
        return ""
    if all(token in LOW_SIGNAL_WORDS for token in lowered):
        return ""
    if sum(token in LOW_SIGNAL_WORDS for token in lowered) >= max(2, len(lowered) - 1):
        return ""
    return clean


def clean_note(note: str, relationship_label: str) -> str:
    clean = normalize_text(note)
    if not clean:
        return ""
    lowered = clean.casefold()
    if any(fragment in lowered for fragment in NOISY_NOTE_FRAGMENTS):
        return ""
    if len(clean) > 420:
        return ""
    if "likely " in lowered:
        return ""
    contradictions = KINSHIP_CONTRADICTIONS.get(relationship_label, set())
    if any(word in lowered for word in contradictions):
        return ""
    return clean


def shorten_text(value: str, limit: int = 110) -> str:
    clean = normalize_text(value)
    if len(clean) <= limit:
        return clean
    truncated = clean[: limit - 1].rsplit(" ", 1)[0].rstrip(",;:")
    return f"{truncated}..."


def note_reason_fragment(note: str) -> str:
    clean = normalize_text(note)
    if not clean:
        return ""
    clean = re.sub(r"^(this|the) person is\s+", "", clean, flags=re.IGNORECASE)
    clean = re.sub(r"^sunil(?:'s)?\s+", "", clean, flags=re.IGNORECASE)
    clauses = re.split(r"[.;]\s+|\s+and\s+", clean)
    for clause in clauses:
        snippet = normalize_text(clause).rstrip(".")
        if not snippet:
            continue
        lowered = snippet.casefold()
        if "source=" in lowered or "relationship=" in lowered:
            continue
        return shorten_text(snippet[0].upper() + snippet[1:] if len(snippet) > 1 else snippet.upper(), 95)
    return shorten_text(clean, 95)


def dossier_reason_fragment(excerpt: str) -> str:
    clean = normalize_text(excerpt)
    if not clean:
        return ""
    company_match = re.search(r"\*\*Company:\*\*\s*([^#\n-]+)", excerpt)
    title_match = re.search(r"\*\*Title:\*\*\s*([^#\n-]+)", excerpt)
    if title_match and company_match:
        return shorten_text(
            f"{normalize_text(title_match.group(1))} at {normalize_text(company_match.group(1))}",
            95,
        )
    if company_match:
        return shorten_text(f"Current company: {normalize_text(company_match.group(1))}", 95)
    for line in excerpt.splitlines():
        stripped = normalize_text(line.lstrip("-"))
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("**"):
            continue
        return shorten_text(stripped, 95)
    return ""


def person_reason(record: PersonRecord) -> str:
    if record.relationship_label == "executive assistant":
        return "Executive assistant and human execution partner"
    if record.category == "family":
        if record.relationship_label:
            return f"Family: {record.relationship_label}"
        return "Immediate family"
    for note in display_notes(record):
        fragment = note_reason_fragment(note)
        if fragment:
            return fragment
    dossier_fragment = dossier_reason_fragment(record.dossier_excerpt)
    if dossier_fragment:
        return dossier_fragment
    orgs = display_organizations(record)
    if orgs:
        return f"Current context: {', '.join(orgs[:2])}"
    topics = display_topics(record)
    if topics:
        return f"Context: {', '.join(topics[:2])}"
    if record.roles:
        return f"Role: {', '.join(sorted(record.roles)[:2])}"
    return "High-signal relationship in Sunil's network"


def suggested_channel(record: PersonRecord) -> str:
    if record.category == "family":
        return "text or call if it is time-sensitive"
    if record.relationship_label == "executive assistant":
        return "Slack or email"
    return CHANNEL_BY_PREFERENCE.get(record.preferred_channel, "the strongest existing channel")


def why_now(record: PersonRecord, days_since: int | None, urgency: str) -> str:
    if record.open_actions:
        return f"Open loop: {record.open_actions[0]['description']}"
    if record.important_dates:
        upcoming = sorted(
            (item for item in (normalized_date_item(raw) for raw in record.important_dates) if item),
            key=lambda item: item.get("date") or "9999-12-31",
        )
        if upcoming:
            item = upcoming[0]
            label = item.get("date_type") or "date"
            if item.get("description"):
                return f"Upcoming {label}: {item['description']}"
            return f"Upcoming {label} on {item.get('date', 'unknown')}"
    if days_since is None:
        return "The relationship needs a fresh context check."
    if urgency == "urgent":
        return f"{days_since}d since last touch against a {record.cadence_days}d cadence"
    if urgency == "due":
        return f"Due for touchpoint: {days_since}d since last touch"
    if urgency == "watch":
        return f"Approaching cadence threshold at {days_since}d"
    return "No urgent trigger; keep warm"


def strategic_context_score(record: PersonRecord) -> float:
    score = 0.0
    if record.category == "family":
        return 3.0
    if record.relationship_label == "executive assistant":
        return 2.5
    note_blob = " ".join(display_notes(record)).casefold()
    if any(
        keyword in note_blob
        for keyword in (
            "founder",
            "investor",
            "operator",
            "friend",
            "colleague",
            "collaboration",
            "partner",
            "customer",
            "advisor",
            "board",
            "recruit",
        )
    ):
        score += 1.75
    if display_organizations(record):
        score += 0.6
    if display_topics(record):
        score += 0.4
    if not display_notes(record) and not display_organizations(record) and not display_topics(record):
        score -= 1.0
    return score


def reconnect_score(record: PersonRecord, days_since: int | None, urgency: str) -> float:
    score = record.importance + strategic_context_score(record)
    if days_since is not None:
        ratio = days_since / max(record.cadence_days, 1)
        score += min(ratio, 4.0) * 0.9
    if urgency == "urgent":
        score += 2.0
    elif urgency == "due":
        score += 1.0
    elif urgency == "watch":
        score += 0.4
    return round(score, 2)


def dedupe_radar_items(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    deduped: dict[str, dict[str, Any]] = {}
    for item in items:
        key = normalize_name(item.get("display_name")) or item.get("person_id") or safe_json(item)
        current = deduped.get(key)
        if not current:
            deduped[key] = item
            continue
        current_score = float(current.get("reconnect_score", current.get("importance", 0.0)))
        item_score = float(item.get("reconnect_score", item.get("importance", 0.0)))
        if item_score > current_score:
            deduped[key] = item
            continue
        if item_score == current_score and len(item.get("open_actions", [])) > len(current.get("open_actions", [])):
            deduped[key] = item
    return list(deduped.values())


def parse_embedded_year(value: str) -> int | None:
    match = re.search(r"\b(20\d{2})\b", value)
    if not match:
        return None
    return int(match.group(1))


def clean_action(action: dict[str, Any], category: str) -> dict[str, Any] | None:
    description = normalize_text(action.get("description"))
    if not description:
        return None
    lowered = description.casefold()
    if "http" in lowered:
        return None
    due_text = normalize_text(action.get("due_date"))
    due = parse_date(due_text)
    now = now_utc()
    if due and due < now - timedelta(days=180):
        return None
    embedded_year = parse_embedded_year(description)
    if embedded_year and embedded_year < now.year - 1:
        return None
    token_list = words(description)
    if not token_list:
        return None
    if len(token_list) > 12:
        return None
    if category != "family" and sum(token.casefold() in LOW_SIGNAL_WORDS for token in token_list) >= len(token_list) - 1:
        return None
    return {
        "description": description,
        "due_date": due_text,
        "status": normalize_text(action.get("status")) or "open",
        "priority": int(action.get("priority") or 3),
        "created_from": normalize_text(action.get("created_from")),
    }


def clean_important_date(item: dict[str, Any]) -> dict[str, Any] | None:
    date_text = normalize_text(item.get("date"))
    parsed = parse_date(date_text)
    if not parsed:
        return None
    now = now_utc()
    recurring = bool(item.get("recurring"))
    if recurring:
        if parsed.year < now.year - 2:
            parsed = parsed.replace(year=now.year)
            date_text = parsed.date().isoformat()
    elif parsed < now - timedelta(days=30):
        return None
    description = normalize_text(item.get("description"))
    if description and parse_embedded_year(description) and parse_embedded_year(description) < now.year - 1:
        return None
    if description and any(fragment in description.casefold() for fragment in NOISY_NOTE_FRAGMENTS):
        return None
    return {
        "date_type": normalize_text(item.get("date_type")) or "date",
        "date": date_text,
        "description": description,
        "recurring": recurring,
    }


def columns_for_table(conn: sqlite3.Connection, table: str) -> set[str]:
    rows = conn.execute(f"PRAGMA table_info({table})").fetchall()
    return {row[1] for row in rows}


def contact_column(columns: set[str], *candidates: str) -> str | None:
    for candidate in candidates:
        if candidate in columns:
            return candidate
    return None


def special_match(name: str, phone: str, email: str) -> dict[str, Any] | None:
    for special in SPECIAL_CONTACTS:
        aliases = {alias.casefold() for alias in special.get("aliases", [])}
        phones = {normalize_phone(item) for item in special.get("phones", [])}
        emails = {normalize_text(item).casefold() for item in special.get("emails", [])}
        if name and name in aliases:
            return special
        if phone and phone in phones:
            return special
        if email and email in emails:
            return special
    return None


def stable_person_key(
    display_name: str,
    email: str,
    phone: str,
    special: dict[str, Any] | None,
) -> str:
    if special:
        return slugify(special["canonical"])
    if email:
        return f"email-{slugify(email)}"
    if phone:
        return f"phone-{slugify(phone)}"
    return f"name-{slugify(display_name)}"


def choose_org(contact_row: sqlite3.Row, company_col: str | None) -> str:
    if company_col and company_col in contact_row.keys():
        return normalize_text(contact_row[company_col])
    return ""


def maybe_role(contact_row: sqlite3.Row) -> str:
    for candidate in ("role", "title"):
        if candidate in contact_row.keys():
            value = normalize_text(contact_row[candidate])
            if value:
                return value
    return ""


def compute_importance(record: PersonRecord) -> float:
    tier_value = TIER_WEIGHT.get(record.tier, TIER_WEIGHT["unassigned"])
    score_value = min(max(record.relationship_score, 0.0), 10.0)
    importance = tier_value * 0.65 + score_value * 0.75
    if record.category == "family":
        importance += 1.5
    if record.relationship_label == "executive assistant":
        importance += 1.0
    if record.open_actions:
        importance += min(len(record.open_actions), 3) * 0.3
    if record.last_touch_date:
        days_since = days_since_touch(record.last_touch_date)
        if days_since is not None and days_since > record.cadence_days:
            importance += min((days_since - record.cadence_days) / max(record.cadence_days, 1), 3.0)
    return round(importance, 2)


def days_since_touch(last_touch_date: str | None) -> int | None:
    parsed = parse_date(last_touch_date)
    if not parsed:
        return None
    return max((now_utc() - parsed).days, 0)


def urgency_band(record: PersonRecord) -> str:
    days_since = days_since_touch(record.last_touch_date)
    if record.open_actions:
        return "open-loop"
    if days_since is None:
        return "unknown"
    if days_since >= record.cadence_days * 2:
        return "urgent"
    if days_since >= record.cadence_days:
        return "due"
    if days_since >= math.floor(record.cadence_days * 0.7):
        return "watch"
    return "warm"


def short_summary(record: PersonRecord) -> str:
    parts: list[str] = []
    if record.relationship_label:
        parts.append(record.relationship_label)
    if record.roles:
        parts.append(f"role: {sorted(record.roles)[0]}")
    reason = person_reason(record)
    if reason and record.category not in {"family", "operations"}:
        parts.append(reason)
    visible_actions = display_actions(record)
    if visible_actions:
        parts.append(f"open loops: {len(visible_actions)}")
    return "; ".join(parts)


def display_organizations(record: PersonRecord) -> list[str]:
    if record.category in {"family", "operations"}:
        return []
    visible: list[str] = []
    for org in sorted(record.organizations):
        if not org:
            continue
        if not has_corporate_shape(org):
            continue
        visible.append(org)
        if len(visible) >= 3:
            break
    return visible


def display_topics(record: PersonRecord) -> list[str]:
    if record.category in {"family", "operations"}:
        return []
    return record.topics[:6]


def display_actions(record: PersonRecord) -> list[dict[str, Any]]:
    limit = 3 if record.category in {"family", "operations"} else 4
    return record.open_actions[:limit]


def display_notes(record: PersonRecord) -> list[str]:
    notes = record.notes
    if record.category == "family":
        notes = [note for note in notes if "likely" not in note.casefold()]
    return notes[:4]


def load_dossiers(dossiers_dir: Path | None) -> dict[str, str]:
    if not dossiers_dir or not dossiers_dir.exists():
        return {}
    dossiers: dict[str, str] = {}
    for path in sorted(dossiers_dir.glob("*.md")):
        text = path.read_text(encoding="utf-8", errors="ignore")
        excerpt = "\n".join(line.strip() for line in text.splitlines() if line.strip())[:1200]
        dossiers[slugify(path.stem)] = excerpt
    return dossiers


def build_seed(crm_db: Path, dossiers_dir: Path | None, out_dir: Path, top_n: int) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    db_out = out_dir / "relationship_intel.sqlite"
    if db_out.exists():
        db_out.unlink()

    source = sqlite3.connect(crm_db)
    source.row_factory = sqlite3.Row
    contacts_columns = columns_for_table(source, "contacts")
    name_col = contact_column(contacts_columns, "name", "full_name")
    tier_col = contact_column(contacts_columns, "tier", "relationship_tier")
    score_col = contact_column(contacts_columns, "relationship_score", "score")
    last_touch_col = contact_column(contacts_columns, "last_contact_date", "last_contact")
    phone_col = contact_column(contacts_columns, "phone")
    email_col = contact_column(contacts_columns, "email")
    notes_col = contact_column(contacts_columns, "notes")
    company_col = contact_column(contacts_columns, "company")

    org_rows: dict[int, list[str]] = {}
    if {"contact_organizations", "organizations"}.issubset(
        {
            row[0]
            for row in source.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()
        }
    ):
        for row in source.execute(
            """
            SELECT co.contact_id, o.name AS org_name
            FROM contact_organizations co
            JOIN organizations o ON o.id = co.org_id
            ORDER BY o.name
            """
        ):
            org_rows.setdefault(int(row["contact_id"]), []).append(normalize_text(row["org_name"]))

    topic_rows: dict[int, list[dict[str, str]]] = {}
    if {"contact_topics", "topics"}.issubset(
        {
            row[0]
            for row in source.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()
        }
    ):
        for row in source.execute(
            """
            SELECT ct.contact_id, t.name AS topic_name, COALESCE(t.category, '') AS topic_category
            FROM contact_topics ct
            JOIN topics t ON t.id = ct.topic_id
            ORDER BY COALESCE(ct.expertise_level, 0) DESC, t.name ASC
            """
        ):
            topic_rows.setdefault(int(row["contact_id"]), []).append(
                {
                    "name": normalize_text(row["topic_name"]),
                    "category": normalize_text(row["topic_category"]),
                }
            )

    action_rows: dict[int, list[dict[str, Any]]] = {}
    if "action_items" in {
        row[0] for row in source.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
    }:
        for row in source.execute(
            """
            SELECT contact_id, description, due_date, status, priority, created_from
            FROM action_items
            ORDER BY
              CASE status WHEN 'open' THEN 0 WHEN 'in_progress' THEN 1 ELSE 9 END,
              COALESCE(priority, 5) ASC,
              COALESCE(due_date, created_at, updated_at) ASC
            """
        ):
            action_rows.setdefault(int(row["contact_id"]), []).append(
                {
                    "description": normalize_text(row["description"]),
                    "due_date": normalize_text(row["due_date"]),
                    "status": normalize_text(row["status"]) or "open",
                    "priority": int(row["priority"]) if row["priority"] is not None else 3,
                    "created_from": normalize_text(row["created_from"]),
                }
            )

    date_rows: dict[int, list[dict[str, Any]]] = {}
    if "critical_dates" in {
        row[0] for row in source.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
    }:
        for row in source.execute(
            """
            SELECT contact_id, date_type, date, description, recurring
            FROM critical_dates
            ORDER BY date ASC
            """
        ):
            date_rows.setdefault(int(row["contact_id"]), []).append(
                {
                    "date_type": normalize_text(row["date_type"]),
                    "date": normalize_text(row["date"]),
                    "description": normalize_text(row["description"]),
                    "recurring": bool(row["recurring"]),
                }
            )

    dossiers = load_dossiers(dossiers_dir)
    merged: dict[str, PersonRecord] = {}

    query = "SELECT * FROM contacts"
    for row in source.execute(query):
        contact_id = int(row["id"])
        raw_name = normalize_text(row[name_col]) if name_col else ""
        raw_phone = normalize_phone(row[phone_col]) if phone_col else ""
        raw_email = normalize_text(row[email_col]).lower() if email_col else ""
        if not raw_name and not raw_phone and not raw_email:
            continue

        special = special_match(normalize_name(raw_name), raw_phone, raw_email)
        display_name = raw_name or (special["canonical"] if special else raw_email or raw_phone or DISPLAY_NAME_FALLBACK)
        person_key = stable_person_key(display_name, raw_email, raw_phone, special)
        if person_key not in merged:
            canonical_name = special["canonical"] if special else display_name
            merged[person_key] = PersonRecord(
                person_id=person_key,
                display_name=display_name or DISPLAY_NAME_FALLBACK,
                canonical_name=canonical_name or display_name or DISPLAY_NAME_FALLBACK,
                tier=special["category"] if special and special["category"] == "operations" else normalize_tier(row[tier_col]) if tier_col else "unassigned",
                category=special["category"] if special else "network",
                relationship_label=special["relationship_label"] if special else "",
                cadence_days=special["cadence_days"] if special else CADENCE_BY_TIER.get(normalize_tier(row[tier_col]) if tier_col else "unassigned", 180),
            )

        record = merged[person_key]
        if special:
            record.display_name = special["canonical"]
            record.canonical_name = special["canonical"]
        else:
            record.display_name = choose_display_name(record.display_name, display_name)
            record.canonical_name = choose_display_name(record.canonical_name, display_name)
        if raw_name:
            record.aliases.add(raw_name)
        if special:
            for alias in special.get("aliases", []):
                record.aliases.add(alias)
            if special.get("notes"):
                record.notes.append(special["notes"])
        if raw_phone:
            record.phones.add(raw_phone)
        if raw_email:
            record.emails.add(raw_email)
        org_value = clean_org(choose_org(row, company_col), record.category)
        if org_value:
            record.organizations.add(org_value)
        for org_value in org_rows.get(contact_id, []):
            cleaned_org = clean_org(org_value, record.category)
            if cleaned_org:
                record.organizations.add(cleaned_org)
        role_value = maybe_role(row)
        if role_value:
            record.roles.add(role_value)

        row_tier = special["category"] if special and special["category"] == "operations" else normalize_tier(row[tier_col]) if tier_col else "unassigned"
        if special and special["category"] == "family":
            row_tier = "family"
        if tier_rank(row_tier) < tier_rank(record.tier):
            record.tier = row_tier
        score_value = float(row[score_col]) if score_col and row[score_col] is not None else 0.0
        record.relationship_score = max(record.relationship_score, score_value)
        record.last_touch_date = best_iso_date(record.last_touch_date, normalize_text(row[last_touch_col]) if last_touch_col else None)
        if notes_col and row[notes_col]:
            record.notes.append(normalize_text(row[notes_col]))

        for topic in topic_rows.get(contact_id, []):
            cleaned_topic = clean_topic(topic.get("name", ""), topic.get("category", ""))
            if cleaned_topic:
                record.topics.append(cleaned_topic)
        for action in action_rows.get(contact_id, []):
            cleaned_action = clean_action(action, record.category)
            if cleaned_action:
                record.open_actions.append(cleaned_action)
        for date_info in date_rows.get(contact_id, []):
            cleaned_date = clean_important_date(date_info)
            if cleaned_date:
                record.important_dates.append(cleaned_date)
        record.source_contact_ids.append(contact_id)

        dossier_key = slugify(raw_name or raw_email or raw_phone)
        valid_dossier_keys = {slugify(record.canonical_name), slugify(record.display_name)}
        valid_dossier_keys.update(slugify(alias) for alias in record.aliases)
        if not record.dossier_excerpt and dossier_key in dossiers and dossier_key in valid_dossier_keys:
            record.dossier_excerpt = dossiers[dossier_key]

    source.close()

    for special in SPECIAL_CONTACTS:
        person_key = slugify(special["canonical"])
        if person_key in merged:
            continue
        record = PersonRecord(
            person_id=person_key,
            display_name=special["canonical"],
            canonical_name=special["canonical"],
            aliases=set(special.get("aliases", [])),
            phones=set(normalize_phone(item) for item in special.get("phones", [])),
            emails=set(normalize_text(item).lower() for item in special.get("emails", [])),
            tier="family" if special["category"] == "family" else "operations",
            relationship_score=10.0 if special["category"] in {"family", "operations"} else 8.0,
            category=special["category"],
            relationship_label=special["relationship_label"],
            cadence_days=special["cadence_days"],
            notes=[special.get("notes", "")],
        )
        merged[person_key] = record

    curated: list[PersonRecord] = []
    for record in merged.values():
        record.aliases = {
            alias
            for alias in dedupe_preserve(record.aliases)
            if alias.casefold() != record.display_name.casefold() and not is_probably_noise_name(alias)
        }
        record.topics = dedupe_preserve(record.topics)[:10]
        record.notes = [
            note for note in (clean_note(item, record.relationship_label) for item in dedupe_preserve(record.notes)) if note
        ][:6]
        record.open_actions = [
            action
            for action in record.open_actions
            if normalize_text(action.get("status")).lower() in {"", "open", "in_progress"}
        ][:6]
        record.important_dates = [item for item in record.important_dates if item][:6]
        record.source_contact_ids = sorted(set(record.source_contact_ids))
        if not record.relationship_label:
            if record.tier == "family":
                record.relationship_label = "family"
            elif record.tier == "operations":
                record.relationship_label = "operations"
            elif record.tier == "service_provider":
                record.relationship_label = "service provider"
        if not record.category:
            record.category = "network"
        if not record.preferred_channel:
            record.preferred_channel = preferred_channel(record.phones, record.emails)
        if record.category == "family":
            record.cadence_days = min(record.cadence_days, 14)
        elif record.relationship_label == "executive assistant":
            record.cadence_days = 7
        elif not record.cadence_days:
            record.cadence_days = CADENCE_BY_TIER.get(record.tier, 180)
        if record.category in {"family", "operations"}:
            record.dossier_excerpt = ""
        record.importance = compute_importance(record)

        display_noise = is_probably_noise_name(record.display_name)
        if display_noise and not (record.category == "family" or record.relationship_label == "executive assistant"):
            continue
        if record.tier == "service_provider" and not record.emails and any(phone.isdigit() and len(phone) <= 6 for phone in record.phones):
            continue
        curated.append(record)

    curated.sort(
        key=lambda item: (
            -item.importance,
            days_since_touch(item.last_touch_date) if days_since_touch(item.last_touch_date) is not None else 99999,
            item.display_name.lower(),
        )
    )

    dest = sqlite3.connect(db_out)
    dest.execute("PRAGMA journal_mode = WAL")
    dest.execute("PRAGMA foreign_keys = ON")
    dest.executescript(
        """
        CREATE TABLE people (
            person_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            canonical_name TEXT NOT NULL,
            aliases_json TEXT NOT NULL,
            phones_json TEXT NOT NULL,
            emails_json TEXT NOT NULL,
            organizations_json TEXT NOT NULL,
            roles_json TEXT NOT NULL,
            tier TEXT NOT NULL,
            relationship_score REAL NOT NULL,
            importance REAL NOT NULL,
            category TEXT NOT NULL,
            relationship_label TEXT NOT NULL,
            preferred_channel TEXT NOT NULL,
            last_touch_date TEXT,
            cadence_days INTEGER NOT NULL,
            notes_json TEXT NOT NULL,
            dossier_excerpt TEXT NOT NULL,
            topics_json TEXT NOT NULL,
            open_actions_json TEXT NOT NULL,
            important_dates_json TEXT NOT NULL,
            source_contact_ids_json TEXT NOT NULL,
            built_at TEXT NOT NULL
        );

        CREATE TABLE touch_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            person_id TEXT NOT NULL,
            touched_at TEXT NOT NULL,
            channel TEXT NOT NULL,
            note TEXT NOT NULL,
            direction TEXT NOT NULL DEFAULT 'manual',
            source TEXT NOT NULL DEFAULT 'linus',
            FOREIGN KEY (person_id) REFERENCES people (person_id) ON DELETE CASCADE
        );

        CREATE INDEX idx_people_importance ON people (importance DESC, relationship_score DESC);
        CREATE INDEX idx_people_last_touch ON people (last_touch_date);
        CREATE INDEX idx_touch_events_person_date ON touch_events (person_id, touched_at DESC);
        """
    )
    try:
        dest.execute(
            """
            CREATE VIRTUAL TABLE people_fts USING fts5(
                person_id UNINDEXED,
                display_name,
                canonical_name,
                aliases,
                emails,
                phones,
                organizations,
                roles,
                topics,
                notes
            )
            """
        )
    except sqlite3.OperationalError:
        pass

    fts_enabled = "people_fts" in {row[0] for row in dest.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    built_at = now_utc().isoformat()
    for record in curated:
        dest.execute(
            """
            INSERT INTO people (
                person_id, display_name, canonical_name, aliases_json, phones_json, emails_json,
                organizations_json, roles_json, tier, relationship_score, importance, category,
                relationship_label, preferred_channel, last_touch_date, cadence_days, notes_json,
                dossier_excerpt, topics_json, open_actions_json, important_dates_json,
                source_contact_ids_json, built_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                record.person_id,
                record.display_name,
                record.canonical_name,
                safe_json(dedupe_preserve(record.aliases)),
                safe_json(sorted(record.phones)),
                safe_json(sorted(record.emails)),
                safe_json(sorted(record.organizations)),
                safe_json(sorted(record.roles)),
                record.tier,
                record.relationship_score,
                record.importance,
                record.category,
                record.relationship_label,
                record.preferred_channel,
                record.last_touch_date,
                record.cadence_days,
                safe_json(record.notes),
                record.dossier_excerpt,
                safe_json(record.topics),
                safe_json(record.open_actions),
                safe_json(record.important_dates),
                safe_json(record.source_contact_ids),
                built_at,
            ),
        )
        if fts_enabled:
            dest.execute(
                """
                INSERT INTO people_fts (
                    person_id, display_name, canonical_name, aliases, emails, phones,
                    organizations, roles, topics, notes
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    record.person_id,
                    record.display_name,
                    record.canonical_name,
                    " ".join(dedupe_preserve(record.aliases)),
                    " ".join(sorted(record.emails)),
                    " ".join(sorted(record.phones)),
                    " ".join(sorted(record.organizations)),
                    " ".join(sorted(record.roles)),
                    " ".join(record.topics),
                    " ".join(record.notes),
                ),
            )
    dest.commit()
    dest.close()

    render_memory(db_out, out_dir / "memory", top_n=top_n)


def open_store(db_path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    return conn


def has_people_fts(conn: sqlite3.Connection) -> bool:
    row = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'people_fts' LIMIT 1"
    ).fetchone()
    return bool(row)


def fts_match_query(query: str) -> str:
    tokens = [re.sub(r"[^A-Za-z0-9+@._-]", "", token) for token in query.strip().split()]
    tokens = [token for token in tokens if token]
    if not tokens:
        return ""
    return " AND ".join(f'"{token}"*' for token in tokens)


def record_for_row(row: sqlite3.Row) -> PersonRecord:
    return PersonRecord(
        person_id=row["person_id"],
        display_name=row["display_name"],
        canonical_name=row["canonical_name"],
        aliases=set(json.loads(row["aliases_json"])),
        phones=set(json.loads(row["phones_json"])),
        emails=set(json.loads(row["emails_json"])),
        organizations=set(json.loads(row["organizations_json"])),
        roles=set(json.loads(row["roles_json"])),
        tier=row["tier"],
        relationship_score=float(row["relationship_score"]),
        importance=float(row["importance"]),
        category=row["category"],
        relationship_label=row["relationship_label"],
        preferred_channel=row["preferred_channel"],
        last_touch_date=row["last_touch_date"],
        cadence_days=int(row["cadence_days"]),
        notes=json.loads(row["notes_json"]),
        dossier_excerpt=row["dossier_excerpt"],
        topics=json.loads(row["topics_json"]),
        open_actions=json.loads(row["open_actions_json"]),
        important_dates=json.loads(row["important_dates_json"]),
        source_contact_ids=json.loads(row["source_contact_ids_json"]),
    )


def latest_touch(conn: sqlite3.Connection, row: sqlite3.Row) -> str | None:
    event = conn.execute(
        "SELECT touched_at FROM touch_events WHERE person_id = ? ORDER BY touched_at DESC LIMIT 1",
        (row["person_id"],),
    ).fetchone()
    if event:
        return best_iso_date(row["last_touch_date"], event["touched_at"])
    return row["last_touch_date"]


def search_people(db_path: Path, query: str, limit: int) -> list[dict[str, Any]]:
    conn = open_store(db_path)
    rows: list[sqlite3.Row] = []
    if has_people_fts(conn):
        try:
            match = fts_match_query(query)
            if match:
                rows = conn.execute(
                    """
                    SELECT p.*, bm25(people_fts) AS rank
                    FROM people_fts
                    JOIN people p ON p.person_id = people_fts.person_id
                    WHERE people_fts MATCH ?
                    ORDER BY rank ASC, p.importance DESC, p.relationship_score DESC
                    LIMIT ?
                    """,
                    (match, limit),
                ).fetchall()
        except sqlite3.OperationalError:
            rows = []
    if not rows:
        q = f"%{query.strip().lower()}%"
        rows = conn.execute(
            """
            SELECT *
            FROM people
            WHERE lower(display_name) LIKE ?
               OR lower(canonical_name) LIKE ?
               OR lower(aliases_json) LIKE ?
               OR lower(emails_json) LIKE ?
               OR lower(phones_json) LIKE ?
               OR lower(organizations_json) LIKE ?
               OR lower(roles_json) LIKE ?
               OR lower(topics_json) LIKE ?
               OR lower(notes_json) LIKE ?
            ORDER BY importance DESC, relationship_score DESC
            LIMIT ?
            """,
            (q, q, q, q, q, q, q, q, q, limit),
        ).fetchall()
    results: list[dict[str, Any]] = []
    for row in rows:
        last_touch = latest_touch(conn, row)
        record = record_for_row(row)
        results.append(
            {
                "person_id": row["person_id"],
                "display_name": row["display_name"],
                "tier": row["tier"],
                "category": row["category"],
                "relationship_label": row["relationship_label"],
                "importance": row["importance"],
                "relationship_score": row["relationship_score"],
                "last_touch_date": last_touch,
                "days_since_touch": days_since_touch(last_touch),
                "why_they_matter": person_reason(record),
                "summary": short_summary(record),
            }
        )
    conn.close()
    return results


def resolve_person(db_path: Path, query: str) -> sqlite3.Row | None:
    conn = open_store(db_path)
    q = f"%{query.strip().lower()}%"
    row = conn.execute(
        """
        SELECT *
        FROM people
        WHERE lower(display_name) = lower(?)
           OR lower(canonical_name) = lower(?)
           OR person_id = ?
           OR lower(aliases_json) LIKE ?
           OR lower(display_name) LIKE ?
           OR lower(canonical_name) LIKE ?
        ORDER BY
          CASE
            WHEN lower(display_name) = lower(?) THEN 0
            WHEN lower(canonical_name) = lower(?) THEN 1
            ELSE 2
          END,
          importance DESC
        LIMIT 1
        """,
        (query, query, query, q, q, q, query, query),
    ).fetchone()
    if not row and has_people_fts(conn):
        try:
            match = fts_match_query(query)
            if match:
                row = conn.execute(
                    """
                    SELECT p.*, bm25(people_fts) AS rank
                    FROM people_fts
                    JOIN people p ON p.person_id = people_fts.person_id
                    WHERE people_fts MATCH ?
                    ORDER BY rank ASC, p.importance DESC
                    LIMIT 1
                    """,
                    (match,),
                ).fetchone()
        except sqlite3.OperationalError:
            row = None
    conn.close()
    return row


def summarize_person(db_path: Path, query: str) -> dict[str, Any] | None:
    row = resolve_person(db_path, query)
    if not row:
        return None
    return summarize_row(db_path, row)


def summarize_row(db_path: Path, row: sqlite3.Row) -> dict[str, Any]:
    conn = open_store(db_path)
    latest = latest_touch(conn, row)
    manual_touches = conn.execute(
        """
        SELECT touched_at, channel, note, direction, source
        FROM touch_events
        WHERE person_id = ?
        ORDER BY touched_at DESC
        LIMIT 10
        """,
        (row["person_id"],),
    ).fetchall()
    conn.close()
    record = record_for_row(row)
    visible_orgs = display_organizations(record)
    visible_topics = display_topics(record)
    visible_actions = display_actions(record)
    visible_notes = display_notes(record)
    days = days_since_touch(latest)
    urgency = urgency_band(record)
    return {
        "person_id": record.person_id,
        "display_name": record.display_name,
        "canonical_name": record.canonical_name,
        "aliases": sorted(record.aliases),
        "phones": sorted(record.phones),
        "emails": sorted(record.emails),
        "organizations": visible_orgs,
        "roles": sorted(record.roles),
        "tier": record.tier,
        "category": record.category,
        "relationship_label": record.relationship_label,
        "importance": record.importance,
        "relationship_score": record.relationship_score,
        "preferred_channel": record.preferred_channel,
        "last_touch_date": latest,
        "days_since_touch": days,
        "cadence_days": record.cadence_days,
        "urgency": urgency,
        "why_they_matter": person_reason(record),
        "why_now": why_now(record, days, urgency),
        "suggested_channel": suggested_channel(record),
        "topics": visible_topics,
        "open_actions": visible_actions,
        "important_dates": record.important_dates,
        "notes": visible_notes,
        "dossier_excerpt": record.dossier_excerpt,
        "source_contact_ids": record.source_contact_ids,
        "manual_touches": [dict(item) for item in manual_touches],
    }


def is_high_signal(record: PersonRecord, days_since: int | None) -> bool:
    if record.category in {"family", "operations"}:
        return True
    if record.tier == "inner_circle":
        return True
    if record.tier == "close" and record.relationship_score >= 4.0:
        return True
    if record.relationship_score >= 8.0 and days_since is not None and days_since <= 365:
        return True
    if record.importance >= 11.0 and record.relationship_score >= 6.0 and days_since is not None and days_since <= 180:
        return True
    return False


def is_recent_enough(record: PersonRecord, days_since: int | None) -> bool:
    if record.category in {"family", "operations"}:
        return True
    if days_since is None:
        return False
    if record.tier in {"inner_circle", "close"}:
        return days_since <= 540
    return days_since <= 365


def keep_date_item(item: dict[str, Any]) -> bool:
    return normalized_date_item(item) is not None


def normalized_date_item(item: dict[str, Any]) -> dict[str, Any] | None:
    parsed = parse_date(item.get("date"))
    if not parsed:
        return None
    now = now_utc()
    if bool(item.get("recurring")):
        while parsed < now - timedelta(days=3):
            try:
                parsed = parsed.replace(year=parsed.year + 1)
            except ValueError:
                parsed = parsed + timedelta(days=365)
    if parsed < now - timedelta(days=3):
        return None
    if parsed > now + timedelta(days=120):
        return None
    normalized = dict(item)
    normalized["date"] = parsed.date().isoformat()
    return normalized


def ranked_people(conn: sqlite3.Connection, limit: int) -> list[PersonRecord]:
    rows = conn.execute(
        "SELECT * FROM people ORDER BY importance DESC, relationship_score DESC LIMIT ?",
        (limit,),
    ).fetchall()
    return [record_for_row(row) for row in rows]


def relationship_radar(db_path: Path, limit: int = 12) -> dict[str, list[dict[str, Any]]]:
    conn = open_store(db_path)
    rows = conn.execute("SELECT * FROM people").fetchall()
    priority_reconnect: list[dict[str, Any]] = []
    maintain_warmth: list[dict[str, Any]] = []
    open_loops: list[dict[str, Any]] = []
    important_dates: list[dict[str, Any]] = []
    for row in rows:
        last_touch = latest_touch(conn, row)
        record = record_for_row(row)
        record.last_touch_date = last_touch
        days = days_since_touch(last_touch)
        urgency = urgency_band(record)
        if not is_high_signal(record, days):
            continue
        if not is_recent_enough(record, days) and not record.open_actions:
            continue
        item = {
            "person_id": record.person_id,
            "display_name": record.display_name,
            "tier": record.tier,
            "importance": record.importance,
            "relationship_score": record.relationship_score,
            "last_touch_date": last_touch,
            "days_since_touch": days,
            "cadence_days": record.cadence_days,
            "urgency": urgency,
            "why_they_matter": person_reason(record),
            "why_now": why_now(record, days, urgency),
            "suggested_channel": suggested_channel(record),
            "recommended_action": recommended_action(record, days, urgency),
            "summary": short_summary(record),
            "reconnect_score": reconnect_score(record, days, urgency),
        }
        if record.open_actions:
            filtered_actions = []
            for action in record.open_actions:
                due = parse_date(action.get("due_date"))
                if not due or due >= now_utc() - timedelta(days=180):
                    filtered_actions.append(action)
            if filtered_actions:
                open_loops.append(
                    {
                        **item,
                        "open_actions": filtered_actions[:3],
                    }
                )
        if record.important_dates:
            for date_info in record.important_dates[:2]:
                normalized_date = normalized_date_item(date_info)
                if not normalized_date:
                    continue
                important_dates.append(
                    {
                        **item,
                        "date_type": normalized_date.get("date_type") or "date",
                        "date": normalized_date.get("date"),
                        "date_description": normalized_date.get("description"),
                    }
                )
        if urgency in {"urgent", "due"} and record.importance >= 7.5:
            priority_reconnect.append(item)
        elif urgency in {"watch", "warm"} and record.importance >= 7.0:
            maintain_warmth.append(item)

    conn.close()
    priority_reconnect = dedupe_radar_items(priority_reconnect)
    maintain_warmth = dedupe_radar_items(maintain_warmth)
    open_loops = dedupe_radar_items(open_loops)
    important_dates = dedupe_radar_items(important_dates)
    sort_key = lambda item: (
        {"urgent": 0, "due": 1, "open-loop": 2, "watch": 3, "warm": 4, "unknown": 5}.get(item["urgency"], 9),
        -float(item.get("reconnect_score", item["importance"])),
        -(item["days_since_touch"] or 0),
    )
    priority_reconnect.sort(key=sort_key)
    maintain_warmth.sort(key=sort_key)
    open_loops.sort(key=sort_key)
    important_dates.sort(key=lambda item: item.get("date") or "9999-12-31")
    return {
        "priority_reconnect": priority_reconnect[:limit],
        "maintain_warmth": maintain_warmth[:limit],
        "open_loops": open_loops[:limit],
        "important_dates": important_dates[:limit],
        "generated_at": [now_utc().isoformat()],
    }


def weekly_brief(
    db_path: Path,
    reconnect_limit: int = 5,
    loop_limit: int = 5,
    date_limit: int = 3,
) -> dict[str, Any]:
    radar = relationship_radar(db_path, limit=max(reconnect_limit, loop_limit, date_limit, 8))
    anchor_names = [
        "Prasad Rao",
        "Uma Rao",
        "Marie-Angelic Vendette",
        "Rhaine Arongat",
    ]
    anchors: list[dict[str, Any]] = []
    for name in anchor_names:
        summary = summarize_person(db_path, name)
        if not summary:
            continue
        anchors.append(
            {
                "display_name": summary["display_name"],
                "relationship": summary["relationship_label"] or summary["tier"],
                "last_touch_date": summary["last_touch_date"],
                "why_they_matter": summary["why_they_matter"],
                "open_loop": summary["open_actions"][0]["description"] if summary["open_actions"] else "",
            }
        )
    return {
        "generated_at": radar["generated_at"][0],
        "core_anchors": anchors,
        "priority_reconnect": radar["priority_reconnect"][:reconnect_limit],
        "open_loops": radar["open_loops"][:loop_limit],
        "important_dates": radar["important_dates"][:date_limit],
    }


def prompt_surface_block(db_path: Path, reconnect_limit: int = 5, loop_limit: int = 5) -> str:
    brief = weekly_brief(db_path, reconnect_limit=reconnect_limit, loop_limit=loop_limit, date_limit=3)
    lines = [
        "## Relationship Radar Snapshot",
        "",
        f"- Refreshed: {brief['generated_at']}",
        "- Full detail lives in `memory/RELATIONSHIP-RADAR.md`, `memory/PEOPLE-INDEX.md`, and `memory/people/*.md`.",
        "",
        "### Core Anchors",
        "",
    ]
    for anchor in brief["core_anchors"]:
        open_loop = anchor["open_loop"] or anchor["why_they_matter"]
        last_touch = anchor["last_touch_date"] or "unknown"
        lines.append(
            f"- {anchor['display_name']} — {anchor['relationship']}; last touch {last_touch}; {open_loop}"
        )

    lines.extend(["", "### Priority Reconnect", ""])
    for item in brief["priority_reconnect"][:reconnect_limit]:
        days = item["days_since_touch"]
        cadence = item["cadence_days"]
        lines.append(
            f"- {item['display_name']} — {item['why_they_matter']}. {days if days is not None else 'unknown'}d since touch vs {cadence}d target. {item['recommended_action']}"
        )
    if not brief["priority_reconnect"][:reconnect_limit]:
        lines.append("- None currently.")

    lines.extend(["", "### Open Loops", ""])
    for item in brief["open_loops"][:loop_limit]:
        action = item["open_actions"][0]["description"] if item.get("open_actions") else item["recommended_action"]
        lines.append(f"- {item['display_name']} — {action}")
    if not brief["open_loops"][:loop_limit]:
        lines.append("- None currently.")

    lines.extend(["", "### Upcoming Moments", ""])
    for item in brief["important_dates"]:
        label = item.get("date_type") or "date"
        reason = item.get("date_description") or item.get("why_they_matter") or ""
        lines.append(f"- {item['display_name']} — {label} on {item.get('date', 'unknown')}. {reason}".rstrip())
    if not brief["important_dates"]:
        lines.append("- None currently.")

    lines.extend(
        [
            "",
            "### Operating Rule",
            "",
            "- Do not guess about network details when the relationship-intelligence toolchain can answer them; query it directly and keep this snapshot in sync.",
        ]
    )
    return "\n".join(lines)


def recommended_action(record: PersonRecord, days_since: int | None, urgency: str) -> str:
    if record.open_actions:
        return f"Close the open loop: {record.open_actions[0]['description']}"
    if record.category == "family":
        return "Check in personally and keep family context fresh."
    if record.relationship_label == "executive assistant":
        return "Review current handoffs, waiting-fors, and operations context."
    if days_since is None:
        return "Confirm the right channel and refresh the relationship context."
    if urgency == "urgent":
        return f"Draft a proactive {suggested_channel(record)} outreach note and send or hand off."
    if urgency == "due":
        return f"Prepare a concise {suggested_channel(record)} touchpoint or follow-up this week."
    if urgency == "watch":
        return "Keep warm; line up a lightweight nudge or check-in."
    return "No immediate action required; maintain awareness."


def write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content.rstrip() + "\n", encoding="utf-8")


def person_markdown(data: dict[str, Any]) -> str:
    lines = [
        f"# {data['display_name']}",
        "",
        f"- Canonical name: {data['canonical_name']}",
        f"- Tier: {data['tier']}",
        f"- Category: {data['category']}",
        f"- Relationship: {data['relationship_label'] or 'network'}",
        f"- Preferred channel: {data['preferred_channel']}",
        f"- Importance: {data['importance']}",
        f"- Relationship score: {data['relationship_score']}",
        f"- Last touch: {data['last_touch_date'] or 'unknown'}",
        f"- Days since touch: {data['days_since_touch'] if data['days_since_touch'] is not None else 'unknown'}",
        f"- Cadence target: every {data['cadence_days']} days",
        f"- Urgency: {data['urgency']}",
        f"- Why they matter: {data['why_they_matter']}",
        f"- Why now: {data['why_now']}",
        f"- Suggested channel: {data['suggested_channel']}",
        "",
    ]
    if data["aliases"]:
        lines.extend(["## Aliases", "", ", ".join(data["aliases"]), ""])
    if data["phones"] or data["emails"]:
        lines.extend(["## Contact", ""])
        if data["phones"]:
            lines.append(f"- Phones: {', '.join(data['phones'])}")
        if data["emails"]:
            lines.append(f"- Emails: {', '.join(data['emails'])}")
        lines.append("")
    if data["organizations"] or data["roles"]:
        lines.extend(["## Current Context", ""])
        if data["organizations"]:
            lines.append(f"- Organizations: {', '.join(data['organizations'])}")
        if data["roles"]:
            lines.append(f"- Roles: {', '.join(data['roles'])}")
        lines.append("")
    if data["topics"]:
        lines.extend(["## Topics", "", ", ".join(data["topics"]), ""])
    if data["open_actions"]:
        lines.extend(["## Open Loops", ""])
        for action in data["open_actions"][:8]:
            due = f" (due {action['due_date']})" if action.get("due_date") else ""
            lines.append(f"- [{action['status']}] {action['description']}{due}")
        lines.append("")
    if data["important_dates"]:
        lines.extend(["## Important Dates", ""])
        for item in data["important_dates"]:
            desc = f" - {item['description']}" if item.get("description") else ""
            lines.append(f"- {item.get('date_type', 'date')}: {item.get('date', 'unknown')}{desc}")
        lines.append("")
    if data["notes"]:
        lines.extend(["## Notes", ""])
        for note in data["notes"][:8]:
            lines.append(f"- {note}")
        lines.append("")
    if data["manual_touches"]:
        lines.extend(["## Recent Manual Touches", ""])
        for touch in data["manual_touches"][:5]:
            lines.append(f"- {touch['touched_at']} via {touch['channel']}: {touch['note']}")
        lines.append("")
    if data["dossier_excerpt"]:
        lines.extend(["## Dossier Excerpt", "", data["dossier_excerpt"], ""])
    return "\n".join(lines)


def render_memory(db_path: Path, memory_dir: Path, top_n: int) -> None:
    memory_dir.mkdir(parents=True, exist_ok=True)
    people_dir = memory_dir / "people"
    people_dir.mkdir(parents=True, exist_ok=True)
    conn = open_store(db_path)
    rows = conn.execute(
        "SELECT * FROM people ORDER BY importance DESC, relationship_score DESC LIMIT ?",
        (top_n * 3,),
    ).fetchall()
    index_lines = [
        "# Relationship Intelligence Index",
        "",
        "This is Linus's indexed people graph. Use the `relationship-intel` tooling for deeper queries and updates.",
        "",
        f"- Generated at: {now_utc().isoformat()}",
        f"- Key people rendered: {len(rows)}",
        "",
        "## Highest-signal people",
        "",
    ]
    rendered = 0
    for row in rows:
        summary = summarize_row(db_path, row)
        if is_probably_noise_name(summary["display_name"]) and summary["category"] not in {"family", "operations"}:
            continue
        if not is_high_signal(record_for_row(row), summary["days_since_touch"]):
            continue
        slug = slugify(summary["display_name"])
        write_text(people_dir / f"{slug}.md", person_markdown(summary))
        index_lines.append(
            f"- {summary['display_name']} — {summary['tier']}, last touch {summary['last_touch_date'] or 'unknown'}, urgency {summary['urgency']}"
        )
        rendered += 1
        if rendered >= top_n:
            break
    write_text(memory_dir / "PEOPLE-INDEX.md", "\n".join(index_lines))

    radar = relationship_radar(db_path, limit=12)
    radar_lines = [
        "# Relationship Radar",
        "",
        f"Generated at {radar['generated_at'][0]}",
        "",
        "## Priority Reconnect",
        "",
    ]
    for item in radar["priority_reconnect"] or []:
        radar_lines.append(
            f"- {item['display_name']} — {item['why_they_matter']}. {item['days_since_touch']}d since touch vs target {item['cadence_days']}d. {item['recommended_action']}"
        )
    if not radar["priority_reconnect"]:
        radar_lines.append("- None right now.")
    radar_lines.extend(["", "## Maintain Warmth", ""])
    for item in radar["maintain_warmth"] or []:
        radar_lines.append(f"- {item['display_name']} — {item['why_they_matter']}. {item['recommended_action']}")
    if not radar["maintain_warmth"]:
        radar_lines.append("- None right now.")
    radar_lines.extend(["", "## Open Loops", ""])
    for item in radar["open_loops"] or []:
        action = item["open_actions"][0]["description"] if item.get("open_actions") else item["recommended_action"]
        radar_lines.append(f"- {item['display_name']} — {action} ({item['why_they_matter']})")
    if not radar["open_loops"]:
        radar_lines.append("- None right now.")
    radar_lines.extend(["", "## Important Dates", ""])
    for item in radar["important_dates"] or []:
        radar_lines.append(
            f"- {item['display_name']} — {item['date_type']} on {item['date']}. {item.get('date_description') or ''}".rstrip()
        )
    if not radar["important_dates"]:
        radar_lines.append("- None captured yet.")
    write_text(memory_dir / "RELATIONSHIP-RADAR.md", "\n".join(radar_lines))

    overview = "\n".join(
        [
            "# Relationship Intelligence",
            "",
            "Linus uses this subsystem to stay aware of Sunil's network, maintain warm relationships,",
            "track open loops, and suggest proactive outreach.",
            "",
            "## Ground Rules",
            "",
            "- This is not a generic CRM. It is a relationship-awareness layer.",
            "- Prefer the person summaries in `memory/people/` and the live radar in `memory/RELATIONSHIP-RADAR.md`.",
            "- For deeper lookup or updates, use `python3 /root/.openclaw/workspace/relationship-intel/relationship_intel.py ...`.",
            "- When Sunil mentions a meaningful interaction, record it with `touch` so the graph stays current.",
            "",
            "## Primary commands",
            "",
            "- `search <query>`",
            "- `summary <person-or-alias>`",
            "- `brief`",
            "- `radar`",
            "- `touch <person-or-alias> --note \"...\" --channel slack|telegram|email|sms|phone|in-person`",
            "",
        ]
    )
    write_text(memory_dir / "RELATIONSHIP-INTELLIGENCE.md", overview)
    conn.close()


def touch_person(db_path: Path, query: str, note: str, channel: str, touched_at: str | None) -> dict[str, Any]:
    row = resolve_person(db_path, query)
    if not row:
        raise SystemExit(f"No person found for query: {query}")
    when = touched_at or now_utc().isoformat()
    conn = open_store(db_path)
    conn.execute(
        """
        INSERT INTO touch_events (person_id, touched_at, channel, note, direction, source)
        VALUES (?, ?, ?, ?, 'manual', 'linus')
        """,
        (row["person_id"], when, channel, note),
    )
    conn.commit()
    conn.close()
    summary = summarize_person(db_path, row["person_id"])
    if not summary:
        raise SystemExit("Touch recorded but summary lookup failed.")
    return summary


def maybe_rerender_after_touch(db_path: Path, memory_dir: Path | None, top_n: int) -> Path | None:
    if not memory_dir:
        return None
    target = memory_dir.expanduser().resolve()
    render_memory(db_path, target, top_n)
    return target


def stats(db_path: Path) -> dict[str, Any]:
    conn = open_store(db_path)
    total = conn.execute("SELECT COUNT(*) AS c FROM people").fetchone()["c"]
    counts = conn.execute(
        """
        SELECT tier, COUNT(*) AS c
        FROM people
        GROUP BY tier
        ORDER BY c DESC, tier ASC
        """
    ).fetchall()
    conn.close()
    return {"total_people": total, "tiers": {row["tier"]: row["c"] for row in counts}}


def print_json(payload: Any) -> None:
    print(json.dumps(payload, indent=2, ensure_ascii=True))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Linus relationship-intelligence tool")
    parser.add_argument(
        "--db",
        default=str(Path(__file__).resolve().with_name("relationship_intel.sqlite")),
        help="Path to relationship_intel.sqlite",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    build = subparsers.add_parser("build", help="Build a cleaned relationship graph from legacy CRM data")
    build.add_argument("--crm-db", required=True, help="Path to source CRM SQLite DB")
    build.add_argument("--dossiers-dir", help="Optional path to dossier markdown files")
    build.add_argument("--out-dir", required=True, help="Output directory for the built graph and rendered memory")
    build.add_argument("--top-n", type=int, default=250, help="Number of people summaries to render")

    search = subparsers.add_parser("search", help="Search the relationship graph")
    search.add_argument("query", help="Name, alias, topic, org, phone, or email")
    search.add_argument("--limit", type=int, default=10)
    search.add_argument("--json", action="store_true")

    summary = subparsers.add_parser("summary", help="Summarize one person")
    summary.add_argument("query", help="Name, alias, email, or phone")
    summary.add_argument("--json", action="store_true")

    radar = subparsers.add_parser("radar", help="Generate a relationship radar snapshot")
    radar.add_argument("--limit", type=int, default=12)
    radar.add_argument("--json", action="store_true")

    brief = subparsers.add_parser("brief", help="Generate a compact weekly relationship brief")
    brief.add_argument("--reconnect-limit", type=int, default=5)
    brief.add_argument("--loop-limit", type=int, default=5)
    brief.add_argument("--date-limit", type=int, default=3)
    brief.add_argument("--json", action="store_true")

    render = subparsers.add_parser("render", help="Render memory markdown from the existing graph")
    render.add_argument("--memory-dir", required=True, help="Target memory directory")
    render.add_argument("--top-n", type=int, default=250)

    prompt_block = subparsers.add_parser("prompt-block", help="Render a compact relationship snapshot for prompt surface files")
    prompt_block.add_argument("--reconnect-limit", type=int, default=5)
    prompt_block.add_argument("--loop-limit", type=int, default=5)

    touch = subparsers.add_parser("touch", help="Record a manual touchpoint")
    touch.add_argument("query", help="Person name or alias")
    touch.add_argument("--note", required=True, help="What happened")
    touch.add_argument("--channel", default="manual", help="email|slack|telegram|sms|phone|in-person|manual")
    touch.add_argument("--touched-at", help="Override timestamp in ISO 8601")
    touch.add_argument("--memory-dir", help="Optional memory directory to re-render after recording the touch")
    touch.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    touch.add_argument("--json", action="store_true")

    stats_cmd = subparsers.add_parser("stats", help="Show graph stats")
    stats_cmd.add_argument("--json", action="store_true")

    args = parser.parse_args(argv)
    db_path = Path(args.db).expanduser().resolve()

    if args.command == "build":
        build_seed(
            crm_db=Path(args.crm_db).expanduser().resolve(),
            dossiers_dir=Path(args.dossiers_dir).expanduser().resolve() if args.dossiers_dir else None,
            out_dir=Path(args.out_dir).expanduser().resolve(),
            top_n=args.top_n,
        )
        print(f"Built relationship graph at {Path(args.out_dir).expanduser().resolve()}")
        return 0

    if args.command == "search":
        payload = search_people(db_path, args.query, args.limit)
        if args.json:
            print_json(payload)
        else:
            for item in payload:
                print(
                    f"{item['display_name']} | tier={item['tier']} | last_touch={item['last_touch_date'] or 'unknown'} | "
                    f"context={item['summary']}"
                )
        return 0

    if args.command == "summary":
        payload = summarize_person(db_path, args.query)
        if not payload:
            print(f"No person found for query: {args.query}", file=sys.stderr)
            return 1
        if args.json:
            print_json(payload)
        else:
            print(person_markdown(payload))
        return 0

    if args.command == "radar":
        payload = relationship_radar(db_path, args.limit)
        if args.json:
            print_json(payload)
        else:
            print("# Relationship Radar")
            print()
            for section in ("priority_reconnect", "maintain_warmth", "open_loops", "important_dates"):
                print(f"## {section.replace('_', ' ').title()}")
                items = payload.get(section) or []
                if not items:
                    print("- None")
                else:
                    for item in items:
                        print(f"- {item['display_name']} — {item['recommended_action']}")
                print()
        return 0

    if args.command == "brief":
        payload = weekly_brief(db_path, args.reconnect_limit, args.loop_limit, args.date_limit)
        if args.json:
            print_json(payload)
        else:
            print("# Weekly Relationship Brief")
            print()
            print("## Priority Reconnect")
            items = payload.get("priority_reconnect") or []
            if not items:
                print("- None")
            else:
                for item in items:
                    print(f"- {item['display_name']} — {item['why_they_matter']}. {item['why_now']}. {item['recommended_action']}")
            print()
            print("## Open Loops")
            loops = payload.get("open_loops") or []
            if not loops:
                print("- None")
            else:
                for item in loops:
                    action = item["open_actions"][0]["description"] if item.get("open_actions") else item["recommended_action"]
                    print(f"- {item['display_name']} — {action}")
            print()
            print("## Upcoming Moments")
            upcoming = payload.get("important_dates") or []
            if not upcoming:
                print("- None")
            else:
                for item in upcoming:
                    print(f"- {item['display_name']} — {item.get('date_type') or 'date'} on {item.get('date', 'unknown')}")
        return 0

    if args.command == "render":
        render_memory(db_path, Path(args.memory_dir).expanduser().resolve(), args.top_n)
        print(f"Rendered memory markdown into {Path(args.memory_dir).expanduser().resolve()}")
        return 0

    if args.command == "prompt-block":
        print(prompt_surface_block(db_path, args.reconnect_limit, args.loop_limit))
        return 0

    if args.command == "touch":
        payload = touch_person(db_path, args.query, args.note, args.channel, args.touched_at)
        rerendered = maybe_rerender_after_touch(
            db_path,
            Path(args.memory_dir) if getattr(args, "memory_dir", None) else None,
            args.top_n,
        )
        if args.json:
            response = {"person": payload}
            if rerendered:
                response["rerendered_memory_dir"] = str(rerendered)
            print_json(response)
        else:
            print(f"Recorded touchpoint for {payload['display_name']}")
            if rerendered:
                print(f"Re-rendered memory into {rerendered}")
        return 0

    if args.command == "stats":
        payload = stats(db_path)
        if args.json:
            print_json(payload)
        else:
            print(f"People: {payload['total_people']}")
            for tier, count in payload["tiers"].items():
                print(f"- {tier}: {count}")
        return 0

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
