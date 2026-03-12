#!/usr/bin/env python3
"""Linus relationship-intelligence builder and query tool.

This script turns legacy relationship artifacts into a cleaner people graph,
renders searchable Markdown summaries for OpenClaw memory, and supports a few
live maintenance/query operations on the resulting SQLite store.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import html
import json
import math
import os
import re
import sqlite3
import sys
import time
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from email.utils import getaddresses, parsedate_to_datetime
from pathlib import Path
from typing import Any, Iterable
from urllib import error as urlerror
from urllib import parse as urlparse
from urllib import request as urlrequest


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
    re.compile(r"^[A-Za-z0-9+/=]{16,}$"),
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
    "came",
    "come",
    "coming",
    "did",
    "do",
    "done",
    "evening",
    "for",
    "free",
    "get",
    "got",
    "good",
    "had",
    "have",
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
    "lol",
    "made",
    "make",
    "man",
    "me",
    "media",
    "message",
    "morning",
    "needed",
    "no",
    "not",
    "now",
    "ok",
    "okay",
    "sent",
    "shared",
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
    "week",
    "well",
    "when",
    "very",
    "happy",
    "proud",
    "safe",
    "sleep",
    "same",
    "some",
    "app",
    "meeting",
    "arrived",
    "here",
    "like",
    "thing",
    "things",
    "there",
    "time",
    "today",
    "looking",
    "doing",
    "food",
    "airport",
    "home",
    "want",
    "what",
    "will",
    "with",
    "you",
    "yeah",
    "yea",
    "your",
    "ill",
    "i'll",
    "shit",
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
PROMOTED_DOC_KEYWORDS = {
    "company": {
        "annual plan",
        "board",
        "brief",
        "competitor",
        "customer",
        "deck",
        "engineering",
        "finance",
        "forecast",
        "fundraise",
        "go to market",
        "gtm",
        "hiring",
        "investor",
        "metrics",
        "okr",
        "operating",
        "plan",
        "product",
        "q1",
        "q2",
        "q3",
        "q4",
        "quarterly",
        "research",
        "roadmap",
        "sales",
        "strategy",
        "tribble",
    },
    "personal": {
        "angelic",
        "anniversary",
        "birthday",
        "doctor",
        "family",
        "health",
        "linus",
        "parents",
        "personal",
        "prasad",
        "rhaine",
        "travel",
        "uma",
    },
}
LOW_SIGNAL_EMAIL_PATTERNS = (
    "noreply@",
    "no-reply@",
    "notifications@",
    "mailer-daemon@",
    "bounce@",
    "mailer@",
    "team@substack.com",
    "hello@substack.com",
)
EMAIL_GUIDED_STOPWORDS = LOW_SIGNAL_WORDS | {
    "attached",
    "attachments",
    "calendar",
    "deck",
    "email",
    "emails",
    "fwd",
    "fw",
    "gmail",
    "google",
    "intro",
    "intros",
    "invoice",
    "project",
    "proposal",
    "receipt",
    "re",
    "reply",
    "thread",
    "threads",
    "travel",
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

OWNER_ENTITY_REF = "sunil-rao"
OWNER_EMAILS = {
    "sunil@tribble.ai",
    "sunilrao.inc@gmail.com",
}
GOOGLE_CREDENTIALS_DIR = Path.home() / ".google_workspace_mcp" / "credentials"
GOOGLE_TOKEN_CACHE: dict[str, tuple[str, datetime]] = {}
GOOGLE_RETRYABLE_HTTP_CODES = {408, 409, 425, 429, 500, 502, 503, 504}
PROMOTABLE_CANDIDATE_PREDICATES = {
    "about_status",
    "availability_window",
    "child_context",
    "follow_up",
    "health_update",
    "mentioned_topic",
    "out_of_office",
    "partner_context",
    "travel_location",
}
TEMPORAL_PREDICATES = {
    "availability_window",
    "out_of_office",
    "travel_location",
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


def normalize_email(value: str | None) -> str:
    clean = normalize_text(value).strip("<>").lower()
    if not clean or "@" not in clean:
        return ""
    return clean


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
    lowered = clean.casefold()
    if not re.search(r"[A-Za-z]", clean):
        return True
    if lowered in LOW_SIGNAL_WORDS:
        return True
    if lowered.startswith("phone ") or lowered.startswith("message ") or lowered.startswith("conversation "):
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


def is_trusted_anchor_name(name: str | None) -> bool:
    clean = normalize_text(name).casefold()
    return clean in {item["canonical"].casefold() for item in SPECIAL_CONTACTS}


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


def safe_json_loads(value: Any, default: Any) -> Any:
    if value in (None, "", b""):
        return default
    if isinstance(value, (bytes, bytearray)):
        try:
            value = value.decode("utf-8", errors="ignore")
        except Exception:
            return default
    try:
        return json.loads(value)
    except (TypeError, json.JSONDecodeError):
        return default


def strip_html(value: str) -> str:
    clean = normalize_text(value)
    if not clean:
        return ""
    clean = re.sub(r"<br\s*/?>", "\n", clean, flags=re.IGNORECASE)
    clean = re.sub(r"</p\s*>", "\n\n", clean, flags=re.IGNORECASE)
    clean = re.sub(r"<[^>]+>", " ", clean)
    return normalize_text(html.unescape(clean))


def doc_excerpt(value: str, limit: int = 280) -> str:
    clean = normalize_text(strip_html(value))
    if len(clean) <= limit:
        return clean
    clipped = clean[:limit].rsplit(" ", 1)[0].rstrip(",;:")
    return f"{clipped}..."


def trim_body(value: str, limit: int = 40000) -> str:
    clean = value.strip()
    if len(clean) <= limit:
        return clean
    return clean[:limit]


def doc_text(*parts: str) -> str:
    return " ".join(normalize_text(part) for part in parts if normalize_text(part)).strip()


def document_domain(title: str, excerpt: str = "", author: str = "", channel: str = "") -> str:
    text = doc_text(title, excerpt, author, channel).casefold()
    company_score = sum(1 for item in PROMOTED_DOC_KEYWORDS["company"] if item in text)
    personal_score = sum(1 for item in PROMOTED_DOC_KEYWORDS["personal"] if item in text)
    if channel == "calendar" and any(item in text for item in ("doctor", "birthday", "anniversary", "parents", "travel")):
        personal_score += 2
    if "tribble" in text or "board" in text or "roadmap" in text or "customer" in text:
        company_score += 2
    if "angelic" in text or "prasad" in text or "uma" in text or "rhaine" in text:
        personal_score += 2
    if personal_score > company_score:
        return "personal"
    if company_score > 0:
        return "company"
    return "general"


def document_priority_score(
    title: str,
    *,
    channel: str,
    excerpt: str = "",
    author: str = "",
    updated_at: str = "",
) -> tuple[float, list[str]]:
    text = doc_text(title, excerpt, author).casefold()
    reasons: list[str] = []
    score = {"roam": 3.0, "drive": 6.0, "calendar": 2.0}.get(channel, 1.0)
    if channel == "roam":
        reasons.append("personal-notes")
    elif channel == "drive":
        reasons.append("workspace-doc")
    elif channel == "calendar":
        reasons.append("calendar-signal")

    domain = document_domain(title, excerpt, author, channel)
    if domain == "company":
        score += 2.0
        reasons.append("company-context")
    elif domain == "personal":
        score += 2.0
        reasons.append("personal-context")

    for item in sorted(PROMOTED_DOC_KEYWORDS["company"] | PROMOTED_DOC_KEYWORDS["personal"]):
        if item in text:
            score += 0.6
            reasons.append(f"keyword:{item}")

    parsed = parse_date(updated_at)
    if parsed:
        age_days = max((now_utc() - parsed).days, 0)
        if age_days <= 14:
            score += 4.0
            reasons.append("fresh")
        elif age_days <= 45:
            score += 3.0
            reasons.append("recent")
        elif age_days <= 120:
            score += 1.5

    lowered_author = normalize_text(author).casefold()
    if "sunil" in lowered_author or "tribble" in lowered_author:
        score += 0.8
        reasons.append("owner-adjacent")

    if excerpt:
        score += min(len(excerpt) / 400.0, 1.0)
    return score, reasons[:6]


def email_domain(value: str) -> str:
    clean = normalize_email(value)
    if not clean or "@" not in clean:
        return ""
    return clean.split("@", 1)[1]


def guess_org_from_email(value: str) -> str:
    domain = email_domain(value)
    if not domain:
        return ""
    base = domain.split(".")[0]
    if base in {"gmail", "googlemail", "icloud", "me", "mac", "hotmail", "outlook", "live", "yahoo"}:
        return ""
    return clean_org(base.replace("-", " ").title(), "")


def decode_bytes(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, (bytes, bytearray)):
        return value.decode("utf-8", errors="ignore")
    return str(value)


def markdown_to_text(value: str) -> str:
    clean = value.replace("\r\n", "\n")
    clean = re.sub(r"\[\[([^\]]+)\]\]", r"\1", clean)
    clean = re.sub(r"\{\{[^}]+\}\}", " ", clean)
    clean = re.sub(r"`{1,3}", "", clean)
    clean = re.sub(r"^[#>\-\*\s]+", "", clean, flags=re.MULTILINE)
    return normalize_text(clean)


def source_document_key(channel: str, source_kind: str, doc_id: str) -> str:
    return f"{channel}:{source_kind}:{doc_id}"


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
        if note.casefold().startswith("whatsapp history:"):
            continue
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
    if record.preferred_channel == "whatsapp":
        return "Frequent WhatsApp contact in Sunil's network"
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
    ensure_runtime_tables(dest)
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


def ensure_runtime_tables(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS people (
            person_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            canonical_name TEXT NOT NULL,
            aliases_json TEXT NOT NULL DEFAULT '[]',
            phones_json TEXT NOT NULL DEFAULT '[]',
            emails_json TEXT NOT NULL DEFAULT '[]',
            organizations_json TEXT NOT NULL DEFAULT '[]',
            roles_json TEXT NOT NULL DEFAULT '[]',
            tier TEXT NOT NULL DEFAULT 'unassigned',
            relationship_score REAL NOT NULL DEFAULT 0,
            importance REAL NOT NULL DEFAULT 0,
            category TEXT NOT NULL DEFAULT 'network',
            relationship_label TEXT NOT NULL DEFAULT '',
            preferred_channel TEXT NOT NULL DEFAULT 'unknown',
            last_touch_date TEXT,
            cadence_days INTEGER NOT NULL DEFAULT 180,
            notes_json TEXT NOT NULL DEFAULT '[]',
            dossier_excerpt TEXT NOT NULL DEFAULT '',
            topics_json TEXT NOT NULL DEFAULT '[]',
            open_actions_json TEXT NOT NULL DEFAULT '[]',
            important_dates_json TEXT NOT NULL DEFAULT '[]',
            source_contact_ids_json TEXT NOT NULL DEFAULT '[]',
            built_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS touch_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            person_id TEXT NOT NULL,
            touched_at TEXT NOT NULL,
            channel TEXT NOT NULL,
            note TEXT NOT NULL,
            direction TEXT NOT NULL DEFAULT 'manual',
            source TEXT NOT NULL DEFAULT 'linus',
            FOREIGN KEY (person_id) REFERENCES people (person_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS channel_contacts (
            channel TEXT NOT NULL,
            account_id TEXT NOT NULL DEFAULT '',
            contact_id TEXT NOT NULL,
            display_name TEXT NOT NULL DEFAULT '',
            short_name TEXT NOT NULL DEFAULT '',
            phone TEXT NOT NULL DEFAULT '',
            email TEXT NOT NULL DEFAULT '',
            raw_json TEXT NOT NULL DEFAULT '{}',
            updated_at TEXT NOT NULL,
            PRIMARY KEY (channel, account_id, contact_id)
        );

        CREATE TABLE IF NOT EXISTS conversation_threads (
            channel TEXT NOT NULL,
            account_id TEXT NOT NULL DEFAULT '',
            chat_id TEXT NOT NULL,
            chat_name TEXT NOT NULL DEFAULT '',
            chat_phone TEXT NOT NULL DEFAULT '',
            is_group INTEGER NOT NULL DEFAULT 0,
            last_message_at TEXT,
            raw_json TEXT NOT NULL DEFAULT '{}',
            updated_at TEXT NOT NULL,
            PRIMARY KEY (channel, account_id, chat_id)
        );

        CREATE TABLE IF NOT EXISTS message_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            channel TEXT NOT NULL,
            account_id TEXT NOT NULL DEFAULT '',
            chat_id TEXT NOT NULL,
            chat_name TEXT NOT NULL DEFAULT '',
            chat_phone TEXT NOT NULL DEFAULT '',
            is_group INTEGER NOT NULL DEFAULT 0,
            message_id TEXT NOT NULL,
            sender_id TEXT NOT NULL DEFAULT '',
            sender_name TEXT NOT NULL DEFAULT '',
            sender_phone TEXT NOT NULL DEFAULT '',
            sender_email TEXT NOT NULL DEFAULT '',
            counterpart_name TEXT NOT NULL DEFAULT '',
            counterpart_phone TEXT NOT NULL DEFAULT '',
            counterpart_email TEXT NOT NULL DEFAULT '',
            person_id TEXT,
            direction TEXT NOT NULL,
            sent_at TEXT NOT NULL,
            message_type TEXT NOT NULL DEFAULT 'text',
            text TEXT NOT NULL DEFAULT '',
            excerpt TEXT NOT NULL DEFAULT '',
            is_history INTEGER NOT NULL DEFAULT 0,
            raw_json TEXT NOT NULL DEFAULT '{}',
            imported_at TEXT NOT NULL,
            FOREIGN KEY (person_id) REFERENCES people (person_id) ON DELETE SET NULL,
            UNIQUE (channel, account_id, chat_id, message_id)
        );

        CREATE TABLE IF NOT EXISTS person_facts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            person_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            fact_type TEXT NOT NULL,
            fact_value TEXT NOT NULL,
            normalized_value TEXT NOT NULL DEFAULT '',
            confidence REAL NOT NULL DEFAULT 0.5,
            source_kind TEXT NOT NULL DEFAULT '',
            source_ref TEXT NOT NULL DEFAULT '',
            observed_at TEXT,
            raw_json TEXT NOT NULL DEFAULT '{}',
            FOREIGN KEY (person_id) REFERENCES people (person_id) ON DELETE CASCADE,
            UNIQUE (person_id, channel, fact_type, normalized_value, source_kind, source_ref)
        );

        CREATE TABLE IF NOT EXISTS relationship_signals (
            person_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            direct_inbound_count INTEGER NOT NULL DEFAULT 0,
            direct_outbound_count INTEGER NOT NULL DEFAULT 0,
            group_inbound_count INTEGER NOT NULL DEFAULT 0,
            group_outbound_count INTEGER NOT NULL DEFAULT 0,
            first_seen_at TEXT,
            last_seen_at TEXT,
            last_direct_at TEXT,
            last_group_at TEXT,
            recent_excerpt TEXT NOT NULL DEFAULT '',
            top_topics_json TEXT NOT NULL DEFAULT '[]',
            raw_json TEXT NOT NULL DEFAULT '{}',
            updated_at TEXT NOT NULL,
            PRIMARY KEY (person_id, channel),
            FOREIGN KEY (person_id) REFERENCES people (person_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS semantic_claims (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            person_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            claim_type TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object_value TEXT NOT NULL,
            normalized_value TEXT NOT NULL DEFAULT '',
            claim_status TEXT NOT NULL DEFAULT 'accepted',
            confidence REAL NOT NULL DEFAULT 0.5,
            source_kind TEXT NOT NULL DEFAULT '',
            source_ref TEXT NOT NULL DEFAULT '',
            source_message_id TEXT NOT NULL DEFAULT '',
            source_chat_id TEXT NOT NULL DEFAULT '',
            observed_at TEXT,
            raw_json TEXT NOT NULL DEFAULT '{}',
            FOREIGN KEY (person_id) REFERENCES people (person_id) ON DELETE CASCADE,
            UNIQUE (
                person_id, channel, claim_type, predicate, normalized_value,
                source_ref, source_message_id
            )
        );

        CREATE TABLE IF NOT EXISTS relationship_edges (
            subject_type TEXT NOT NULL,
            subject_ref TEXT NOT NULL,
            predicate TEXT NOT NULL,
            object_type TEXT NOT NULL,
            object_ref TEXT NOT NULL,
            channel TEXT NOT NULL DEFAULT '',
            confidence REAL NOT NULL DEFAULT 0.5,
            source_kind TEXT NOT NULL DEFAULT '',
            source_ref TEXT NOT NULL DEFAULT '',
            observed_at TEXT,
            raw_json TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (
                subject_type, subject_ref, predicate,
                object_type, object_ref, channel, source_ref
            )
        );

        CREATE TABLE IF NOT EXISTS entities (
            entity_id TEXT PRIMARY KEY,
            entity_type TEXT NOT NULL,
            canonical_name TEXT NOT NULL,
            normalized_name TEXT NOT NULL DEFAULT '',
            source_channel TEXT NOT NULL DEFAULT '',
            confidence REAL NOT NULL DEFAULT 0.5,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS entity_aliases (
            entity_id TEXT NOT NULL,
            source_channel TEXT NOT NULL DEFAULT '',
            alias_type TEXT NOT NULL DEFAULT 'name',
            alias_value TEXT NOT NULL,
            normalized_value TEXT NOT NULL DEFAULT '',
            source_kind TEXT NOT NULL DEFAULT '',
            source_ref TEXT NOT NULL DEFAULT '',
            observed_at TEXT,
            raw_json TEXT NOT NULL DEFAULT '{}',
            FOREIGN KEY (entity_id) REFERENCES entities (entity_id) ON DELETE CASCADE,
            UNIQUE (entity_id, source_channel, alias_type, normalized_value, source_kind, source_ref)
        );

        CREATE TABLE IF NOT EXISTS source_documents (
            source_channel TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            doc_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            url TEXT NOT NULL DEFAULT '',
            author TEXT NOT NULL DEFAULT '',
            created_at TEXT,
            updated_at TEXT,
            excerpt TEXT NOT NULL DEFAULT '',
            body TEXT NOT NULL DEFAULT '',
            metadata_json TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (source_channel, source_kind, doc_id)
        );

        CREATE INDEX IF NOT EXISTS idx_channel_contacts_phone
            ON channel_contacts (channel, account_id, phone);
        CREATE INDEX IF NOT EXISTS idx_people_importance
            ON people (importance DESC, relationship_score DESC);
        CREATE INDEX IF NOT EXISTS idx_people_last_touch
            ON people (last_touch_date);
        CREATE INDEX IF NOT EXISTS idx_touch_events_person_date
            ON touch_events (person_id, touched_at DESC);
        CREATE INDEX IF NOT EXISTS idx_conversation_threads_channel_last
            ON conversation_threads (channel, account_id, last_message_at DESC);
        CREATE INDEX IF NOT EXISTS idx_message_events_channel_time
            ON message_events (channel, account_id, sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_message_events_person_time
            ON message_events (person_id, sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_message_events_chat_time
            ON message_events (chat_id, sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_message_events_direction_time
            ON message_events (direction, sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_person_facts_person
            ON person_facts (person_id, channel, fact_type);
        CREATE INDEX IF NOT EXISTS idx_person_facts_value
            ON person_facts (channel, fact_type, normalized_value);
        CREATE INDEX IF NOT EXISTS idx_relationship_signals_channel_last
            ON relationship_signals (channel, last_seen_at DESC);
        CREATE INDEX IF NOT EXISTS idx_semantic_claims_person
            ON semantic_claims (person_id, channel, claim_type, observed_at DESC);
        CREATE INDEX IF NOT EXISTS idx_semantic_claims_lookup
            ON semantic_claims (channel, predicate, normalized_value);
        CREATE INDEX IF NOT EXISTS idx_relationship_edges_subject
            ON relationship_edges (subject_type, subject_ref, predicate);
        CREATE INDEX IF NOT EXISTS idx_relationship_edges_object
            ON relationship_edges (object_type, object_ref, predicate);
        CREATE INDEX IF NOT EXISTS idx_entities_type_name
            ON entities (entity_type, normalized_name);
        CREATE INDEX IF NOT EXISTS idx_entity_aliases_value
            ON entity_aliases (source_channel, alias_type, normalized_value);
        CREATE INDEX IF NOT EXISTS idx_source_documents_channel_time
            ON source_documents (source_channel, updated_at DESC);
        """
    )
    semantic_columns = {row[1] for row in conn.execute("PRAGMA table_info(semantic_claims)").fetchall()}
    if "claim_status" not in semantic_columns:
        conn.execute("ALTER TABLE semantic_claims ADD COLUMN claim_status TEXT NOT NULL DEFAULT 'accepted'")
    contact_columns = {row[1] for row in conn.execute("PRAGMA table_info(channel_contacts)").fetchall()}
    if "email" not in contact_columns:
        conn.execute("ALTER TABLE channel_contacts ADD COLUMN email TEXT NOT NULL DEFAULT ''")
    message_columns = {row[1] for row in conn.execute("PRAGMA table_info(message_events)").fetchall()}
    if "sender_email" not in message_columns:
        conn.execute("ALTER TABLE message_events ADD COLUMN sender_email TEXT NOT NULL DEFAULT ''")
    if "counterpart_email" not in message_columns:
        conn.execute("ALTER TABLE message_events ADD COLUMN counterpart_email TEXT NOT NULL DEFAULT ''")
    try:
        conn.execute(
            """
            CREATE VIRTUAL TABLE IF NOT EXISTS source_documents_fts USING fts5(
                doc_key UNINDEXED,
                source_channel,
                source_kind,
                title,
                excerpt,
                body
            )
            """
        )
    except sqlite3.OperationalError:
        pass
    try:
        conn.execute(
            """
            CREATE VIRTUAL TABLE IF NOT EXISTS people_fts USING fts5(
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


def open_store(db_path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    ensure_runtime_tables(conn)
    return conn


def has_source_document_fts(conn: sqlite3.Connection) -> bool:
    row = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'source_documents_fts' LIMIT 1"
    ).fetchone()
    return bool(row)


def refresh_source_document_fts(conn: sqlite3.Connection, doc_key: str) -> None:
    if not has_source_document_fts(conn):
        return
    row = conn.execute(
        """
        SELECT source_channel, source_kind, title, excerpt, body
        FROM source_documents
        WHERE (source_channel || ':' || source_kind || ':' || doc_id) = ?
        """,
        (doc_key,),
    ).fetchone()
    conn.execute("DELETE FROM source_documents_fts WHERE doc_key = ?", (doc_key,))
    if not row:
        return
    conn.execute(
        """
        INSERT INTO source_documents_fts (
            doc_key, source_channel, source_kind, title, excerpt, body
        ) VALUES (?, ?, ?, ?, ?, ?)
        """,
        (
            doc_key,
            row["source_channel"],
            row["source_kind"],
            row["title"],
            row["excerpt"],
            row["body"],
        ),
    )


def upsert_source_document(
    conn: sqlite3.Connection,
    *,
    source_channel: str,
    source_kind: str,
    doc_id: str,
    title: str,
    body: str,
    url: str = "",
    author: str = "",
    created_at: str | None = None,
    updated_at: str | None = None,
    metadata: dict[str, Any] | None = None,
) -> str:
    doc_key = source_document_key(source_channel, source_kind, doc_id)
    clean_title = normalize_text(title) or doc_id
    clean_body = trim_body(body)
    conn.execute(
        """
        INSERT INTO source_documents (
            source_channel, source_kind, doc_id, title, url, author,
            created_at, updated_at, excerpt, body, metadata_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(source_channel, source_kind, doc_id) DO UPDATE SET
            title = excluded.title,
            url = excluded.url,
            author = excluded.author,
            created_at = COALESCE(source_documents.created_at, excluded.created_at),
            updated_at = COALESCE(excluded.updated_at, source_documents.updated_at),
            excerpt = excluded.excerpt,
            body = excluded.body,
            metadata_json = excluded.metadata_json
        """,
        (
            source_channel,
            source_kind,
            doc_id,
            clean_title,
            normalize_text(url),
            normalize_text(author),
            created_at,
            updated_at or now_utc().isoformat(),
            doc_excerpt(clean_body),
            clean_body,
            safe_json(metadata or {}),
        ),
    )
    refresh_source_document_fts(conn, doc_key)
    return doc_key


def clear_source_documents_channel(conn: sqlite3.Connection, source_channel: str) -> None:
    conn.execute("DELETE FROM source_documents WHERE source_channel = ?", (source_channel,))
    if has_source_document_fts(conn):
        conn.execute("DELETE FROM source_documents_fts WHERE source_channel = ?", (source_channel,))


def available_google_accounts() -> list[str]:
    if not GOOGLE_CREDENTIALS_DIR.exists():
        return []
    return sorted(path.stem for path in GOOGLE_CREDENTIALS_DIR.glob("*.json"))


def load_google_credentials(account_email: str | None = None) -> tuple[Path, dict[str, Any]]:
    accounts = available_google_accounts()
    if not accounts:
        raise SystemExit(f"No Google Workspace credentials found under {GOOGLE_CREDENTIALS_DIR}")
    chosen = normalize_email(account_email) if account_email else ""
    if chosen and chosen not in accounts:
        raise SystemExit(f"No Google Workspace credential file for {chosen}")
    if not chosen:
        chosen = "sunil@tribble.ai" if "sunil@tribble.ai" in accounts else accounts[0]
    path = GOOGLE_CREDENTIALS_DIR / f"{chosen}.json"
    return path, safe_json_loads(path.read_text(encoding="utf-8"), {})


def google_access_token(account_email: str | None = None) -> tuple[str, str]:
    path, payload = load_google_credentials(account_email)
    cached = GOOGLE_TOKEN_CACHE.get(path.stem)
    if cached and cached[1] > now_utc() + timedelta(seconds=60):
        return path.stem, cached[0]
    refresh_token = normalize_text(payload.get("refresh_token"))
    client_id = normalize_text(payload.get("client_id"))
    client_secret = normalize_text(payload.get("client_secret"))
    token_uri = normalize_text(payload.get("token_uri")) or "https://oauth2.googleapis.com/token"
    if not refresh_token or not client_id or not client_secret:
        raise SystemExit(f"Incomplete Google OAuth credentials in {path}")
    encoded = urlparse.urlencode(
        {
            "client_id": client_id,
            "client_secret": client_secret,
            "refresh_token": refresh_token,
            "grant_type": "refresh_token",
        }
    ).encode("utf-8")
    request = urlrequest.Request(
        token_uri,
        data=encoded,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    try:
        with urlrequest.urlopen(request, timeout=30) as response:
            refreshed = json.loads(response.read().decode("utf-8"))
    except urlerror.HTTPError as exc:
        raise SystemExit(f"Google token refresh failed for {path.stem}: {exc.read().decode('utf-8', errors='ignore')}") from exc
    access_token = normalize_text(refreshed.get("access_token"))
    if not access_token:
        raise SystemExit(f"Google token refresh returned no access token for {path.stem}")
    payload["token"] = access_token
    expires_in = int(refreshed.get("expires_in") or payload.get("expires_in") or 3000)
    payload["expires_in"] = expires_in
    path.write_text(safe_json(payload), encoding="utf-8")
    GOOGLE_TOKEN_CACHE[path.stem] = (access_token, now_utc() + timedelta(seconds=max(expires_in - 120, 60)))
    return path.stem, access_token


def google_api_json(
    account_email: str | None,
    url: str,
    *,
    params: dict[str, Any] | None = None,
) -> dict[str, Any]:
    account, access_token = google_access_token(account_email)
    return google_api_json_with_token(account, access_token, url, params=params)


def google_api_json_with_token(
    account_label: str,
    access_token: str,
    url: str,
    *,
    params: dict[str, Any] | None = None,
) -> dict[str, Any]:
    query = urlparse.urlencode({key: value for key, value in (params or {}).items() if value not in (None, "")}, doseq=True)
    target = f"{url}?{query}" if query else url
    request = urlrequest.Request(
        target,
        headers={
            "Authorization": f"Bearer {access_token}",
            "Accept": "application/json",
        },
    )
    last_error: Exception | None = None
    for attempt in range(5):
        try:
            with urlrequest.urlopen(request, timeout=90) as response:
                return safe_json_loads(response.read().decode("utf-8"), {})
        except urlerror.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="ignore")
            if exc.code in GOOGLE_RETRYABLE_HTTP_CODES and attempt < 4:
                time.sleep(min(2**attempt, 16))
                last_error = exc
                continue
            raise SystemExit(f"Google API request failed for {account_label}: {exc.code} {body}") from exc
        except (TimeoutError, urlerror.URLError) as exc:
            if attempt < 4:
                time.sleep(min(2**attempt, 16))
                last_error = exc
                continue
            raise SystemExit(f"Google API request failed for {account_label}: {exc}") from exc
    if last_error is not None:
        raise SystemExit(f"Google API request failed for {account_label}: {last_error}")
    return {}


def google_api_bytes(
    account_email: str | None,
    url: str,
    *,
    params: dict[str, Any] | None = None,
) -> bytes:
    _, access_token = google_access_token(account_email)
    query = urlparse.urlencode({key: value for key, value in (params or {}).items() if value not in (None, "")}, doseq=True)
    target = f"{url}?{query}" if query else url
    request = urlrequest.Request(
        target,
        headers={"Authorization": f"Bearer {access_token}"},
    )
    last_error: Exception | None = None
    for attempt in range(5):
        try:
            with urlrequest.urlopen(request, timeout=180) as response:
                return response.read()
        except urlerror.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="ignore")
            if exc.code in GOOGLE_RETRYABLE_HTTP_CODES and attempt < 4:
                time.sleep(min(2**attempt, 16))
                last_error = exc
                continue
            raise SystemExit(f"Google binary request failed: {exc.code} {body}") from exc
        except (TimeoutError, urlerror.URLError) as exc:
            if attempt < 4:
                time.sleep(min(2**attempt, 16))
                last_error = exc
                continue
            raise SystemExit(f"Google binary request failed: {exc}") from exc
    if last_error is not None:
        raise SystemExit(f"Google binary request failed: {last_error}")
    return b""


def parse_address_list(*values: str) -> list[tuple[str, str]]:
    parsed = getaddresses(values)
    deduped: list[tuple[str, str]] = []
    seen: set[str] = set()
    for name, email in parsed:
        clean_email = normalize_email(email)
        if not clean_email or clean_email in seen:
            continue
        seen.add(clean_email)
        deduped.append((normalize_text(name), clean_email))
    return deduped


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
    raw_phones = [normalize_phone(item) for item in json.loads(row["phones_json"]) if normalize_phone(item)]
    normalized_phones: set[str] = set()
    for phone in raw_phones:
        if phone.startswith("+"):
            normalized_phones.add(phone)
        elif f"+{phone}" in raw_phones:
            continue
        else:
            normalized_phones.add(phone)
    return PersonRecord(
        person_id=row["person_id"],
        display_name=row["display_name"],
        canonical_name=row["canonical_name"],
        aliases=set(json.loads(row["aliases_json"])),
        phones=normalized_phones,
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
        notes=clean_profile_notes(json.loads(row["notes_json"])),
        dossier_excerpt=row["dossier_excerpt"],
        topics=clean_profile_topics(json.loads(row["topics_json"])),
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


def append_json_list(raw_json: str, values: Iterable[str], limit: int = 12) -> str:
    existing = json.loads(raw_json) if raw_json else []
    merged = dedupe_preserve([*existing, *values])
    return safe_json(merged[:limit])


def clean_profile_topics(values: Iterable[str], limit: int = 20) -> list[str]:
    cleaned: list[str] = []
    for value in dedupe_preserve(values):
        topic = normalize_text(value)
        if not topic:
            continue
        tokens = [token.casefold() for token in words(topic)]
        if not tokens:
            continue
        if len(tokens) == 1 and (len(tokens[0]) < 4 or tokens[0] in LOW_SIGNAL_WORDS):
            continue
        if all(token in LOW_SIGNAL_WORDS for token in tokens):
            continue
        cleaned.append(topic)
        if len(cleaned) >= limit:
            break
    return cleaned


def clean_profile_notes(values: Iterable[str], limit: int = 24, keep_whatsapp_history: bool = True) -> list[str]:
    cleaned: list[str] = []
    for value in dedupe_preserve(values):
        note = normalize_text(value)
        if not note:
            continue
        if note.casefold().startswith("whatsapp history:") and not keep_whatsapp_history:
            continue
        cleaned.append(note)
        if len(cleaned) >= limit:
            break
    return cleaned


def append_json_objects(raw_json: str, values: Iterable[dict[str, Any]], limit: int = 12) -> str:
    existing = json.loads(raw_json) if raw_json else []
    seen: set[str] = set()
    merged: list[dict[str, Any]] = []
    for item in [*existing, *values]:
        if not item:
            continue
        key = safe_json(item)
        if key in seen:
            continue
        seen.add(key)
        merged.append(item)
    return safe_json(merged[:limit])


def phone_from_chat_id(chat_id: str) -> str:
    clean = normalize_text(chat_id)
    if not clean:
        return ""
    if "@s.whatsapp.net" in clean or "@lid" in clean:
        base = clean.split("@", 1)[0]
        base = base.split(":", 1)[0]
        if base.isdigit():
            return normalize_phone(f"+{base}")
    return ""


def trim_message_excerpt(value: str, limit: int = 180) -> str:
    clean = normalize_text(value)
    if not clean:
        return ""
    if len(clean) <= limit:
        return clean
    return clean[: limit - 3].rstrip() + "..."


def merge_contact_ids(raw_json_a: str, raw_json_b: str) -> str:
    values = {int(item) for item in json.loads(raw_json_a or "[]") + json.loads(raw_json_b or "[]") if str(item).isdigit()}
    return safe_json(sorted(values))


def merge_people_priority(row: sqlite3.Row) -> tuple[float, float, float, float, float]:
    aliases = len(json.loads(row["aliases_json"]))
    phones = len(json.loads(row["phones_json"]))
    emails = len(json.loads(row["emails_json"]))
    notes = len(json.loads(row["notes_json"]))
    actions = len(json.loads(row["open_actions_json"]))
    return (
        -float(tier_rank(row["tier"])),
        float(row["importance"]),
        float(row["relationship_score"]),
        float(phones + emails + aliases + notes + actions * 2),
        1.0 if row["relationship_label"] else 0.0,
    )


def better_tier(a: str, b: str) -> str:
    return a if tier_rank(a) <= tier_rank(b) else b


def choose_category(current: str, candidate: str) -> str:
    current_clean = normalize_text(current).lower()
    candidate_clean = normalize_text(candidate).lower()
    if not current_clean:
        return candidate_clean or "network"
    if current_clean == "network" and candidate_clean:
        return candidate_clean
    return current_clean


def choose_channel(current: str, candidate: str) -> str:
    current_clean = normalize_text(current).lower()
    candidate_clean = normalize_text(candidate).lower()
    if not current_clean:
        return candidate_clean or "unknown"
    if current_clean == "unknown" and candidate_clean:
        return candidate_clean
    if current_clean == "sms" and candidate_clean == "whatsapp":
        return candidate_clean
    return current_clean


def match_person_row(conn: sqlite3.Connection, phone: str, display_name: str, email: str = "") -> sqlite3.Row | None:
    normalized_phone = normalize_phone(phone)
    if normalized_phone:
        row = conn.execute(
            """
            SELECT *
            FROM people
            WHERE lower(phones_json) LIKE ?
            ORDER BY importance DESC
            LIMIT 1
            """,
            (f"%{normalized_phone.lower()}%",),
        ).fetchone()
        if row:
            return row
    clean_email = normalize_text(email).lower()
    if clean_email:
        row = conn.execute(
            """
            SELECT *
            FROM people
            WHERE lower(emails_json) LIKE ?
            ORDER BY importance DESC
            LIMIT 1
            """,
            (f"%{clean_email}%",),
        ).fetchone()
        if row:
            return row
    clean_name = normalize_text(display_name)
    if clean_name:
        like = f"%{clean_name.casefold()}%"
        row = conn.execute(
            """
            SELECT *
            FROM people
            WHERE lower(display_name) = lower(?)
               OR lower(canonical_name) = lower(?)
               OR lower(aliases_json) LIKE ?
               OR lower(display_name) LIKE ?
               OR lower(canonical_name) LIKE ?
            ORDER BY importance DESC
            LIMIT 1
            """,
            (clean_name, clean_name, like, like, like),
        ).fetchone()
        if row:
            return row
    return None


def refresh_people_fts(conn: sqlite3.Connection, person_id: str) -> None:
    if not has_people_fts(conn):
        return
    row = conn.execute("SELECT * FROM people WHERE person_id = ?", (person_id,)).fetchone()
    if not row:
        return
    conn.execute("DELETE FROM people_fts WHERE person_id = ?", (person_id,))
    conn.execute(
        """
        INSERT INTO people_fts (
            person_id, display_name, canonical_name, aliases, emails, phones,
            organizations, roles, topics, notes
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            row["person_id"],
            row["display_name"],
            row["canonical_name"],
            " ".join(json.loads(row["aliases_json"])),
            " ".join(json.loads(row["emails_json"])),
            " ".join(json.loads(row["phones_json"])),
            " ".join(json.loads(row["organizations_json"])),
            " ".join(json.loads(row["roles_json"])),
            " ".join(json.loads(row["topics_json"])),
            " ".join(json.loads(row["notes_json"])),
        ),
    )


def merge_people_rows(conn: sqlite3.Connection, target_id: str, source_id: str) -> bool:
    if not target_id or not source_id or target_id == source_id:
        return False
    target = conn.execute("SELECT * FROM people WHERE person_id = ?", (target_id,)).fetchone()
    source = conn.execute("SELECT * FROM people WHERE person_id = ?", (source_id,)).fetchone()
    if not target or not source:
        return False

    target_record = record_for_row(target)
    source_record = record_for_row(source)
    display_name = choose_display_name(target_record.display_name, source_record.display_name)
    canonical_name = choose_display_name(target_record.canonical_name, source_record.canonical_name or source_record.display_name)
    aliases_json = append_json_list(target["aliases_json"], [*target_record.aliases, *source_record.aliases, display_name, canonical_name], limit=24)
    phones_json = append_json_list(target["phones_json"], [*target_record.phones, *source_record.phones], limit=24)
    emails_json = append_json_list(target["emails_json"], [*target_record.emails, *source_record.emails], limit=24)
    organizations_json = append_json_list(target["organizations_json"], [*target_record.organizations, *source_record.organizations], limit=24)
    roles_json = append_json_list(target["roles_json"], [*target_record.roles, *source_record.roles], limit=24)
    topics_json = append_json_list(target["topics_json"], [*target_record.topics, *source_record.topics], limit=20)
    notes_json = append_json_list(target["notes_json"], [*target_record.notes, *source_record.notes], limit=24)
    open_actions_json = append_json_objects(target["open_actions_json"], [*target_record.open_actions, *source_record.open_actions], limit=24)
    important_dates_json = append_json_objects(target["important_dates_json"], [*target_record.important_dates, *source_record.important_dates], limit=24)
    source_contact_ids_json = merge_contact_ids(target["source_contact_ids_json"], source["source_contact_ids_json"])
    tier = better_tier(target["tier"], source["tier"])
    category = choose_category(target["category"], source["category"])
    relationship_label = target["relationship_label"] or source["relationship_label"]
    preferred = choose_channel(target["preferred_channel"], source["preferred_channel"])
    last_touch_date = best_iso_date(latest_touch(conn, target), latest_touch(conn, source))
    cadence_days = min(int(target["cadence_days"]), int(source["cadence_days"]))
    relationship_score = max(float(target["relationship_score"]), float(source["relationship_score"]))
    dossier_excerpt = target["dossier_excerpt"] or source["dossier_excerpt"]
    built_at = now_utc().isoformat()

    conn.execute(
        """
        UPDATE people
        SET display_name = ?, canonical_name = ?, aliases_json = ?, phones_json = ?, emails_json = ?,
            organizations_json = ?, roles_json = ?, tier = ?, relationship_score = ?, category = ?,
            relationship_label = ?, preferred_channel = ?, last_touch_date = ?, cadence_days = ?,
            notes_json = ?, dossier_excerpt = ?, topics_json = ?, open_actions_json = ?,
            important_dates_json = ?, source_contact_ids_json = ?, built_at = ?
        WHERE person_id = ?
        """,
        (
            display_name,
            canonical_name,
            aliases_json,
            phones_json,
            emails_json,
            organizations_json,
            roles_json,
            tier,
            relationship_score,
            category,
            relationship_label,
            preferred,
            last_touch_date,
            cadence_days,
            notes_json,
            dossier_excerpt,
            topics_json,
            open_actions_json,
            important_dates_json,
            source_contact_ids_json,
            built_at,
            target_id,
        ),
    )
    conn.execute("UPDATE message_events SET person_id = ? WHERE person_id = ?", (target_id, source_id))
    conn.execute("UPDATE touch_events SET person_id = ? WHERE person_id = ?", (target_id, source_id))
    conn.execute("UPDATE person_facts SET person_id = ? WHERE person_id = ?", (target_id, source_id))
    conn.execute("UPDATE relationship_signals SET person_id = ? WHERE person_id = ?", (target_id, source_id))
    conn.execute("UPDATE semantic_claims SET person_id = ? WHERE person_id = ?", (target_id, source_id))
    conn.execute(
        """
        UPDATE relationship_edges
        SET subject_ref = ?
        WHERE subject_type = 'person' AND subject_ref = ?
        """,
        (target_id, source_id),
    )
    conn.execute(
        """
        UPDATE relationship_edges
        SET object_ref = ?
        WHERE object_type = 'person' AND object_ref = ?
        """,
        (target_id, source_id),
    )
    conn.execute("DELETE FROM people WHERE person_id = ?", (source_id,))
    refresh_people_fts(conn, target_id)
    conn.execute("DELETE FROM people_fts WHERE person_id = ?", (source_id,))
    row = conn.execute("SELECT * FROM people WHERE person_id = ?", (target_id,)).fetchone()
    if row:
        record = record_for_row(row)
        updated_importance = compute_importance(record)
        conn.execute("UPDATE people SET importance = ? WHERE person_id = ?", (updated_importance, target_id))
        refresh_people_fts(conn, target_id)
    return True


def enforce_special_contacts(conn: sqlite3.Connection) -> int:
    normalized = 0
    for special in SPECIAL_CONTACTS:
        special_person_id = slugify(special["canonical"])
        candidate_rows: list[sqlite3.Row] = []
        row = conn.execute("SELECT * FROM people WHERE person_id = ?", (special_person_id,)).fetchone()
        if row:
            candidate_rows.append(row)
        for phone in special.get("phones", []):
            clean_phone = normalize_phone(phone)
            if not clean_phone:
                continue
            row = conn.execute(
                """
                SELECT *
                FROM people
                WHERE lower(phones_json) LIKE ?
                ORDER BY importance DESC
                LIMIT 1
                """,
                (f"%{clean_phone.lower()}%",),
            ).fetchone()
            if row and all(existing["person_id"] != row["person_id"] for existing in candidate_rows):
                candidate_rows.append(row)
        for email in special.get("emails", []):
            clean_email = normalize_text(email).lower()
            if not clean_email:
                continue
            row = conn.execute(
                """
                SELECT *
                FROM people
                WHERE lower(emails_json) LIKE ?
                ORDER BY importance DESC
                LIMIT 1
                """,
                (f"%{clean_email}%",),
            ).fetchone()
            if row and all(existing["person_id"] != row["person_id"] for existing in candidate_rows):
                candidate_rows.append(row)
        if not candidate_rows:
            continue
        target = sorted(candidate_rows, key=merge_people_priority, reverse=True)[0]
        for source in candidate_rows:
            if source["person_id"] != target["person_id"]:
                merge_people_rows(conn, target["person_id"], source["person_id"])
        aliases = append_json_list(
            target["aliases_json"],
            [special["canonical"], *special.get("aliases", [])],
            limit=24,
        )
        emails = append_json_list(target["emails_json"], [normalize_text(item).lower() for item in special.get("emails", [])], limit=24)
        phones = append_json_list(target["phones_json"], [normalize_phone(item) for item in special.get("phones", [])], limit=24)
        tier = "family" if special["category"] == "family" else "operations"
        conn.execute(
            """
            UPDATE people
            SET display_name = ?, canonical_name = ?, aliases_json = ?, phones_json = ?, emails_json = ?,
                tier = ?, category = ?, relationship_label = ?, cadence_days = ?, built_at = ?
            WHERE person_id = ?
            """,
            (
                special["canonical"],
                special["canonical"],
                aliases,
                phones,
                emails,
                tier,
                special["category"],
                special["relationship_label"],
                special["cadence_days"],
                now_utc().isoformat(),
                target["person_id"],
            ),
        )
        refresh_people_fts(conn, target["person_id"])
        normalized += 1
    return normalized


def ensure_person_for_counterparty(
    conn: sqlite3.Connection,
    *,
    display_name: str,
    phone: str,
    email: str = "",
    channel: str,
) -> str | None:
    clean_name = normalize_text(display_name)
    normalized_phone = normalize_phone(phone)
    clean_email = normalize_text(email).lower()
    if not clean_name and not normalized_phone and not clean_email:
        return None

    existing = match_person_row(conn, normalized_phone, clean_name, clean_email)
    timestamp = now_utc().isoformat()
    special = special_match(normalize_name(clean_name), normalized_phone, clean_email)

    if existing:
        updates: dict[str, Any] = {}
        if clean_name and normalize_name(existing["display_name"]) != normalize_name(clean_name):
            updates["display_name"] = choose_display_name(existing["display_name"], clean_name)
            updates["canonical_name"] = choose_display_name(existing["canonical_name"], clean_name)
        if clean_name:
            updates["aliases_json"] = append_json_list(existing["aliases_json"], [clean_name])
        if normalized_phone:
            updates["phones_json"] = append_json_list(existing["phones_json"], [normalized_phone])
        if clean_email:
            updates["emails_json"] = append_json_list(existing["emails_json"], [clean_email])
        if channel == "whatsapp":
            updates["preferred_channel"] = "sms"
        elif channel == "email":
            updates["preferred_channel"] = "email"
        if updates:
            assignments = ", ".join(f"{column} = ?" for column in updates)
            conn.execute(
                f"UPDATE people SET {assignments} WHERE person_id = ?",
                (*updates.values(), existing["person_id"]),
            )
            refresh_people_fts(conn, existing["person_id"])
        return str(existing["person_id"])

    person_key = stable_person_key(clean_name or clean_email or normalized_phone, clean_email, normalized_phone, special)
    if conn.execute("SELECT 1 FROM people WHERE person_id = ? LIMIT 1", (person_key,)).fetchone():
        return person_key

    tier = "family" if special and special["category"] == "family" else "unknown"
    category = special["category"] if special else "network"
    relationship_label = special["relationship_label"] if special else ""
    cadence_days = special["cadence_days"] if special else CADENCE_BY_TIER.get(tier, 180)
    aliases = []
    if clean_name:
        aliases.append(clean_name)
    if special:
        aliases.extend(special.get("aliases", []))
    notes = [f"Ingested from {channel} message evidence."]
    if special and special.get("notes"):
        notes.insert(0, special["notes"])
    conn.execute(
        """
        INSERT INTO people (
            person_id, display_name, canonical_name, aliases_json, phones_json, emails_json,
            organizations_json, roles_json, tier, relationship_score, importance, category,
            relationship_label, preferred_channel, last_touch_date, cadence_days, notes_json,
            dossier_excerpt, topics_json, open_actions_json, important_dates_json,
            source_contact_ids_json, built_at
        ) VALUES (?, ?, ?, ?, ?, ?, '[]', '[]', ?, ?, ?, ?, ?, ?, NULL, ?, ?, '', '[]', '[]', '[]', '[]', ?)
        """,
        (
            person_key,
            clean_name or special["canonical"] if special else clean_email or normalized_phone or DISPLAY_NAME_FALLBACK,
            special["canonical"] if special else clean_name or clean_email or normalized_phone or DISPLAY_NAME_FALLBACK,
            safe_json(dedupe_preserve(aliases)),
            safe_json([normalized_phone] if normalized_phone else []),
            safe_json([clean_email] if clean_email else []),
            tier,
            2.0 if not special else 8.0,
            2.5 if not special else 8.5,
            category,
            relationship_label,
            "email" if clean_email and not normalized_phone else ("sms" if normalized_phone else "unknown"),
            cadence_days,
            safe_json(dedupe_preserve(notes)),
            timestamp,
        ),
    )
    refresh_people_fts(conn, person_key)
    return person_key


def message_topic_terms(text: str, limit: int = 8) -> list[str]:
    clean = normalize_text(text)
    if not clean:
        return []
    clean = re.sub(r"https?://\S+", " ", clean)
    terms: list[str] = []
    for token in re.findall(r"[A-Za-z][A-Za-z'&.-]*", clean):
        lowered = token.casefold()
        if len(lowered) < 4:
            continue
        if lowered in LOW_SIGNAL_WORDS:
            continue
        if lowered.isdigit():
            continue
        terms.append(lowered)
    return terms[:limit]


def insert_person_fact(
    conn: sqlite3.Connection,
    *,
    person_id: str,
    channel: str,
    fact_type: str,
    fact_value: str,
    normalized_value: str,
    confidence: float,
    source_kind: str,
    source_ref: str,
    observed_at: str | None,
    raw: dict[str, Any] | None = None,
) -> None:
    clean_value = normalize_text(fact_value)
    clean_norm = normalize_text(normalized_value or fact_value).casefold()
    if not person_id or not clean_value:
        return
    conn.execute(
        """
        INSERT OR REPLACE INTO person_facts (
            person_id, channel, fact_type, fact_value, normalized_value,
            confidence, source_kind, source_ref, observed_at, raw_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            person_id,
            channel,
            fact_type,
            clean_value,
            clean_norm,
            confidence,
            source_kind,
            source_ref,
            observed_at,
            safe_json(raw or {}),
        ),
    )


def semantic_value_key(value: str) -> str:
    clean = normalize_text(value)
    if not clean:
        return ""
    lowered = clean.casefold()
    if re.fullmatch(r"[+\d][\d+]+", clean):
        return normalize_phone(clean)
    return lowered


def insert_semantic_claim(
    conn: sqlite3.Connection,
    *,
    person_id: str,
    channel: str,
    claim_type: str,
    predicate: str,
    object_value: str,
    claim_status: str = "accepted",
    confidence: float,
    source_kind: str,
    source_ref: str,
    source_message_id: str = "",
    source_chat_id: str = "",
    observed_at: str | None = None,
    raw: dict[str, Any] | None = None,
) -> None:
    clean_value = normalize_text(object_value)
    if not person_id or not clean_value:
        return
    conn.execute(
        """
        INSERT OR REPLACE INTO semantic_claims (
            person_id, channel, claim_type, predicate, object_value, normalized_value, claim_status,
            confidence, source_kind, source_ref, source_message_id, source_chat_id,
            observed_at, raw_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            person_id,
            channel,
            claim_type,
            predicate,
            clean_value,
            semantic_value_key(clean_value),
            claim_status,
            confidence,
            source_kind,
            source_ref,
            source_message_id,
            source_chat_id,
            observed_at,
            safe_json(raw or {}),
        ),
    )


def entity_key(entity_type: str, value: str) -> str:
    return f"{entity_type}:{slugify(value)}"


def upsert_entity(
    conn: sqlite3.Connection,
    *,
    entity_type: str,
    canonical_name: str,
    source_channel: str,
    confidence: float,
    metadata: dict[str, Any] | None = None,
) -> str:
    clean_name = normalize_text(canonical_name)
    if not clean_name:
        raise ValueError("entity canonical_name required")
    entity_id = entity_key(entity_type, clean_name)
    current = conn.execute("SELECT confidence, metadata_json FROM entities WHERE entity_id = ?", (entity_id,)).fetchone()
    merged_metadata = metadata or {}
    if current:
        try:
            existing_metadata = json.loads(current["metadata_json"])
        except json.JSONDecodeError:
            existing_metadata = {}
        merged_metadata = {**existing_metadata, **merged_metadata}
    conn.execute(
        """
        INSERT INTO entities (
            entity_id, entity_type, canonical_name, normalized_name, source_channel, confidence, metadata_json, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(entity_id) DO UPDATE SET
            canonical_name = excluded.canonical_name,
            normalized_name = excluded.normalized_name,
            source_channel = excluded.source_channel,
            confidence = CASE
                WHEN excluded.confidence > entities.confidence THEN excluded.confidence
                ELSE entities.confidence
            END,
            metadata_json = excluded.metadata_json,
            updated_at = excluded.updated_at
        """,
        (
            entity_id,
            entity_type,
            clean_name,
            semantic_value_key(clean_name),
            source_channel,
            confidence,
            safe_json(merged_metadata),
            now_utc().isoformat(),
        ),
    )
    return entity_id


def add_entity_alias(
    conn: sqlite3.Connection,
    *,
    entity_id: str,
    source_channel: str,
    alias_type: str,
    alias_value: str,
    source_kind: str,
    source_ref: str,
    observed_at: str | None = None,
    raw: dict[str, Any] | None = None,
) -> None:
    clean_alias = normalize_text(alias_value)
    if not entity_id or not clean_alias:
        return
    conn.execute(
        """
        INSERT OR REPLACE INTO entity_aliases (
            entity_id, source_channel, alias_type, alias_value, normalized_value,
            source_kind, source_ref, observed_at, raw_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            entity_id,
            source_channel,
            alias_type,
            clean_alias,
            semantic_value_key(clean_alias),
            source_kind,
            source_ref,
            observed_at,
            safe_json(raw or {}),
        ),
    )


def insert_relationship_edge(
    conn: sqlite3.Connection,
    *,
    subject_type: str,
    subject_ref: str,
    predicate: str,
    object_type: str,
    object_ref: str,
    channel: str,
    confidence: float,
    source_kind: str,
    source_ref: str,
    observed_at: str | None = None,
    raw: dict[str, Any] | None = None,
) -> None:
    if not subject_ref or not object_ref:
        return
    conn.execute(
        """
        INSERT OR REPLACE INTO relationship_edges (
            subject_type, subject_ref, predicate, object_type, object_ref,
            channel, confidence, source_kind, source_ref, observed_at, raw_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            subject_type,
            subject_ref,
            predicate,
            object_type,
            object_ref,
            channel,
            confidence,
            source_kind,
            source_ref,
            observed_at,
            safe_json(raw or {}),
        ),
    )


def resolve_ref_name(conn: sqlite3.Connection, entity_type: str, entity_ref: str) -> str:
    if entity_type == "owner" and entity_ref == OWNER_ENTITY_REF:
        return "Sunil Rao"
    if entity_type == "person":
        row = conn.execute("SELECT display_name FROM people WHERE person_id = ?", (entity_ref,)).fetchone()
        if row and row["display_name"]:
            return row["display_name"]
    row = conn.execute("SELECT canonical_name FROM entities WHERE entity_id = ?", (entity_ref,)).fetchone()
    if row and row["canonical_name"]:
        return row["canonical_name"]
    return entity_ref


def weekday_mentions(text: str) -> list[str]:
    return dedupe_preserve(
        re.findall(
            r"\b(monday|tuesday|wednesday|thursday|friday|saturday|sunday|tomorrow|today|next week|this week)\b",
            text,
            flags=re.IGNORECASE,
        )
    )


def clean_place_candidate(value: str) -> str:
    clean = normalize_text(value)
    if not clean:
        return ""
    clean = re.split(r"\b(if you|if u|around|later|tonight)\b", clean, flags=re.IGNORECASE)[0].strip(" .,;:")
    if not clean:
        return ""
    banned_exact = {"bar", "restaurant", "hotel"}
    banned_tokens = {"bar", "cafe", "caffe", "club", "grill", "hotel", "pub", "resort", "restaurant", "brewery"}
    lowered = clean.casefold()
    if lowered in banned_exact:
        return ""
    token_list = [token.casefold() for token in words(clean)]
    if len(token_list) > 4:
        return ""
    if any(token in banned_tokens for token in token_list):
        return ""
    if token_list and all(token in LOW_SIGNAL_WORDS for token in token_list):
        return ""
    return clean


def extract_message_semantic_claims(text: str) -> list[dict[str, Any]]:
    clean = normalize_text(text)
    lowered = clean.casefold()
    if not clean:
        return []

    claims: list[dict[str, Any]] = []

    travel_patterns = [
        r"\babout to land in ([A-Za-z][A-Za-z .'-]{2,40})",
        r"\blanded in ([A-Za-z][A-Za-z .'-]{2,40})",
        r"\bheading to ([A-Za-z][A-Za-z .'-]{2,40})",
        r"\bflying to ([A-Za-z][A-Za-z .'-]{2,40})",
    ]
    for pattern in travel_patterns:
        match = re.search(pattern, clean, flags=re.IGNORECASE)
        if match:
            place = clean_place_candidate(match.group(1))
            if place:
                claims.append(
                    {
                        "claim_type": "life_event",
                        "predicate": "travel_location",
                        "object_value": shorten_text(place, 60),
                        "confidence": 0.82,
                        "raw": {"excerpt": clean},
                    }
                )
                break

    if any(keyword in lowered for keyword in ("sick", "flu", "fever", "recover", "recovered", "hospital", "surgery", "injured")):
        claims.append(
            {
                "claim_type": "life_event",
                "predicate": "health_update",
                "object_value": shorten_text(clean, 140),
                "confidence": 0.78,
                "raw": {"excerpt": clean},
            }
        )

    if re.search(
        r"\b("
        r"vacay time|out of office|mandatory leave|on leave|"
        r"i(?:'m| am)? off\b|"
        r"i(?:'m| am)? on vacation\b|"
        r"i(?:'ll| will) be away\b|"
        r"i(?:'m| am)? away (?:til|till|until)\b|"
        r"off (?:til|till|until)\b|"
        r"away (?:til|till|until)\b"
        r")",
        lowered,
    ):
        claims.append(
            {
                "claim_type": "availability",
                "predicate": "out_of_office",
                "object_value": shorten_text(clean, 140),
                "confidence": 0.72,
                "raw": {"excerpt": clean},
            }
        )

    if re.search(r"\b(my wife|my husband|my partner|wife|husband|girlfriend|boyfriend)\b", lowered):
        claims.append(
            {
                "claim_type": "family_context",
                "predicate": "partner_context",
                "object_value": shorten_text(clean, 160),
                "confidence": 0.66,
                "raw": {"excerpt": clean},
            }
        )

    if any(keyword in lowered for keyword in ("baby", "son", "daughter", "kid", "kids", "child")):
        claims.append(
            {
                "claim_type": "family_context",
                "predicate": "child_context",
                "object_value": shorten_text(clean, 160),
                "confidence": 0.66,
                "raw": {"excerpt": clean},
            }
        )

    weekdays = weekday_mentions(lowered)
    if weekdays or any(phrase in lowered for phrase in ("can we connect", "let's connect", "call you tomorrow", "when tomorrow", "second half")):
        window = ", ".join(item.title() for item in weekdays) if weekdays else shorten_text(clean, 80)
        claims.append(
            {
                "claim_type": "coordination",
                "predicate": "availability_window",
                "object_value": window,
                "confidence": 0.7,
                "raw": {"excerpt": clean},
            }
        )

    if any(phrase in lowered for phrase in ("let me", "i will", "i'll", "we shall", "take it forward", "check the", "send the", "will send", "need to")):
        claims.append(
            {
                "claim_type": "commitment",
                "predicate": "follow_up",
                "object_value": shorten_text(clean, 160),
                "confidence": 0.68,
                "raw": {"excerpt": clean},
            }
        )

    return claims


def relationship_type_profile(value: str) -> tuple[str, str, str, int, float]:
    clean = normalize_text(value).lower()
    mapping = {
        "family": ("family", "family", "family", 14, 9.0),
        "parent": ("family", "family", "family", 14, 9.0),
        "close_friend": ("close", "network", "close_friend", 30, 8.0),
        "friend": ("active", "network", "friend", 60, 7.0),
        "colleague": ("active", "network", "colleague", 75, 6.5),
        "romantic_partner": ("family", "family", "partner", 7, 9.0),
        "partner": ("close", "network", "partner", 30, 7.5),
        "service_provider": ("service_provider", "service_provider", "service provider", 180, 4.0),
        "operations": ("operations", "operations", "operations", 7, 8.0),
    }
    return mapping.get(clean, ("active", "network", clean or "relationship", 90, 6.0))


def import_imessage_profiles(
    db_path: Path,
    profiles_dir: Path,
    *,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    conn = open_store(db_path)
    imported_profiles = 0
    linked_people = 0
    created_signals = 0
    semantic_claim_count = 0

    for path in sorted(profiles_dir.glob("*.json")):
        try:
            raw = path.read_bytes()
        except OSError:
            continue
        payload = None
        for encoding in ("utf-8", "utf-8-sig", "latin-1"):
            try:
                payload = json.loads(raw.decode(encoding))
                break
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
        if payload is None:
            continue

        identifier = normalize_text(payload.get("identifier"))
        likely_name = normalize_text(payload.get("likely_name"))
        summary = normalize_text(payload.get("summary"))
        relationship_type = normalize_text(payload.get("relationship_type"))
        relationship_strength = float(payload.get("relationship_strength") or 0)
        communication_pattern = normalize_text(payload.get("communication_pattern"))
        total_messages = int(payload.get("total_messages") or 0)
        first_message = normalize_text(payload.get("first_message"))
        last_message = normalize_text(payload.get("last_message"))
        key_topics = [normalize_text(item) for item in payload.get("key_topics") or []]
        key_facts = [normalize_text(item) for item in payload.get("key_facts") or []]
        notable_events = [normalize_text(item) for item in payload.get("notable_events") or []]
        action_items = [normalize_text(item) for item in payload.get("action_items") or []]

        phone = normalize_phone(identifier if identifier.startswith("+") or identifier.isdigit() else "")
        email = normalize_text(identifier).lower() if "@" in identifier else ""
        person_id = ensure_person_for_counterparty(
            conn,
            display_name=likely_name,
            phone=phone,
            email=email,
            channel="imessage",
        )
        if not person_id:
            continue
        linked_people += 1
        imported_profiles += 1

        row = conn.execute("SELECT * FROM people WHERE person_id = ?", (person_id,)).fetchone()
        if not row:
            continue

        tier, category, relationship_label, cadence_days, baseline_score = relationship_type_profile(relationship_type)
        existing_record = record_for_row(row)
        updates: dict[str, Any] = {
            "last_touch_date": best_iso_date(row["last_touch_date"], last_message),
            "built_at": now_utc().isoformat(),
        }
        if likely_name:
            updates["display_name"] = choose_display_name(row["display_name"], likely_name)
            updates["canonical_name"] = choose_display_name(row["canonical_name"], likely_name)
            updates["aliases_json"] = append_json_list(row["aliases_json"], [likely_name], limit=24)
        if phone:
            updates["phones_json"] = append_json_list(row["phones_json"], [phone], limit=24)
        if email:
            updates["emails_json"] = append_json_list(row["emails_json"], [email], limit=24)
        updates["topics_json"] = safe_json(
            clean_profile_topics([*json.loads(row["topics_json"]), *key_topics], limit=20)
        )
        note_fragments = []
        if summary:
            note_fragments.append(f"iMessage summary: {summary}")
        if communication_pattern:
            note_fragments.append(f"iMessage pattern: {communication_pattern}")
        if key_facts:
            note_fragments.extend(f"iMessage fact: {item}" for item in key_facts[:4])
        updates["notes_json"] = safe_json(
            clean_profile_notes([*json.loads(row["notes_json"]), *note_fragments], limit=24)
        )
        profile_ref = f"crm-imessage/profiles/{path.name}"
        imessage_actions = [
            {
                "description": item,
                "due_date": "",
                "status": "open",
                "priority": 3,
                "created_from": profile_ref,
            }
            for item in action_items
            if item
        ]
        updates["open_actions_json"] = append_json_objects(row["open_actions_json"], imessage_actions, limit=24)
        updates["relationship_score"] = max(float(row["relationship_score"]), relationship_strength or baseline_score)
        updates["preferred_channel"] = choose_channel(row["preferred_channel"], "sms" if phone else ("email" if email else "unknown"))
        updates["cadence_days"] = min(int(row["cadence_days"]), cadence_days) if int(row["cadence_days"]) else cadence_days
        updates["tier"] = better_tier(row["tier"], tier)
        updates["category"] = choose_category(row["category"], category)
        if not normalize_text(row["relationship_label"]) and relationship_label not in {"relationship", "friend", "colleague"}:
            updates["relationship_label"] = relationship_label
        assignments = ", ".join(f"{column} = ?" for column in updates)
        conn.execute(
            f"UPDATE people SET {assignments} WHERE person_id = ?",
            (*updates.values(), person_id),
        )
        maybe_insert_touch_event(
            conn,
            person_id=person_id,
            touched_at=last_message or now_utc().isoformat(),
            channel="imessage",
            note=summary or communication_pattern or "iMessage profile activity",
            direction="any",
            source="imessage-profile",
        )

        if likely_name:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel="imessage",
                fact_type="contact_name",
                fact_value=likely_name,
                normalized_value=likely_name,
                confidence=0.88,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=last_message or first_message,
            )
        if phone:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel="imessage",
                fact_type="channel_phone",
                fact_value=phone,
                normalized_value=phone,
                confidence=1.0,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=last_message or first_message,
            )
        if email:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel="imessage",
                fact_type="channel_email",
                fact_value=email,
                normalized_value=email,
                confidence=1.0,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=last_message or first_message,
            )

        claim_source_at = last_message or first_message
        if summary:
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="summary",
                predicate="profile_summary",
                object_value=summary,
                claim_status="observed",
                confidence=0.8,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
                raw={"identifier": identifier, "total_messages": total_messages},
            )
            semantic_claim_count += 1
        if relationship_type:
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="relationship",
                predicate="relationship_type",
                object_value=relationship_type,
                claim_status="observed",
                confidence=0.8,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
            )
            insert_relationship_edge(
                conn,
                subject_type="owner",
                subject_ref=OWNER_ENTITY_REF,
                predicate=relationship_label,
                object_type="person",
                object_ref=person_id,
                channel="imessage",
                confidence=0.75,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
                raw={"relationship_type": relationship_type},
            )
            semantic_claim_count += 1
        if communication_pattern:
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="communication",
                predicate="communication_pattern",
                object_value=communication_pattern,
                claim_status="observed",
                confidence=0.76,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
            )
            semantic_claim_count += 1
        for topic in key_topics:
            clean_topic_value = normalize_text(topic)
            if not clean_topic_value:
                continue
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="topic",
                predicate="mentioned_topic",
                object_value=clean_topic_value,
                claim_status="observed",
                confidence=0.72,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
            )
            semantic_claim_count += 1
        for fact in key_facts:
            clean_fact = normalize_text(fact)
            if not clean_fact:
                continue
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="profile",
                predicate="key_fact",
                object_value=clean_fact,
                claim_status="observed",
                confidence=0.7,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
            )
            semantic_claim_count += 1
        for event in notable_events:
            clean_event = normalize_text(event)
            if not clean_event:
                continue
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="event",
                predicate="notable_event",
                object_value=clean_event,
                claim_status="observed",
                confidence=0.7,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
            )
            semantic_claim_count += 1
        for action in action_items:
            clean_action = normalize_text(action)
            if not clean_action:
                continue
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="imessage",
                claim_type="commitment",
                predicate="open_loop",
                object_value=clean_action,
                claim_status="observed",
                confidence=0.74,
                source_kind="profile",
                source_ref=profile_ref,
                observed_at=claim_source_at,
            )
            semantic_claim_count += 1

        conn.execute(
            """
            INSERT OR REPLACE INTO relationship_signals (
                person_id, channel, direct_inbound_count, direct_outbound_count,
                group_inbound_count, group_outbound_count, first_seen_at, last_seen_at,
                last_direct_at, last_group_at, recent_excerpt, top_topics_json, raw_json, updated_at
            ) VALUES (?, 'imessage', 0, 0, 0, 0, ?, ?, ?, NULL, ?, ?, ?, ?)
            """,
            (
                person_id,
                first_message or claim_source_at,
                last_message or claim_source_at,
                last_message or claim_source_at,
                summary or communication_pattern or "",
                safe_json(clean_profile_topics(key_topics, limit=8)),
                safe_json(
                    {
                        "identifier": identifier,
                        "relationship_type": relationship_type,
                        "relationship_strength": relationship_strength,
                        "total_messages": total_messages,
                        "source": payload.get("source") or "imessage",
                        "profile_path": path.name,
                    }
                ),
                now_utc().isoformat(),
            ),
        )
        created_signals += 1

        updated_row = conn.execute("SELECT * FROM people WHERE person_id = ?", (person_id,)).fetchone()
        if updated_row:
            updated_record = record_for_row(updated_row)
            importance = compute_importance(updated_record)
            conn.execute("UPDATE people SET importance = ? WHERE person_id = ?", (importance, person_id))
            refresh_people_fts(conn, person_id)

    normalized_special_contacts = enforce_special_contacts(conn)
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "profiles_dir": str(profiles_dir),
        "imported_profiles": imported_profiles,
        "linked_people": linked_people,
        "relationship_signals": created_signals,
        "semantic_claims_added": semantic_claim_count,
        "normalized_special_contacts": normalized_special_contacts,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def slack_profile_name(payload: dict[str, Any]) -> str:
    profile = payload.get("profile") or {}
    return choose_display_name(
        normalize_text(profile.get("display_name") or profile.get("real_name") or payload.get("real_name")),
        normalize_text(profile.get("real_name") or payload.get("name")),
    )


def import_slack_archive(
    db_path: Path,
    archive_dir: Path,
    *,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    sqlite_path = archive_dir / "slackdump.sqlite"
    if not sqlite_path.exists():
        raise SystemExit(f"Slack archive not found: {sqlite_path}")
    source = sqlite3.connect(sqlite_path)
    source.row_factory = sqlite3.Row
    conn = open_store(db_path)

    workspace = source.execute("SELECT * FROM WORKSPACE ORDER BY ID DESC LIMIT 1").fetchone()
    if not workspace:
        raise SystemExit(f"No WORKSPACE row found in {sqlite_path}")
    workspace_data = safe_json_loads(workspace["DATA"], {})
    account_id = normalize_text(workspace_data.get("team_id") or workspace["TEAM_ID"] or "")
    owner_user_id = normalize_text(workspace_data.get("user_id") or workspace["USER_ID"] or "")

    channel_members: dict[str, list[str]] = defaultdict(list)
    for row in source.execute("SELECT CHANNEL_ID, USER_ID FROM CHANNEL_USER").fetchall():
        channel_members[normalize_text(row["CHANNEL_ID"])].append(normalize_text(row["USER_ID"]))

    user_payloads: dict[str, dict[str, Any]] = {}
    imported_contacts = 0
    linked_people: set[str] = set()
    for row in source.execute("SELECT ID, USERNAME, DATA FROM S_USER").fetchall():
        payload = safe_json_loads(decode_bytes(row["DATA"]), {})
        user_id = normalize_text(payload.get("id") or row["ID"])
        if not user_id:
            continue
        user_payloads[user_id] = payload
        display_name = slack_profile_name(payload)
        profile = payload.get("profile") or {}
        email = normalize_email(profile.get("email"))
        phone = normalize_phone(profile.get("phone"))
        upsert_channel_contact(
            conn,
            {
                "channel": "slack",
                "account_id": account_id,
                "contact_id": user_id,
                "display_name": display_name,
                "short_name": normalize_text(payload.get("name") or row["USERNAME"]),
                "phone": phone,
                "email": email,
                "raw": payload,
                "updated_at": now_utc().isoformat(),
            },
        )
        person_id = ensure_person_for_counterparty(
            conn,
            display_name=display_name,
            phone=phone,
            email=email,
            channel="slack",
        )
        if person_id:
            linked_people.add(person_id)
        imported_contacts += 1

    channels: dict[str, dict[str, Any]] = {}
    imported_threads = 0
    for row in source.execute("SELECT ID, NAME, DATA FROM CHANNEL").fetchall():
        payload = safe_json_loads(decode_bytes(row["DATA"]), {})
        channel_id = normalize_text(payload.get("id") or row["ID"])
        if not channel_id:
            continue
        channels[channel_id] = payload
        members = channel_members.get(channel_id, [])
        is_im = bool(payload.get("is_im"))
        is_mpim = bool(payload.get("is_mpim"))
        direct_partner_id = ""
        if is_im:
            for member_id in members:
                if member_id and member_id != owner_user_id:
                    direct_partner_id = member_id
                    break
        direct_partner = user_payloads.get(direct_partner_id, {})
        direct_partner_name = slack_profile_name(direct_partner)
        direct_partner_email = normalize_email((direct_partner.get("profile") or {}).get("email"))
        thread_name = normalize_text(payload.get("name") or payload.get("name_normalized"))
        if is_im:
            thread_name = direct_partner_name or thread_name or direct_partner_id
        elif is_mpim and members:
            participant_names = [slack_profile_name(user_payloads.get(item, {})) for item in members if item != owner_user_id]
            thread_name = ", ".join(name for name in participant_names if name)[:180] or thread_name or channel_id
        upsert_conversation_thread(
            conn,
            {
                "channel": "slack",
                "account_id": account_id,
                "chat_id": channel_id,
                "chat_name": thread_name or channel_id,
                "chat_phone": "",
                "is_group": not is_im,
                "last_message_at": None,
                "raw": {
                    **payload,
                    "direct_partner_id": direct_partner_id,
                    "direct_partner_name": direct_partner_name,
                    "direct_partner_email": direct_partner_email,
                },
                "updated_at": now_utc().isoformat(),
            },
        )
        imported_threads += 1

    imported_messages = 0
    for row in source.execute(
        "SELECT CHANNEL_ID, TS, THREAD_TS, TXT, DATA FROM MESSAGE ORDER BY CHANNEL_ID ASC, TS ASC"
    ):
        payload = safe_json_loads(decode_bytes(row["DATA"]), {})
        channel_id = normalize_text(row["CHANNEL_ID"])
        channel_payload = channels.get(channel_id, {})
        is_im = bool(channel_payload.get("is_im"))
        is_group = not is_im
        sender_id = normalize_text(payload.get("user") or payload.get("bot_id"))
        sender_payload = user_payloads.get(sender_id, {})
        sender_name = slack_profile_name(sender_payload) or normalize_text(payload.get("username")) or "Slack"
        sender_email = normalize_email((sender_payload.get("profile") or {}).get("email"))
        sender_phone = normalize_phone((sender_payload.get("profile") or {}).get("phone"))
        direction = "outbound" if sender_id and sender_id == owner_user_id else "inbound"
        members = channel_members.get(channel_id, [])
        counterpart_id = ""
        if is_im:
            for member_id in members:
                if member_id and member_id != owner_user_id:
                    counterpart_id = member_id
                    break
        counterpart_payload = user_payloads.get(counterpart_id, {})
        counterpart_name = slack_profile_name(counterpart_payload)
        counterpart_email = normalize_email((counterpart_payload.get("profile") or {}).get("email"))
        counterpart_phone = normalize_phone((counterpart_payload.get("profile") or {}).get("phone"))
        person_id = None
        if is_im:
            person_id = ensure_person_for_counterparty(
                conn,
                display_name=counterpart_name,
                phone=counterpart_phone,
                email=counterpart_email,
                channel="slack",
            )
        elif direction == "inbound":
            person_id = ensure_person_for_counterparty(
                conn,
                display_name=sender_name,
                phone=sender_phone,
                email=sender_email,
                channel="slack",
            )
        if person_id:
            linked_people.add(person_id)
        text = normalize_text(payload.get("text") or row["TXT"])
        sent_at = datetime.fromtimestamp(float(normalize_text(row["TS"])), tz=timezone.utc).isoformat()
        thread_name = normalize_text(channel_payload.get("name") or channel_payload.get("name_normalized"))
        if is_im:
            thread_name = counterpart_name or thread_name or channel_id
        cursor = conn.execute(
            """
            INSERT OR IGNORE INTO message_events (
                channel, account_id, chat_id, chat_name, chat_phone, is_group, message_id,
                sender_id, sender_name, sender_phone, sender_email, counterpart_name, counterpart_phone, counterpart_email,
                person_id, direction, sent_at, message_type, text, excerpt, is_history, raw_json, imported_at
            ) VALUES (?, ?, ?, ?, '', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
            """,
            (
                "slack",
                account_id,
                channel_id,
                thread_name or channel_id,
                1 if is_group else 0,
                f"{channel_id}:{normalize_text(row['TS'])}",
                sender_id,
                sender_name,
                sender_phone,
                sender_email,
                counterpart_name,
                counterpart_phone,
                counterpart_email,
                person_id,
                direction,
                sent_at,
                normalize_text(payload.get("subtype")) or "message",
                text,
                trim_message_excerpt(text or normalize_text(payload.get("subtype")) or "Slack message"),
                safe_json(payload),
                now_utc().isoformat(),
            ),
        )
        if cursor.rowcount <= 0:
            continue
        imported_messages += 1
        if person_id:
            maybe_insert_touch_event(
                conn,
                person_id=person_id,
                touched_at=sent_at,
                channel="slack",
                note=text or "Slack message",
                direction=direction,
                source="slack-import",
            )

    ontology = rebuild_message_channel_ontology(conn, "slack")
    normalized_special_contacts = enforce_special_contacts(conn)
    conn.commit()
    conn.close()
    source.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "archive_dir": str(archive_dir),
        "workspace": normalize_text(workspace_data.get("team") or workspace_data.get("url")),
        "imported_contacts": imported_contacts,
        "imported_threads": imported_threads,
        "imported_messages": imported_messages,
        "linked_people": len(linked_people),
        "normalized_special_contacts": normalized_special_contacts,
        "ontology": ontology,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def gmail_headers(payload: dict[str, Any]) -> dict[str, str]:
    headers = (payload.get("payload") or {}).get("headers") or []
    return {normalize_text(item.get("name")).lower(): normalize_text(item.get("value")) for item in headers}


def gmail_sent_at(payload: dict[str, Any], headers: dict[str, str]) -> str:
    internal_ms = normalize_text(payload.get("internalDate"))
    if internal_ms.isdigit():
        return datetime.fromtimestamp(int(internal_ms) / 1000, tz=timezone.utc).isoformat()
    date_header = headers.get("date", "")
    if date_header:
        try:
            parsed = parsedate_to_datetime(date_header)
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=timezone.utc)
            return parsed.astimezone(timezone.utc).isoformat()
        except (TypeError, ValueError):
            pass
    return now_utc().isoformat()


def gmail_query_literal(value: str) -> str:
    clean = normalize_text(value).replace('"', "").strip()
    if not clean:
        return ""
    return f'"{clean}"' if " " in clean else clean


def extract_guided_email_terms(text: str, *, limit: int = 6) -> list[str]:
    clean = normalize_text(text)
    if not clean:
        return []
    quoted = [normalize_text(item) for item in re.findall(r'"([^"]+)"', clean)]
    scored_terms: list[str] = []
    for phrase in quoted:
        if phrase and phrase.casefold() not in EMAIL_GUIDED_STOPWORDS and len(words(phrase)) <= 5:
            scored_terms.append(phrase)
    token_candidates: list[str] = []
    for token in words(clean):
        lowered = token.casefold()
        if (
            len(lowered) <= 2
            or lowered in EMAIL_GUIDED_STOPWORDS
            or lowered.isdigit()
            or lowered.startswith("http")
        ):
            continue
        token_candidates.append(token)
    return dedupe_preserve([*scored_terms, *token_candidates])[:limit]


def row_email_domains(row: sqlite3.Row) -> list[str]:
    domains: list[str] = []
    for email in json.loads(row["emails_json"]):
        clean = normalize_email(email)
        if not clean or "@" not in clean:
            continue
        domain = clean.split("@", 1)[1].casefold()
        if domain.endswith(("gmail.com", "icloud.com", "me.com", "yahoo.com", "hotmail.com", "outlook.com")):
            continue
        domains.append(domain)
    return dedupe_preserve(domains)


def guided_gmail_seed_rows(
    db_path: Path,
    *,
    person_query: str = "",
    objective: str = "",
    limit_people: int = 6,
) -> list[sqlite3.Row]:
    rows: list[sqlite3.Row] = []
    seen: set[str] = set()

    def add_row(row: sqlite3.Row | None) -> None:
        if not row:
            return
        person_id = normalize_text(row["person_id"])
        if not person_id or person_id in seen:
            return
        seen.add(person_id)
        rows.append(row)

    if normalize_text(person_query):
        add_row(resolve_person(db_path, person_query))

    search_targets = dedupe_preserve([normalize_text(objective), normalize_text(person_query)])
    for target in search_targets:
        if not target:
            continue
        for candidate in search_people(db_path, target, limit_people):
            add_row(resolve_person(db_path, candidate["person_id"]))
            if len(rows) >= limit_people:
                return rows

    if not rows:
        for candidate in weekly_brief(db_path, reconnect_limit=max(3, limit_people), loop_limit=max(3, limit_people), date_limit=2).get("priority_reconnect", []):
            add_row(resolve_person(db_path, candidate["person_id"]))
            if len(rows) >= limit_people:
                break

    return rows[:limit_people]


def build_guided_gmail_queries(
    db_path: Path,
    *,
    person_query: str = "",
    objective: str = "",
    days: int = 365,
    limit_people: int = 6,
    query_limit: int = 10,
) -> dict[str, Any]:
    seed_rows = guided_gmail_seed_rows(db_path, person_query=person_query, objective=objective, limit_people=limit_people)
    objective_terms = extract_guided_email_terms(objective, limit=6)
    doc_hits = search_documents(db_path, objective, limit=6, channel="all") if normalize_text(objective) else []
    if normalize_text(objective) and not doc_hits:
        conn = open_store(db_path)
        promoted = promoted_documents(conn, limit=12)
        conn.close()
        lowered_objective_terms = [term.casefold() for term in objective_terms]
        fallback_hits = [
            item
            for item in promoted
            if any(term in f"{item.get('title', '')} {item.get('excerpt', '')}".casefold() for term in lowered_objective_terms)
        ]
        doc_hits = fallback_hits[:6] if fallback_hits else promoted[:4]
    doc_terms = extract_guided_email_terms(
        " ".join([item.get("title", "") for item in doc_hits] + [item.get("excerpt", "") for item in doc_hits[:2]]),
        limit=6,
    )
    shared_terms = dedupe_preserve([*objective_terms, *doc_terms])[:6]
    queries: list[dict[str, Any]] = []
    seen_queries: set[str] = set()

    def add_query(query_text: str, *, reasons: list[str], person_row: sqlite3.Row | None = None, terms: list[str] | None = None) -> None:
        clean_query = normalize_text(query_text)
        if not clean_query or clean_query in seen_queries:
            return
        seen_queries.add(clean_query)
        payload: dict[str, Any] = {
            "query": clean_query,
            "reasons": dedupe_preserve(reasons),
            "terms": terms or [],
        }
        if person_row is not None:
            payload["person"] = {
                "person_id": person_row["person_id"],
                "display_name": person_row["display_name"],
                "importance": float(person_row["importance"]),
                "relationship_score": float(person_row["relationship_score"]),
                "emails": json.loads(person_row["emails_json"]),
                "organizations": json.loads(person_row["organizations_json"]),
            }
        queries.append(payload)

    for row in seed_rows:
        record = record_for_row(row)
        person_text = " ".join(
            [
                record.display_name,
                *record.aliases,
                *record.organizations,
                *record.topics,
                " ".join(item.get("description", "") for item in record.open_actions),
            ]
        )
        person_terms = extract_guided_email_terms(person_text, limit=6)
        overlap_terms = [term for term in shared_terms if term.casefold() in person_text.casefold()]
        if normalize_text(objective) and not normalize_text(person_query) and not overlap_terms:
            continue
        focused_terms = (
            dedupe_preserve([*shared_terms, *person_terms])[:5]
            if normalize_text(person_query)
            else dedupe_preserve(overlap_terms)[:5]
        )
        date_clause = f"newer_than:{max(days, 1)}d"
        contact_reasons = [f"graph person: {record.display_name}", f"importance={round(record.importance, 2)}"]
        direct_emails = [email for email in record.emails if email not in OWNER_EMAILS][:2]
        for email in direct_emails:
            base = f"(from:{email} OR to:{email} OR cc:{email} OR bcc:{email}) {date_clause}"
            if focused_terms:
                base = f"{base} ({' OR '.join(gmail_query_literal(term) for term in focused_terms[:4])})"
            add_query(base, reasons=[*contact_reasons, f"direct email: {email}", *focused_terms[:3]], person_row=row, terms=focused_terms)
        if record.display_name and not is_probably_noise_name(record.display_name):
            name_query = f"{gmail_query_literal(record.display_name)} {date_clause}"
            if focused_terms:
                name_query = f"{name_query} ({' OR '.join(gmail_query_literal(term) for term in focused_terms[:4])})"
            add_query(name_query, reasons=[*contact_reasons, "name match"], person_row=row, terms=focused_terms)
        for domain in row_email_domains(row)[:1]:
            domain_query = f"({domain}) {date_clause}"
            if focused_terms:
                domain_query = f"{domain_query} ({' OR '.join(gmail_query_literal(term) for term in focused_terms[:3])})"
            add_query(domain_query, reasons=[*contact_reasons, f"domain: {domain}"], person_row=row, terms=focused_terms)
        if len(queries) >= query_limit:
            break

    if shared_terms:
        base = f"{' OR '.join(gmail_query_literal(term) for term in shared_terms[:5])} newer_than:{max(days, 1)}d"
        add_query(base, reasons=[f"objective: {normalize_text(objective) or 'operating-state'}", *shared_terms[:3]], terms=shared_terms)
    doc_title_terms = [gmail_query_literal(item["title"]) for item in doc_hits[:3] if normalize_text(item.get("title"))]
    if doc_title_terms:
        doc_query = f"({' OR '.join(doc_title_terms)}) newer_than:{max(days, 1)}d"
        if shared_terms:
            doc_query = f"{doc_query} ({' OR '.join(gmail_query_literal(term) for term in shared_terms[:3])})"
        add_query(doc_query, reasons=["promoted docs", *(item["title"] for item in doc_hits[:3])], terms=shared_terms)

    return {
        "seed_people": [
            {
                "person_id": row["person_id"],
                "display_name": row["display_name"],
                "importance": float(row["importance"]),
                "relationship_score": float(row["relationship_score"]),
                "emails": json.loads(row["emails_json"]),
                "organizations": json.loads(row["organizations_json"]),
            }
            for row in seed_rows
        ],
        "objective_terms": shared_terms,
        "document_hits": doc_hits,
        "queries": queries[:query_limit],
    }


def guided_gmail_results(
    db_path: Path,
    *,
    account_email: str | None = None,
    person_query: str = "",
    objective: str = "",
    days: int = 365,
    limit: int = 20,
    query_limit: int = 8,
) -> dict[str, Any]:
    account, access_token = google_access_token(account_email)
    plan = build_guided_gmail_queries(
        db_path,
        person_query=person_query,
        objective=objective,
        days=days,
        query_limit=query_limit,
    )
    queries = plan["queries"]
    seed_people_by_id = {item["person_id"]: item for item in plan["seed_people"]}
    if not queries:
        return {
            "account_email": account,
            "person_query": normalize_text(person_query),
            "objective": normalize_text(objective),
            "days": days,
            "queries": [],
            "results": [],
        }

    matched_messages: dict[str, dict[str, Any]] = {}
    for query_item in queries:
        page = google_api_json_with_token(
            account,
            access_token,
            "https://gmail.googleapis.com/gmail/v1/users/me/messages",
            params={
                "q": query_item["query"],
                "maxResults": max(10, min(limit * 3, 50)),
            },
        )
        for message in page.get("messages") or []:
            message_id = normalize_text(message.get("id"))
            if not message_id:
                continue
            bucket = matched_messages.setdefault(
                message_id,
                {
                    "message_id": message_id,
                    "thread_id": normalize_text(message.get("threadId")),
                    "matched_queries": [],
                    "matched_people": [],
                    "matched_terms": [],
                    "query_reasons": [],
                },
            )
            bucket["matched_queries"].append(query_item["query"])
            bucket["matched_terms"] = dedupe_preserve([*bucket["matched_terms"], *(query_item.get("terms") or [])])
            bucket["query_reasons"] = dedupe_preserve([*bucket["query_reasons"], *(query_item.get("reasons") or [])])

    if not matched_messages:
        return {
            "account_email": account,
            "person_query": normalize_text(person_query),
            "objective": normalize_text(objective),
            "days": days,
            "queries": queries,
            "results": [],
        }

    def fetch_message(message_id: str) -> dict[str, Any]:
        return google_api_json_with_token(
            account,
            access_token,
            f"https://gmail.googleapis.com/gmail/v1/users/me/messages/{message_id}",
            params={
                "format": "metadata",
                "metadataHeaders": ["From", "To", "Cc", "Bcc", "Subject", "Date"],
            },
        )

    payloads: list[dict[str, Any]] = []
    with ThreadPoolExecutor(max_workers=8) as executor:
        futures = {executor.submit(fetch_message, message_id): message_id for message_id in matched_messages}
        for future in as_completed(futures):
            try:
                payloads.append(future.result())
            except Exception:
                continue

    results: list[dict[str, Any]] = []
    for payload in payloads:
        message_id = normalize_text(payload.get("id"))
        match_state = matched_messages.get(message_id)
        if not match_state:
            continue
        headers = gmail_headers(payload)
        subject = headers.get("subject") or "(no subject)"
        snippet = normalize_text(payload.get("snippet"))
        sent_at = gmail_sent_at(payload, headers)
        from_addresses = parse_address_list(headers.get("from", ""))
        to_addresses = parse_address_list(headers.get("to", ""))
        cc_addresses = parse_address_list(headers.get("cc", ""))
        bcc_addresses = parse_address_list(headers.get("bcc", ""))
        sender_name, sender_email = from_addresses[0] if from_addresses else ("", "")
        direction = "outbound" if sender_email in OWNER_EMAILS else "inbound"
        all_addresses = [*from_addresses, *to_addresses, *cc_addresses, *bcc_addresses]
        all_emails = {email for _, email in all_addresses if email}
        matched_people = list(match_state["matched_people"])
        why: list[str] = list(match_state.get("query_reasons") or [])
        score = float(len(match_state["matched_queries"]) * 8 + len(match_state["matched_terms"]) * 2)
        for person_id, person in seed_people_by_id.items():
            person_emails = {normalize_email(email) for email in person.get("emails") or []}
            person_name = normalize_text(person.get("display_name"))
            if person_emails & all_emails or (person_name and person_name.casefold() in f"{subject} {snippet} {headers.get('from', '')} {headers.get('to', '')}".casefold()):
                if person_id not in matched_people:
                    matched_people.append(person_id)
                why.append(f"matched person {person_name}")
                score += 25 + float(person.get("importance") or 0)
        objective_hits = [
            term for term in match_state["matched_terms"]
            if term.casefold() in f"{subject} {snippet}".casefold()
        ]
        if objective_hits:
            why.append("objective terms: " + ", ".join(objective_hits[:3]))
            score += len(objective_hits) * 5
        if normalize_text(person_query) and not matched_people and not objective_hits:
            continue
        sent_dt = parse_date(sent_at)
        if sent_dt:
            age_days = max(0, int((now_utc() - sent_dt).total_seconds() // 86400))
            score += max(0.0, 30.0 - min(age_days, 30)) / 3.0
        results.append(
            {
                "message_id": message_id,
                "thread_id": normalize_text(payload.get("threadId")),
                "sent_at": sent_at,
                "direction": direction,
                "subject": subject,
                "snippet": snippet,
                "from": [{"name": name, "email": email} for name, email in from_addresses],
                "to": [{"name": name, "email": email} for name, email in to_addresses],
                "cc": [{"name": name, "email": email} for name, email in cc_addresses],
                "matched_queries": dedupe_preserve(match_state["matched_queries"]),
                "matched_terms": objective_hits or match_state["matched_terms"],
                "matched_people": [
                    seed_people_by_id[person_id]["display_name"]
                    for person_id in matched_people
                    if person_id in seed_people_by_id
                ],
                "score": round(score, 2),
                "why": dedupe_preserve(why)[:6],
            }
        )

    results.sort(key=lambda item: (-float(item["score"]), item["sent_at"] or ""), reverse=False)
    results = results[:limit]
    return {
        "account_email": account,
        "person_query": normalize_text(person_query),
        "objective": normalize_text(objective),
        "days": days,
        "seed_people": plan["seed_people"],
        "objective_terms": plan["objective_terms"],
        "document_hits": plan["document_hits"],
        "queries": queries,
        "results": results,
    }


def import_google_gmail(
    db_path: Path,
    *,
    account_email: str | None = None,
    query: str = "in:anywhere",
    max_messages: int = 0,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    account, _ = google_access_token(account_email)
    conn = open_store(db_path)
    imported_messages = 0
    imported_threads = 0
    imported_contacts = 0
    failed_messages = 0
    linked_people: set[str] = set()
    next_page_token = ""
    processed = 0
    seen_threads: set[str] = set()

    while True:
        page = google_api_json(
            account,
            "https://gmail.googleapis.com/gmail/v1/users/me/messages",
            params={
                "q": query,
                "maxResults": 500,
                "pageToken": next_page_token,
                "includeSpamTrash": "true",
            },
        )
        messages = page.get("messages") or []
        if not messages:
            break
        if max_messages:
            remaining = max_messages - processed
            if remaining <= 0:
                break
            messages = messages[:remaining]
        processed += len(messages)
        _, access_token = google_access_token(account)

        def fetch_message(message_id: str) -> dict[str, Any]:
            return google_api_json_with_token(
                account,
                access_token,
                f"https://gmail.googleapis.com/gmail/v1/users/me/messages/{message_id}",
                params={
                    "format": "metadata",
                    "metadataHeaders": ["From", "To", "Cc", "Bcc", "Subject", "Date"],
                },
            )

        payloads: list[dict[str, Any]] = []
        with ThreadPoolExecutor(max_workers=8) as executor:
            futures = {executor.submit(fetch_message, message["id"]): message["id"] for message in messages}
            for future in as_completed(futures):
                try:
                    payloads.append(future.result())
                except Exception:
                    failed_messages += 1

        payloads.sort(key=lambda item: normalize_text(item.get("internalDate")) or normalize_text(item.get("id")))
        for payload in payloads:
            headers = gmail_headers(payload)
            sent_at = gmail_sent_at(payload, headers)
            from_addresses = parse_address_list(headers.get("from", ""))
            to_addresses = parse_address_list(headers.get("to", ""))
            cc_addresses = parse_address_list(headers.get("cc", ""))
            bcc_addresses = parse_address_list(headers.get("bcc", ""))
            all_external = [
                (name, email)
                for name, email in [*from_addresses, *to_addresses, *cc_addresses, *bcc_addresses]
                if email and email not in OWNER_EMAILS
            ]
            if not all_external:
                continue
            primary_email = all_external[0][1].casefold() if all_external and all_external[0][1] else ""
            if any(pattern in primary_email for pattern in LOW_SIGNAL_EMAIL_PATTERNS):
                continue
            sender_name, sender_email = from_addresses[0] if from_addresses else ("", "")
            direction = "outbound" if sender_email in OWNER_EMAILS else "inbound"
            external_recipients = [
                (name, email)
                for name, email in [*to_addresses, *cc_addresses, *bcc_addresses]
                if email and email not in OWNER_EMAILS
            ]
            is_group = len({email for _, email in all_external}) > 1
            counterpart_name = ""
            counterpart_email = ""
            person_id = None
            if direction == "inbound":
                counterpart_name, counterpart_email = sender_name, sender_email
                person_id = ensure_person_for_counterparty(
                    conn,
                    display_name=sender_name,
                    phone="",
                    email=sender_email,
                    channel="email",
                )
            elif not is_group and external_recipients:
                counterpart_name, counterpart_email = external_recipients[0]
                person_id = ensure_person_for_counterparty(
                    conn,
                    display_name=counterpart_name,
                    phone="",
                    email=counterpart_email,
                    channel="email",
                )
            if person_id:
                linked_people.add(person_id)
            seen_contacts: set[str] = set()
            for parsed_name, parsed_email in all_external:
                if parsed_email in seen_contacts:
                    continue
                seen_contacts.add(parsed_email)
                upsert_channel_contact(
                    conn,
                    {
                        "channel": "email",
                        "account_id": account,
                        "contact_id": parsed_email,
                        "display_name": parsed_name,
                        "short_name": parsed_name.split(" ", 1)[0] if parsed_name else parsed_email,
                        "phone": "",
                        "email": parsed_email,
                        "raw": {"source": "gmail", "account": account},
                        "updated_at": sent_at,
                    },
                )
                imported_contacts += 1
            thread_id = normalize_text(payload.get("threadId") or message["id"])
            subject = headers.get("subject") or counterpart_name or counterpart_email or thread_id
            if thread_id not in seen_threads:
                upsert_conversation_thread(
                    conn,
                    {
                        "channel": "email",
                        "account_id": account,
                        "chat_id": thread_id,
                        "chat_name": subject,
                        "chat_phone": "",
                        "is_group": is_group,
                        "last_message_at": sent_at,
                        "raw": {"subject": subject, "account": account},
                        "updated_at": sent_at,
                    },
                )
                seen_threads.add(thread_id)
                imported_threads += 1
            cursor = conn.execute(
                """
                INSERT OR IGNORE INTO message_events (
                    channel, account_id, chat_id, chat_name, chat_phone, is_group, message_id,
                    sender_id, sender_name, sender_phone, sender_email, counterpart_name, counterpart_phone, counterpart_email,
                    person_id, direction, sent_at, message_type, text, excerpt, is_history, raw_json, imported_at
                ) VALUES (?, ?, ?, ?, '', ?, ?, '', ?, '', ?, ?, '', ?, ?, ?, ?, 'email', ?, ?, 1, ?, ?)
                """,
                (
                    "email",
                    account,
                    thread_id,
                    subject,
                    1 if is_group else 0,
                    normalize_text(payload.get("id")),
                    sender_name,
                    sender_email,
                    counterpart_name,
                    counterpart_email,
                    person_id,
                    direction,
                    sent_at,
                    normalize_text(payload.get("snippet")),
                    trim_message_excerpt(payload.get("snippet") or subject or "Email"),
                    safe_json(
                        {
                            "id": payload.get("id"),
                            "threadId": payload.get("threadId"),
                            "labelIds": payload.get("labelIds") or [],
                            "headers": headers,
                        }
                    ),
                    now_utc().isoformat(),
                ),
            )
            if cursor.rowcount <= 0:
                continue
            imported_messages += 1
            if person_id:
                maybe_insert_touch_event(
                    conn,
                    person_id=person_id,
                    touched_at=sent_at,
                    channel="email",
                    note=payload.get("snippet") or subject or "Email",
                    direction=direction,
                    source="gmail-import",
                )
        conn.commit()
        next_page_token = normalize_text(page.get("nextPageToken"))
        if not next_page_token:
            break

    ontology = rebuild_message_channel_ontology(conn, "email")
    normalized_special_contacts = enforce_special_contacts(conn)
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "account_email": account,
        "query": query,
        "imported_messages": imported_messages,
        "imported_threads": imported_threads,
        "imported_contacts": imported_contacts,
        "failed_messages": failed_messages,
        "linked_people": len(linked_people),
        "normalized_special_contacts": normalized_special_contacts,
        "ontology": ontology,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def import_google_calendar(
    db_path: Path,
    *,
    account_email: str | None = None,
    past_days: int = 365,
    future_days: int = 180,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    account, _ = google_access_token(account_email)
    conn = open_store(db_path)
    clear_source_documents_channel(conn, "calendar")
    conn.execute("DELETE FROM semantic_claims WHERE channel = 'calendar'")
    conn.execute("DELETE FROM person_facts WHERE channel = 'calendar'")
    conn.execute("DELETE FROM relationship_edges WHERE channel = 'calendar'")
    conn.execute("DELETE FROM touch_events WHERE channel = 'calendar' AND source = 'calendar-import'")
    calendars = google_api_json(account, "https://www.googleapis.com/calendar/v3/users/me/calendarList").get("items") or []
    imported_events = 0
    linked_people: set[str] = set()
    touched_people: set[str] = set()
    time_min = (now_utc() - timedelta(days=past_days)).isoformat()
    time_max = (now_utc() + timedelta(days=future_days)).isoformat()
    for calendar in calendars:
        calendar_id = normalize_text(calendar.get("id"))
        if not calendar_id:
            continue
        next_page_token = ""
        while True:
            page = google_api_json(
                account,
                f"https://www.googleapis.com/calendar/v3/calendars/{urlparse.quote(calendar_id, safe='')}/events",
                params={
                    "singleEvents": "true",
                    "orderBy": "startTime",
                    "timeMin": time_min,
                    "timeMax": time_max,
                    "maxResults": 250,
                    "pageToken": next_page_token,
                },
            )
            for event in page.get("items") or []:
                event_id = normalize_text(event.get("id"))
                if not event_id:
                    continue
                start = normalize_text((event.get("start") or {}).get("dateTime") or (event.get("start") or {}).get("date"))
                end = normalize_text((event.get("end") or {}).get("dateTime") or (event.get("end") or {}).get("date"))
                summary = normalize_text(event.get("summary")) or "Calendar event"
                description = normalize_text(event.get("description"))
                body = "\n\n".join(part for part in [summary, description] if part)
                upsert_source_document(
                    conn,
                    source_channel="calendar",
                    source_kind="event",
                    doc_id=f"{calendar_id}:{event_id}",
                    title=summary,
                    body=body,
                    url=normalize_text(event.get("htmlLink")),
                    author=normalize_text((event.get("organizer") or {}).get("email")),
                    created_at=normalize_text(event.get("created")),
                    updated_at=normalize_text(event.get("updated")) or start,
                    metadata={
                        "calendar_id": calendar_id,
                        "location": normalize_text(event.get("location")),
                        "attendees": event.get("attendees") or [],
                        "status": normalize_text(event.get("status")),
                    },
                )
                imported_events += 1
                is_future = bool(parse_date(start) and parse_date(start) > now_utc())
                for attendee in event.get("attendees") or []:
                    attendee_email = normalize_email(attendee.get("email"))
                    if not attendee_email or attendee_email in OWNER_EMAILS:
                        continue
                    attendee_name = normalize_text(attendee.get("displayName")) or attendee_email.split("@", 1)[0]
                    person_id = ensure_person_for_counterparty(
                        conn,
                        display_name=attendee_name,
                        phone="",
                        email=attendee_email,
                        channel="calendar",
                    )
                    if not person_id:
                        continue
                    linked_people.add(person_id)
                    insert_person_fact(
                        conn,
                        person_id=person_id,
                        channel="calendar",
                        fact_type="channel_email",
                        fact_value=attendee_email,
                        normalized_value=attendee_email,
                        confidence=1.0,
                        source_kind="event",
                        source_ref=f"{calendar_id}:{event_id}",
                        observed_at=start,
                    )
                    insert_semantic_claim(
                        conn,
                        person_id=person_id,
                        channel="calendar",
                        claim_type="coordination",
                        predicate="upcoming_meeting" if is_future else "met_in_meeting",
                        object_value=summary,
                        claim_status="observed",
                        confidence=0.8,
                        source_kind="event",
                        source_ref=f"{calendar_id}:{event_id}",
                        observed_at=start,
                        raw={"calendar_id": calendar_id, "event_id": event_id},
                    )
                    if not is_future:
                        maybe_insert_touch_event(
                            conn,
                            person_id=person_id,
                            touched_at=start or end or now_utc().isoformat(),
                            channel="calendar",
                            note=summary,
                            direction="any",
                            source="calendar-import",
                        )
                        touched_people.add(person_id)
            next_page_token = normalize_text(page.get("nextPageToken"))
            if not next_page_token:
                break
    updated_people = recalculate_people(conn, touched_people | linked_people)
    normalized_special_contacts = enforce_special_contacts(conn)
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "account_email": account,
        "imported_events": imported_events,
        "linked_people": len(linked_people),
        "updated_people": updated_people,
        "normalized_special_contacts": normalized_special_contacts,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def drive_file_body(account_email: str | None, payload: dict[str, Any]) -> str:
    file_id = normalize_text(payload.get("id"))
    mime_type = normalize_text(payload.get("mimeType"))
    if not file_id:
        return ""
    exportable = {
        "application/vnd.google-apps.document": "text/plain",
    }
    downloadable = {
        "text/plain",
        "text/markdown",
        "application/json",
        "text/csv",
    }
    try:
        if mime_type in exportable:
            raw = google_api_bytes(
                account_email,
                f"https://www.googleapis.com/drive/v3/files/{file_id}/export",
                params={"mimeType": exportable[mime_type]},
            )
            return trim_body(raw.decode("utf-8", errors="ignore"))
        if mime_type in downloadable:
            raw = google_api_bytes(
                account_email,
                f"https://www.googleapis.com/drive/v3/files/{file_id}",
                params={"alt": "media"},
            )
            return trim_body(raw.decode("utf-8", errors="ignore"))
    except (SystemExit, urlerror.URLError, TimeoutError):
        return ""
    return ""


def should_fetch_drive_body(payload: dict[str, Any], remaining_budget: int) -> bool:
    if remaining_budget <= 0:
        return False
    mime_type = normalize_text(payload.get("mimeType"))
    if mime_type not in {
        "application/vnd.google-apps.document",
        "text/plain",
        "text/markdown",
        "application/json",
        "text/csv",
    }:
        return False
    title = normalize_text(payload.get("name"))
    description = normalize_text(payload.get("description"))
    domain = document_domain(title, description, "", "drive")
    if domain != "general":
        return True
    modified = parse_date(payload.get("modifiedTime"))
    if modified and modified >= now_utc() - timedelta(days=45):
        return True
    return False


def import_google_drive(
    db_path: Path,
    *,
    account_email: str | None = None,
    metadata_only: bool = False,
    body_limit: int = 250,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    account, _ = google_access_token(account_email)
    conn = open_store(db_path)
    clear_source_documents_channel(conn, "drive")
    next_page_token = ""
    imported_files = 0
    body_candidates = 0
    imported_bodies = 0
    while True:
        page = google_api_json(
            account,
            "https://www.googleapis.com/drive/v3/files",
            params={
                "pageSize": 200,
                "pageToken": next_page_token,
                "q": "trashed=false",
                "fields": "nextPageToken,files(id,name,mimeType,createdTime,modifiedTime,webViewLink,description,owners(displayName,emailAddress))",
            },
        )
        for item in page.get("files") or []:
            title = normalize_text(item.get("name")) or normalize_text(item.get("id"))
            owners = item.get("owners") or []
            author = normalize_text((owners[0] or {}).get("displayName") or (owners[0] or {}).get("emailAddress")) if owners else ""
            description = normalize_text(item.get("description"))
            body = ""
            if not metadata_only and should_fetch_drive_body(item, body_limit - imported_bodies):
                body_candidates += 1
                body = drive_file_body(account, item)
                if body:
                    imported_bodies += 1
            if not body and description:
                body = description
            upsert_source_document(
                conn,
                source_channel="drive",
                source_kind="file",
                doc_id=normalize_text(item.get("id")),
                title=title,
                body=body,
                url=normalize_text(item.get("webViewLink")),
                author=author,
                created_at=normalize_text(item.get("createdTime")),
                updated_at=normalize_text(item.get("modifiedTime")),
                metadata={
                    "mime_type": normalize_text(item.get("mimeType")),
                    "owners": owners,
                },
            )
            imported_files += 1
        conn.commit()
        next_page_token = normalize_text(page.get("nextPageToken"))
        if not next_page_token:
            break
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "account_email": account,
        "imported_files": imported_files,
        "body_candidates": body_candidates,
        "imported_bodies": imported_bodies,
        "metadata_only": metadata_only,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def roam_note_title(path: Path, text: str) -> str:
    fallback = normalize_text(path.stem.replace("_", " ").replace("-", " ")) or path.stem
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("#"):
            return normalize_text(stripped.lstrip("#").strip()) or fallback
        candidate = normalize_text(stripped.lstrip("-* ").strip())[:160]
        if not candidate:
            continue
        if stripped.startswith(("-", "*")):
            return fallback
        if len(words(candidate)) <= 2 and candidate.casefold() in {"notes", "plan", "todo", "ideas", "thoughts"}:
            return fallback
        return candidate or fallback
    return fallback


def import_roam_notes(
    db_path: Path,
    notes_dir: Path,
    *,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    conn = open_store(db_path)
    clear_source_documents_channel(conn, "roam")
    imported_notes = 0
    for path in sorted(notes_dir.rglob("*.md")):
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        title = roam_note_title(path, text)
        cleaned = markdown_to_text(text)
        upsert_source_document(
            conn,
            source_channel="roam",
            source_kind="note",
            doc_id=str(path.relative_to(notes_dir)),
            title=title,
            body=cleaned,
            url="",
            author="Sunil Rao",
            created_at=datetime.fromtimestamp(path.stat().st_ctime, tz=timezone.utc).isoformat(),
            updated_at=datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc).isoformat(),
            metadata={"path": str(path.relative_to(notes_dir))},
        )
        imported_notes += 1
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "notes_dir": str(notes_dir),
        "imported_notes": imported_notes,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def search_documents(db_path: Path, query: str, limit: int, channel: str = "all") -> list[dict[str, Any]]:
    conn = open_store(db_path)
    results: list[sqlite3.Row]
    if has_source_document_fts(conn):
        conditions = ["f.doc_key = (d.source_channel || ':' || d.source_kind || ':' || d.doc_id)", "source_documents_fts MATCH ?"]
        params: list[Any] = [fts_match_query(query) or query]
        if channel != "all":
            conditions.append("d.source_channel = ?")
            params.append(channel)
        results = conn.execute(
            f"""
            SELECT d.source_channel, d.source_kind, d.doc_id, d.title, d.url, d.author, d.updated_at, d.excerpt
            FROM source_documents d
            JOIN source_documents_fts f ON {' AND '.join(conditions)}
            ORDER BY bm25(source_documents_fts), d.updated_at DESC
            LIMIT ?
            """,
            (*params, limit),
        ).fetchall()
    else:
        like = f"%{normalize_text(query).casefold()}%"
        conditions = ["(lower(title) LIKE ? OR lower(excerpt) LIKE ? OR lower(body) LIKE ?)"]
        params = [like, like, like]
        if channel != "all":
            conditions.append("source_channel = ?")
            params.append(channel)
        results = conn.execute(
            f"""
            SELECT source_channel, source_kind, doc_id, title, url, author, updated_at, excerpt
            FROM source_documents
            WHERE {' AND '.join(conditions)}
            ORDER BY updated_at DESC
            LIMIT ?
            """,
            (*params, limit),
        ).fetchall()
    conn.close()
    return [dict(row) for row in results]


def promoted_documents(conn: sqlite3.Connection, limit: int = 18) -> list[dict[str, Any]]:
    rows = conn.execute(
        """
        SELECT source_channel, source_kind, doc_id, title, url, author, updated_at, excerpt, body, metadata_json
        FROM source_documents
        ORDER BY updated_at DESC, title ASC
        LIMIT 2000
        """
    ).fetchall()
    payload: list[dict[str, Any]] = []
    seen: set[tuple[str, str]] = set()
    for row in rows:
        title = normalize_text(row["title"])
        if not title:
            continue
        domain = document_domain(title, row["excerpt"] or row["body"] or "", row["author"] or "", row["source_channel"])
        score, reasons = document_priority_score(
            title,
            channel=row["source_channel"],
            excerpt=row["excerpt"] or row["body"] or "",
            author=row["author"] or "",
            updated_at=row["updated_at"] or "",
        )
        if score < 4.5 and domain == "general":
            continue
        key = (row["source_channel"], title.casefold())
        if key in seen:
            continue
        seen.add(key)
        payload.append(
            {
                "source_channel": row["source_channel"],
                "source_kind": row["source_kind"],
                "doc_id": row["doc_id"],
                "title": title,
                "url": row["url"] or "",
                "author": row["author"] or "",
                "updated_at": row["updated_at"] or "",
                "excerpt": row["excerpt"] or doc_excerpt(row["body"] or "", 240),
                "domain": domain,
                "score": round(score, 2),
                "reasons": reasons,
            }
        )
    payload.sort(key=lambda item: (-float(item["score"]), item["updated_at"] or "", item["title"]))
    return payload[:limit]


def reconcile_semantic_claim_statuses(conn: sqlite3.Connection) -> dict[str, int]:
    return reconcile_semantic_claim_statuses_for_channel(conn, "whatsapp")


def rebuild_whatsapp_touch_events(conn: sqlite3.Connection) -> int:
    conn.execute("DELETE FROM touch_events WHERE channel = 'whatsapp' AND source = 'whatsapp-import'")
    rows = conn.execute(
        """
        SELECT person_id, sent_at, direction, excerpt, message_type
        FROM message_events
        WHERE channel = 'whatsapp' AND person_id IS NOT NULL
        ORDER BY sent_at ASC, id ASC
        """
    ).fetchall()
    payload = []
    for row in rows:
        note = normalize_text(row["excerpt"]) or normalize_text(row["message_type"]) or "WhatsApp message"
        payload.append((row["person_id"], row["sent_at"], "whatsapp", note, row["direction"], "whatsapp-import"))
    conn.executemany(
        """
        INSERT INTO touch_events (person_id, touched_at, channel, note, direction, source)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        payload,
    )
    return len(payload)


def rebuild_whatsapp_ontology(conn: sqlite3.Connection) -> dict[str, int]:
    conn.execute("DELETE FROM person_facts WHERE channel = 'whatsapp'")
    conn.execute("DELETE FROM relationship_signals WHERE channel = 'whatsapp'")
    conn.execute("DELETE FROM semantic_claims WHERE channel = 'whatsapp'")
    conn.execute("DELETE FROM relationship_edges WHERE channel = 'whatsapp'")
    conn.execute("DELETE FROM entity_aliases WHERE source_channel = 'whatsapp'")
    conn.execute("DELETE FROM entities WHERE source_channel = 'whatsapp'")

    topic_counters: dict[str, Counter[str]] = defaultdict(Counter)
    signal_rows: dict[str, dict[str, Any]] = {}
    group_memberships: set[tuple[str, str]] = set()
    alias_seen: set[tuple[str, str]] = set()
    phone_seen: set[tuple[str, str]] = set()

    def signal_for(person_id: str) -> dict[str, Any]:
        existing = signal_rows.get(person_id)
        if existing:
            return existing
        payload = {
            "person_id": person_id,
            "direct_inbound_count": 0,
            "direct_outbound_count": 0,
            "group_inbound_count": 0,
            "group_outbound_count": 0,
            "first_seen_at": None,
            "last_seen_at": None,
            "last_direct_at": None,
            "last_group_at": None,
            "recent_excerpt": "",
        }
        signal_rows[person_id] = payload
        return payload

    def usable_excerpt(text: str) -> str:
        clean = normalize_text(text)
        lowered = clean.casefold()
        if not clean:
            return ""
        if lowered in {"media", "text", "image", "video", "audio", "document", "sticker"}:
            return ""
        if len(clean) < 8:
            return ""
        return clean

    people_rows = conn.execute("SELECT * FROM people").fetchall()
    for row in people_rows:
        person_id = row["person_id"]
        record = record_for_row(row)
        if record.relationship_label:
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="whatsapp",
                claim_type="relationship",
                predicate="relationship_label",
                object_value=record.relationship_label,
                claim_status="accepted",
                confidence=0.99,
                source_kind="person",
                source_ref=person_id,
                observed_at=row["last_touch_date"],
            )
            insert_relationship_edge(
                conn,
                subject_type="owner",
                subject_ref=OWNER_ENTITY_REF,
                predicate=record.relationship_label,
                object_type="person",
                object_ref=person_id,
                channel="whatsapp",
                confidence=0.99,
                source_kind="person",
                source_ref=person_id,
                observed_at=row["last_touch_date"],
            )
        if record.category:
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="whatsapp",
                claim_type="profile",
                predicate="category",
                object_value=record.category,
                claim_status="accepted",
                confidence=0.95,
                source_kind="person",
                source_ref=person_id,
                observed_at=row["last_touch_date"],
            )
        if record.preferred_channel and record.preferred_channel != "unknown":
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="whatsapp",
                claim_type="preference",
                predicate="preferred_channel",
                object_value=record.preferred_channel,
                claim_status="accepted",
                confidence=0.78,
                source_kind="person",
                source_ref=person_id,
                observed_at=row["last_touch_date"],
            )
        for alias in json.loads(row["aliases_json"]):
            clean_alias = normalize_text(alias)
            if clean_alias:
                key = (person_id, clean_alias.casefold())
                if key not in alias_seen:
                    insert_person_fact(
                        conn,
                        person_id=person_id,
                        channel="whatsapp",
                        fact_type="alias",
                        fact_value=clean_alias,
                        normalized_value=clean_alias,
                        confidence=0.85,
                        source_kind="person",
                        source_ref=person_id,
                        observed_at=row["last_touch_date"],
                    )
                    alias_seen.add(key)
        for phone in json.loads(row["phones_json"]):
            clean_phone = normalize_phone(phone)
            if clean_phone:
                key = (person_id, clean_phone)
                if key not in phone_seen:
                    insert_person_fact(
                        conn,
                        person_id=person_id,
                        channel="whatsapp",
                        fact_type="phone",
                        fact_value=clean_phone,
                        normalized_value=clean_phone,
                        confidence=1.0,
                        source_kind="person",
                        source_ref=person_id,
                        observed_at=row["last_touch_date"],
                    )
                    phone_seen.add(key)
        for email in json.loads(row["emails_json"]):
            clean_email = normalize_text(email).lower()
            if clean_email:
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    claim_type="identity",
                    predicate="has_email",
                    object_value=clean_email,
                    claim_status="accepted",
                    confidence=0.95,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                )
        for org in json.loads(row["organizations_json"]):
            clean_org = normalize_text(org)
            if clean_org:
                entity_id = upsert_entity(
                    conn,
                    entity_type="organization",
                    canonical_name=clean_org,
                    source_channel="whatsapp",
                    confidence=0.76,
                    metadata={"source": "people.organizations"},
                )
                add_entity_alias(
                    conn,
                    entity_id=entity_id,
                    source_channel="whatsapp",
                    alias_type="name",
                    alias_value=clean_org,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                )
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    claim_type="affiliation",
                    predicate="affiliated_with",
                    object_value=clean_org,
                    claim_status="accepted",
                    confidence=0.76,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                )
                insert_relationship_edge(
                    conn,
                    subject_type="person",
                    subject_ref=person_id,
                    predicate="affiliated_with",
                    object_type="organization",
                    object_ref=entity_id,
                    channel="whatsapp",
                    confidence=0.76,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                    raw={"organization": clean_org},
                )
        for role in json.loads(row["roles_json"]):
            clean_role = normalize_text(role)
            if clean_role:
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    claim_type="affiliation",
                    predicate="has_role",
                    object_value=clean_role,
                    claim_status="accepted",
                    confidence=0.7,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                )
        for action in json.loads(row["open_actions_json"]):
            description = normalize_text(action.get("description"))
            if description:
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    claim_type="commitment",
                    predicate="open_loop",
                    object_value=description,
                    claim_status="accepted",
                    confidence=0.72,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                    raw=action,
                )
        for date_info in json.loads(row["important_dates_json"]):
            date_value = normalize_text(date_info.get("date"))
            if date_value:
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    claim_type="date_marker",
                    predicate=normalize_text(date_info.get("date_type")) or "important_date",
                    object_value=date_value,
                    claim_status="accepted",
                    confidence=0.74,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=date_value,
                    raw=date_info,
                )
        for topic in json.loads(row["topics_json"]):
            clean_topic = normalize_text(topic)
            if clean_topic:
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    claim_type="topic",
                    predicate="mentioned_topic",
                    object_value=clean_topic,
                    claim_status="candidate",
                    confidence=0.58,
                    source_kind="person",
                    source_ref=person_id,
                    observed_at=row["last_touch_date"],
                )

    for row in conn.execute(
        """
        SELECT channel, account_id, contact_id, display_name, short_name, phone, raw_json, updated_at
        FROM channel_contacts
        WHERE channel = 'whatsapp'
        """
    ):
        clean_name = choose_display_name(normalize_text(row["display_name"]), normalize_text(row["short_name"]))
        clean_phone = normalize_phone(row["phone"])
        person = match_person_row(conn, clean_phone, clean_name)
        if not person:
            continue
        person_id = person["person_id"]
        if clean_name:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel="whatsapp",
                fact_type="contact_name",
                fact_value=clean_name,
                normalized_value=clean_name,
                confidence=0.9,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
            )
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="whatsapp",
                claim_type="identity",
                predicate="known_as",
                object_value=clean_name,
                claim_status="accepted",
                confidence=0.9,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
            )
        if clean_phone:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel="whatsapp",
                fact_type="channel_phone",
                fact_value=clean_phone,
                normalized_value=clean_phone,
                confidence=1.0,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
            )
        try:
            raw = json.loads(row["raw_json"])
        except json.JSONDecodeError:
            raw = {}
        about = normalize_text(raw.get("about"))
        if about:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel="whatsapp",
                fact_type="about",
                fact_value=about,
                normalized_value=about,
                confidence=0.7,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
                raw={"about": about},
            )
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="whatsapp",
                claim_type="profile",
                predicate="about_status",
                object_value=about,
                claim_status="observed",
                confidence=0.7,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
                raw={"about": about},
            )

    message_rows = conn.execute(
        """
        SELECT person_id, message_id, chat_id, chat_name, chat_phone, is_group, direction, sent_at, excerpt, text
        FROM message_events
        WHERE channel = 'whatsapp' AND person_id IS NOT NULL
        ORDER BY sent_at ASC, id ASC
        """
    ).fetchall()
    for row in message_rows:
        person_id = row["person_id"]
        signal = signal_for(person_id)
        sent_at = row["sent_at"]
        signal["first_seen_at"] = sent_at if not signal["first_seen_at"] else min(signal["first_seen_at"], sent_at)
        signal["last_seen_at"] = sent_at if not signal["last_seen_at"] else max(signal["last_seen_at"], sent_at)
        excerpt = usable_excerpt(row["excerpt"] or row["text"])
        if excerpt:
            signal["recent_excerpt"] = excerpt
        if row["is_group"]:
            if row["direction"] == "inbound":
                signal["group_inbound_count"] += 1
            else:
                signal["group_outbound_count"] += 1
            signal["last_group_at"] = sent_at if not signal["last_group_at"] else max(signal["last_group_at"], sent_at)
            group_name = normalize_text(row["chat_name"])
            group_ref = normalize_text(row["chat_id"])
            if group_name and (person_id, group_ref) not in group_memberships:
                entity_id = upsert_entity(
                    conn,
                    entity_type="group_chat",
                    canonical_name=group_name,
                    source_channel="whatsapp",
                    confidence=0.75,
                    metadata={"chat_id": group_ref},
                )
                add_entity_alias(
                    conn,
                    entity_id=entity_id,
                    source_channel="whatsapp",
                    alias_type="chat_id",
                    alias_value=group_ref or group_name,
                    source_kind="chat",
                    source_ref=group_ref or group_name,
                    observed_at=sent_at,
                )
                insert_person_fact(
                    conn,
                    person_id=person_id,
                    channel="whatsapp",
                    fact_type="group_membership",
                    fact_value=group_name,
                    normalized_value=group_ref or group_name,
                    confidence=0.75,
                    source_kind="chat",
                    source_ref=group_ref or group_name,
                    observed_at=sent_at,
                )
                group_memberships.add((person_id, group_ref))
                insert_relationship_edge(
                    conn,
                    subject_type="person",
                    subject_ref=person_id,
                    predicate="participates_in",
                    object_type="group_chat",
                    object_ref=entity_id,
                    channel="whatsapp",
                    confidence=0.75,
                    source_kind="chat",
                    source_ref=group_ref or group_name,
                    observed_at=sent_at,
                    raw={"group_name": group_name},
                )
        else:
            if row["direction"] == "inbound":
                signal["direct_inbound_count"] += 1
            else:
                signal["direct_outbound_count"] += 1
            signal["last_direct_at"] = sent_at if not signal["last_direct_at"] else max(signal["last_direct_at"], sent_at)
        for term in message_topic_terms(row["text"] or row["excerpt"]):
            topic_counters[person_id][term] += 1
        for claim in extract_message_semantic_claims(row["text"] or row["excerpt"]):
            claim_status = "observed" if claim["confidence"] >= 0.75 else "candidate"
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel="whatsapp",
                claim_type=claim["claim_type"],
                predicate=claim["predicate"],
                object_value=claim["object_value"],
                claim_status=claim_status,
                confidence=claim["confidence"],
                source_kind="message",
                source_ref=row["message_id"],
                source_message_id=row["message_id"],
                source_chat_id=row["chat_id"],
                observed_at=sent_at,
                raw=claim.get("raw"),
            )
            if claim["predicate"] == "travel_location":
                entity_id = upsert_entity(
                    conn,
                    entity_type="place",
                    canonical_name=claim["object_value"],
                    source_channel="whatsapp",
                    confidence=claim["confidence"],
                    metadata={"source_message_id": row["message_id"]},
                )
                add_entity_alias(
                    conn,
                    entity_id=entity_id,
                    source_channel="whatsapp",
                    alias_type="name",
                    alias_value=claim["object_value"],
                    source_kind="message",
                    source_ref=row["message_id"],
                    observed_at=sent_at,
                )
                insert_relationship_edge(
                    conn,
                    subject_type="person",
                    subject_ref=person_id,
                    predicate="travel_location",
                    object_type="place",
                    object_ref=entity_id,
                    channel="whatsapp",
                    confidence=claim["confidence"],
                    source_kind="message",
                    source_ref=row["message_id"],
                    observed_at=sent_at,
                    raw=claim.get("raw"),
                )

    updated_people = 0
    for person_id, signal in signal_rows.items():
        topics = [token for token, count in topic_counters[person_id].most_common(10) if count >= 3][:6]
        conn.execute(
            """
            INSERT OR REPLACE INTO relationship_signals (
                person_id, channel, direct_inbound_count, direct_outbound_count,
                group_inbound_count, group_outbound_count, first_seen_at, last_seen_at,
                last_direct_at, last_group_at, recent_excerpt, top_topics_json, raw_json, updated_at
            ) VALUES (?, 'whatsapp', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                person_id,
                signal["direct_inbound_count"],
                signal["direct_outbound_count"],
                signal["group_inbound_count"],
                signal["group_outbound_count"],
                signal["first_seen_at"],
                signal["last_seen_at"],
                signal["last_direct_at"],
                signal["last_group_at"],
                signal["recent_excerpt"],
                safe_json(topics),
                safe_json(signal),
                now_utc().isoformat(),
            ),
        )
        row = conn.execute("SELECT * FROM people WHERE person_id = ?", (person_id,)).fetchone()
        if not row:
            continue
        note_parts = [
            f"WhatsApp history: {signal['direct_inbound_count']} inbound direct, {signal['direct_outbound_count']} outbound direct, "
            f"{signal['group_inbound_count']} inbound group, {signal['group_outbound_count']} outbound group."
        ]
        if signal["last_seen_at"]:
            note_parts.append(f"Last WhatsApp touch {signal['last_seen_at']}.")
        if signal["recent_excerpt"]:
            note_parts.append(f"Recent excerpt: {signal['recent_excerpt']}")
        existing_notes = clean_profile_notes(json.loads(row["notes_json"]), keep_whatsapp_history=False)
        notes_json = safe_json(clean_profile_notes([*existing_notes, " ".join(note_parts)], limit=24))
        existing_topics = clean_profile_topics(json.loads(row["topics_json"]), limit=20)
        topics_json = safe_json(clean_profile_topics([*existing_topics, *topics], limit=20))
        preferred = row["preferred_channel"]
        direct_total = signal["direct_inbound_count"] + signal["direct_outbound_count"]
        if direct_total >= 3:
            preferred = "whatsapp"
        last_touch_date = best_iso_date(latest_touch(conn, row), signal["last_seen_at"])
        conn.execute(
            """
            UPDATE people
            SET notes_json = ?, topics_json = ?, preferred_channel = ?, last_touch_date = ?
            WHERE person_id = ?
            """,
            (notes_json, topics_json, preferred, last_touch_date, person_id),
        )
        updated_row = conn.execute("SELECT * FROM people WHERE person_id = ?", (person_id,)).fetchone()
        if updated_row:
            updated_record = record_for_row(updated_row)
            importance = compute_importance(updated_record)
            conn.execute("UPDATE people SET importance = ? WHERE person_id = ?", (importance, person_id))
            refresh_people_fts(conn, person_id)
            updated_people += 1

    claim_status_updates = reconcile_semantic_claim_statuses(conn)

    return {
        "person_facts": conn.execute("SELECT COUNT(*) FROM person_facts WHERE channel = 'whatsapp'").fetchone()[0],
        "relationship_signals": conn.execute("SELECT COUNT(*) FROM relationship_signals WHERE channel = 'whatsapp'").fetchone()[0],
        "semantic_claims": conn.execute("SELECT COUNT(*) FROM semantic_claims WHERE channel = 'whatsapp'").fetchone()[0],
        "relationship_edges": conn.execute("SELECT COUNT(*) FROM relationship_edges WHERE channel = 'whatsapp'").fetchone()[0],
        "entities": conn.execute("SELECT COUNT(*) FROM entities WHERE source_channel = 'whatsapp'").fetchone()[0],
        "entity_aliases": conn.execute("SELECT COUNT(*) FROM entity_aliases WHERE source_channel = 'whatsapp'").fetchone()[0],
        "updated_people": updated_people,
        **claim_status_updates,
    }


def recalculate_people(conn: sqlite3.Connection, person_ids: Iterable[str]) -> int:
    updated = 0
    for person_id in sorted({item for item in person_ids if item}):
        row = conn.execute("SELECT * FROM people WHERE person_id = ?", (person_id,)).fetchone()
        if not row:
            continue
        record = record_for_row(row)
        latest = latest_touch(conn, row)
        record.last_touch_date = latest
        importance = compute_importance(record)
        conn.execute(
            "UPDATE people SET last_touch_date = ?, importance = ?, built_at = ? WHERE person_id = ?",
            (latest, importance, now_utc().isoformat(), person_id),
        )
        refresh_people_fts(conn, person_id)
        updated += 1
    return updated


def reconcile_semantic_claim_statuses_for_channel(conn: sqlite3.Connection, channel: str) -> dict[str, int]:
    promoted = 0
    stale_marked = 0

    promotable = "', '".join(sorted(PROMOTABLE_CANDIDATE_PREDICATES))
    rows = conn.execute(
        f"""
        SELECT person_id, predicate, normalized_value, COUNT(*) AS occurrences, MAX(confidence) AS max_confidence
        FROM semantic_claims
        WHERE channel = ?
          AND claim_status = 'candidate'
          AND predicate IN ('{promotable}')
          AND normalized_value <> ''
        GROUP BY person_id, predicate, normalized_value
        HAVING (predicate = 'mentioned_topic' AND COUNT(*) >= 3)
            OR (predicate <> 'mentioned_topic' AND COUNT(*) >= 2 AND MAX(confidence) >= 0.66)
        """,
        (channel,),
    ).fetchall()
    for row in rows:
        cursor = conn.execute(
            """
            UPDATE semantic_claims
            SET claim_status = 'observed'
            WHERE channel = ?
              AND claim_status = 'candidate'
              AND person_id = ?
              AND predicate = ?
              AND normalized_value = ?
            """,
            (channel, row["person_id"], row["predicate"], row["normalized_value"]),
        )
        promoted += cursor.rowcount

    for predicate in sorted(TEMPORAL_PREDICATES):
        rows = conn.execute(
            """
            SELECT id, person_id
            FROM semantic_claims
            WHERE channel = ?
              AND predicate = ?
              AND claim_status IN ('accepted', 'observed', 'candidate')
            ORDER BY person_id ASC, observed_at DESC, confidence DESC, id DESC
            """,
            (channel, predicate),
        ).fetchall()
        latest_by_person: set[str] = set()
        for row in rows:
            if row["person_id"] not in latest_by_person:
                latest_by_person.add(row["person_id"])
                continue
            cursor = conn.execute(
                """
                UPDATE semantic_claims
                SET claim_status = 'stale'
                WHERE id = ?
                  AND claim_status <> 'stale'
                """,
                (row["id"],),
            )
            stale_marked += cursor.rowcount
    return {"promoted_claims": promoted, "stale_claims": stale_marked}


def rebuild_message_channel_ontology(conn: sqlite3.Connection, channel: str) -> dict[str, int]:
    conn.execute("DELETE FROM person_facts WHERE channel = ?", (channel,))
    conn.execute("DELETE FROM relationship_signals WHERE channel = ?", (channel,))
    conn.execute("DELETE FROM semantic_claims WHERE channel = ?", (channel,))
    conn.execute("DELETE FROM relationship_edges WHERE channel = ?", (channel,))

    signal_rows: dict[str, dict[str, Any]] = {}
    topic_counters: dict[str, Counter[str]] = defaultdict(Counter)
    affected_people: set[str] = set()

    def signal_for(person_id: str) -> dict[str, Any]:
        payload = signal_rows.get(person_id)
        if payload:
            return payload
        payload = {
            "person_id": person_id,
            "direct_inbound_count": 0,
            "direct_outbound_count": 0,
            "group_inbound_count": 0,
            "group_outbound_count": 0,
            "first_seen_at": None,
            "last_seen_at": None,
            "last_direct_at": None,
            "last_group_at": None,
            "recent_excerpt": "",
        }
        signal_rows[person_id] = payload
        return payload

    for row in conn.execute(
        """
        SELECT channel, account_id, contact_id, display_name, short_name, phone, email, raw_json, updated_at
        FROM channel_contacts
        WHERE channel = ?
        """,
        (channel,),
    ).fetchall():
        clean_name = choose_display_name(normalize_text(row["display_name"]), normalize_text(row["short_name"]))
        clean_phone = normalize_phone(row["phone"])
        clean_email = normalize_email(row["email"])
        person = match_person_row(conn, clean_phone, clean_name, clean_email)
        if not person:
            continue
        person_id = person["person_id"]
        affected_people.add(person_id)
        raw_payload = safe_json_loads(row["raw_json"], {})
        if clean_name:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel=channel,
                fact_type="contact_name",
                fact_value=clean_name,
                normalized_value=clean_name,
                confidence=0.9,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
                raw=raw_payload,
            )
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel=channel,
                claim_type="identity",
                predicate="known_as",
                object_value=clean_name,
                claim_status="observed",
                confidence=0.9,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
            )
        if clean_phone:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel=channel,
                fact_type="channel_phone",
                fact_value=clean_phone,
                normalized_value=clean_phone,
                confidence=1.0,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
                raw=raw_payload,
            )
        if clean_email:
            insert_person_fact(
                conn,
                person_id=person_id,
                channel=channel,
                fact_type="channel_email",
                fact_value=clean_email,
                normalized_value=clean_email,
                confidence=1.0,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
                raw=raw_payload,
            )
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel=channel,
                claim_type="identity",
                predicate="has_email",
                object_value=clean_email,
                claim_status="observed",
                confidence=0.92,
                source_kind="contact",
                source_ref=row["contact_id"],
                observed_at=row["updated_at"],
            )
            guessed_org = guess_org_from_email(clean_email)
            if guessed_org:
                insert_semantic_claim(
                    conn,
                    person_id=person_id,
                    channel=channel,
                    claim_type="affiliation",
                    predicate="affiliated_with",
                    object_value=guessed_org,
                    claim_status="candidate",
                    confidence=0.58,
                    source_kind="contact",
                    source_ref=row["contact_id"],
                    observed_at=row["updated_at"],
                )

    message_rows = conn.execute(
        """
        SELECT person_id, message_id, chat_id, chat_name, is_group, direction, sent_at, excerpt, text
        FROM message_events
        WHERE channel = ? AND person_id IS NOT NULL
        ORDER BY sent_at ASC, id ASC
        """,
        (channel,),
    ).fetchall()
    for row in message_rows:
        person_id = row["person_id"]
        affected_people.add(person_id)
        signal = signal_for(person_id)
        sent_at = row["sent_at"]
        signal["first_seen_at"] = sent_at if not signal["first_seen_at"] else min(signal["first_seen_at"], sent_at)
        signal["last_seen_at"] = sent_at if not signal["last_seen_at"] else max(signal["last_seen_at"], sent_at)
        excerpt = normalize_text(row["excerpt"] or row["text"])
        if excerpt:
            signal["recent_excerpt"] = excerpt
        if row["is_group"]:
            if row["direction"] == "outbound":
                signal["group_outbound_count"] += 1
            else:
                signal["group_inbound_count"] += 1
            signal["last_group_at"] = sent_at if not signal["last_group_at"] else max(signal["last_group_at"], sent_at)
        else:
            if row["direction"] == "outbound":
                signal["direct_outbound_count"] += 1
            else:
                signal["direct_inbound_count"] += 1
            signal["last_direct_at"] = sent_at if not signal["last_direct_at"] else max(signal["last_direct_at"], sent_at)
        for term in message_topic_terms(row["text"] or row["excerpt"]):
            topic_counters[person_id][term] += 1
        for claim in extract_message_semantic_claims(row["text"] or row["excerpt"]):
            insert_semantic_claim(
                conn,
                person_id=person_id,
                channel=channel,
                claim_type=claim["claim_type"],
                predicate=claim["predicate"],
                object_value=claim["object_value"],
                claim_status="candidate",
                confidence=claim["confidence"],
                source_kind="message",
                source_ref=row["message_id"],
                source_message_id=row["message_id"],
                source_chat_id=row["chat_id"],
                observed_at=sent_at,
                raw=claim.get("raw"),
            )

    for person_id, signal in signal_rows.items():
        topics = [token for token, count in topic_counters[person_id].most_common(10) if count >= 2][:6]
        conn.execute(
            """
            INSERT OR REPLACE INTO relationship_signals (
                person_id, channel, direct_inbound_count, direct_outbound_count,
                group_inbound_count, group_outbound_count, first_seen_at, last_seen_at,
                last_direct_at, last_group_at, recent_excerpt, top_topics_json, raw_json, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                person_id,
                channel,
                signal["direct_inbound_count"],
                signal["direct_outbound_count"],
                signal["group_inbound_count"],
                signal["group_outbound_count"],
                signal["first_seen_at"],
                signal["last_seen_at"],
                signal["last_direct_at"],
                signal["last_group_at"],
                signal["recent_excerpt"],
                safe_json(topics),
                safe_json(signal),
                now_utc().isoformat(),
            ),
        )
    claim_status_updates = reconcile_semantic_claim_statuses_for_channel(conn, channel)
    updated_people = recalculate_people(conn, affected_people)
    return {
        "person_facts": conn.execute("SELECT COUNT(*) FROM person_facts WHERE channel = ?", (channel,)).fetchone()[0],
        "relationship_signals": conn.execute("SELECT COUNT(*) FROM relationship_signals WHERE channel = ?", (channel,)).fetchone()[0],
        "semantic_claims": conn.execute("SELECT COUNT(*) FROM semantic_claims WHERE channel = ?", (channel,)).fetchone()[0],
        "updated_people": updated_people,
        **claim_status_updates,
    }


def reconcile_whatsapp_graph(
    db_path: Path,
    *,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    conn = open_store(db_path)
    merged_people = 0
    phone_map: dict[str, list[sqlite3.Row]] = defaultdict(list)
    for row in conn.execute("SELECT * FROM people").fetchall():
        for phone in json.loads(row["phones_json"]):
            clean_phone = normalize_phone(phone)
            if clean_phone:
                phone_map[clean_phone].append(row)

    for rows in phone_map.values():
        if len(rows) < 2:
            continue
        ordered = sorted(rows, key=merge_people_priority, reverse=True)
        target = ordered[0]["person_id"]
        for source in ordered[1:]:
            if merge_people_rows(conn, target, source["person_id"]):
                merged_people += 1

    normalized_special_contacts = enforce_special_contacts(conn)

    person_cache: dict[tuple[str, str], str | None] = {}
    identity_cache: dict[str, tuple[str, str]] = {}
    message_updates = []
    relinked_messages = 0

    def resolve_person_id(name: str, phone: str) -> str | None:
        key = (normalize_text(name).casefold(), normalize_phone(phone))
        if key in person_cache:
            return person_cache[key]
        person_id = ensure_person_for_counterparty(conn, display_name=name, phone=phone, channel="whatsapp")
        person_cache[key] = person_id
        return person_id

    def person_identity(person_id: str | None, fallback_name: str, fallback_phone: str) -> tuple[str, str]:
        if not person_id:
            return normalize_text(fallback_name), normalize_phone(fallback_phone)
        cached = identity_cache.get(person_id)
        if cached:
            return cached
        row = conn.execute("SELECT display_name, phones_json FROM people WHERE person_id = ?", (person_id,)).fetchone()
        if not row:
            value = (normalize_text(fallback_name), normalize_phone(fallback_phone))
            identity_cache[person_id] = value
            return value
        phones = [normalize_phone(item) for item in json.loads(row["phones_json"]) if normalize_phone(item)]
        value = (normalize_text(row["display_name"]) or normalize_text(fallback_name), phones[0] if phones else normalize_phone(fallback_phone))
        identity_cache[person_id] = value
        return value

    rows = conn.execute(
        """
        SELECT id, person_id, chat_id, chat_name, chat_phone, is_group, direction,
               sender_id, sender_name, sender_phone, counterpart_name, counterpart_phone
        FROM message_events
        WHERE channel = 'whatsapp'
        ORDER BY id ASC
        """
    ).fetchall()
    for row in rows:
        if row["is_group"]:
            if row["direction"] == "inbound":
                target_phone = normalize_phone(row["sender_phone"]) or phone_from_chat_id(row["sender_id"])
                target_name = normalize_text(row["sender_name"])
                target_person_id = resolve_person_id(target_name, target_phone) if (target_name or target_phone) else None
                display_name, display_phone = person_identity(target_person_id, target_name, target_phone)
                new_values = (
                    row["sender_id"] or row["chat_id"],
                    display_name,
                    display_phone,
                    row["counterpart_name"],
                    row["counterpart_phone"],
                    target_person_id,
                    row["id"],
                )
            else:
                target_person_id = None
                new_values = (
                    row["sender_id"],
                    row["sender_name"],
                    row["sender_phone"],
                    row["counterpart_name"],
                    row["counterpart_phone"],
                    None,
                    row["id"],
                )
        else:
            target_phone = normalize_phone(row["chat_phone"]) or phone_from_chat_id(row["chat_id"])
            target_name = choose_display_name(normalize_text(row["chat_name"]), normalize_text(row["counterpart_name"] or row["sender_name"]))
            target_person_id = resolve_person_id(target_name, target_phone) if (target_name or target_phone) else None
            display_name, display_phone = person_identity(target_person_id, target_name, target_phone)
            if row["direction"] == "inbound":
                new_values = (
                    row["chat_id"],
                    display_name,
                    display_phone,
                    display_name,
                    display_phone,
                    target_person_id,
                    row["id"],
                )
            else:
                new_values = (
                    "",
                    "",
                    "",
                    display_name,
                    display_phone,
                    target_person_id,
                    row["id"],
                )
        message_updates.append(new_values)
        if row["person_id"] != new_values[5]:
            relinked_messages += 1

    conn.executemany(
        """
        UPDATE message_events
        SET sender_id = ?, sender_name = ?, sender_phone = ?, counterpart_name = ?,
            counterpart_phone = ?, person_id = ?
        WHERE id = ?
        """,
        message_updates,
    )

    touch_count = rebuild_whatsapp_touch_events(conn)
    ontology = rebuild_whatsapp_ontology(conn)
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "merged_people": merged_people,
        "normalized_special_contacts": normalized_special_contacts,
        "relinked_messages": relinked_messages,
        "rebuilt_touch_events": touch_count,
        "ontology": ontology,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def reconcile_identities(
    db_path: Path,
    *,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    conn = open_store(db_path)
    merged_by_phone = 0
    merged_by_email = 0

    def merge_groups(groups: dict[str, list[sqlite3.Row]]) -> int:
        merged = 0
        for rows in groups.values():
            if len(rows) < 2:
                continue
            ordered = sorted(rows, key=merge_people_priority, reverse=True)
            target = ordered[0]["person_id"]
            for source in ordered[1:]:
                if merge_people_rows(conn, target, source["person_id"]):
                    merged += 1
        return merged

    rows = conn.execute("SELECT * FROM people").fetchall()
    phone_groups: dict[str, list[sqlite3.Row]] = defaultdict(list)
    email_groups: dict[str, list[sqlite3.Row]] = defaultdict(list)
    for row in rows:
        for phone in json.loads(row["phones_json"]):
            clean_phone = normalize_phone(phone)
            if clean_phone:
                phone_groups[clean_phone].append(row)
        for email in json.loads(row["emails_json"]):
            clean_email = normalize_email(email)
            if clean_email:
                email_groups[clean_email].append(row)

    merged_by_phone = merge_groups(phone_groups)
    merged_by_email = merge_groups(email_groups)
    normalized_special_contacts = enforce_special_contacts(conn)
    updated_people = recalculate_people(conn, [row["person_id"] for row in conn.execute("SELECT person_id FROM people")])
    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "merged_by_phone": merged_by_phone,
        "merged_by_email": merged_by_email,
        "normalized_special_contacts": normalized_special_contacts,
        "updated_people": updated_people,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def maybe_insert_touch_event(
    conn: sqlite3.Connection,
    *,
    person_id: str,
    touched_at: str,
    channel: str,
    note: str,
    direction: str,
    source: str,
) -> None:
    excerpt = trim_message_excerpt(note, 180)
    if not excerpt:
        return
    existing = conn.execute(
        """
        SELECT 1
        FROM touch_events
        WHERE person_id = ? AND touched_at = ? AND channel = ? AND note = ? AND direction = ? AND source = ?
        LIMIT 1
        """,
        (person_id, touched_at, channel, excerpt, direction, source),
    ).fetchone()
    if existing:
        return
    conn.execute(
        """
        INSERT INTO touch_events (person_id, touched_at, channel, note, direction, source)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        (person_id, touched_at, channel, excerpt, direction, source),
    )


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


def summarize_row_with_conn(conn: sqlite3.Connection, row: sqlite3.Row) -> dict[str, Any]:
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


def summarize_row(db_path: Path, row: sqlite3.Row) -> dict[str, Any]:
    conn = open_store(db_path)
    payload = summarize_row_with_conn(conn, row)
    conn.close()
    return payload


def semantic_claims_for_person(db_path: Path, query: str, limit: int = 50, channel: str = "all") -> dict[str, Any] | None:
    row = resolve_person(db_path, query)
    if not row:
        return None
    conn = open_store(db_path)
    conditions = ["person_id = ?"]
    params: list[Any] = [row["person_id"]]
    if channel != "all":
        conditions.append("channel = ?")
        params.append(channel)
    claims = conn.execute(
        f"""
        SELECT claim_type, predicate, object_value, claim_status, confidence, source_kind, source_ref,
               source_message_id, source_chat_id, observed_at, channel
        FROM semantic_claims
        WHERE {' AND '.join(conditions)}
        ORDER BY observed_at DESC, confidence DESC, channel ASC
        LIMIT ?
        """,
        (*params, limit),
    ).fetchall()
    edge_conditions = ["(subject_type = 'person' AND subject_ref = ?)", "(object_type = 'person' AND object_ref = ?)"]
    edge_params: list[Any] = [row["person_id"], row["person_id"]]
    channel_condition = ""
    if channel != "all":
        channel_condition = "AND channel = ?"
        edge_params.append(channel)
    edges = conn.execute(
        f"""
        SELECT subject_type, subject_ref, predicate, object_type, object_ref, confidence, observed_at
        FROM relationship_edges
        WHERE ({' OR '.join(edge_conditions)})
          {channel_condition}
        ORDER BY observed_at DESC, confidence DESC, channel ASC
        LIMIT ?
        """,
        (*edge_params, limit),
    ).fetchall()
    conn.close()
    return {
        "person_id": row["person_id"],
        "display_name": row["display_name"],
        "claims": [dict(item) for item in claims],
        "edges": [dict(item) for item in edges],
    }


def search_entities(db_path: Path, query: str, limit: int = 25) -> list[dict[str, Any]]:
    conn = open_store(db_path)
    like = f"%{normalize_text(query).casefold()}%"
    rows = conn.execute(
        """
        SELECT e.entity_id, e.entity_type, e.canonical_name, e.confidence, e.source_channel,
               group_concat(a.alias_value, ' | ') AS aliases
        FROM entities e
        LEFT JOIN entity_aliases a ON a.entity_id = e.entity_id
        WHERE lower(e.canonical_name) LIKE ?
           OR lower(e.normalized_name) LIKE ?
           OR lower(COALESCE(a.alias_value, '')) LIKE ?
        GROUP BY e.entity_id, e.entity_type, e.canonical_name, e.confidence, e.source_channel
        ORDER BY e.confidence DESC, e.canonical_name ASC
        LIMIT ?
        """,
        (like, like, like, limit),
    ).fetchall()
    conn.close()
    return [dict(row) for row in rows]


def person_timeline(
    db_path: Path,
    query: str,
    *,
    limit: int = 20,
    channel: str = "whatsapp",
) -> dict[str, Any] | None:
    row = resolve_person(db_path, query)
    if not row:
        return None
    conn = open_store(db_path)
    messages = conn.execute(
        """
        SELECT sent_at, direction, chat_name, chat_id, is_group, message_type, excerpt, text
        FROM message_events
        WHERE person_id = ?
          AND channel = ?
        ORDER BY sent_at DESC, id DESC
        LIMIT ?
        """,
        (row["person_id"], channel, limit),
    ).fetchall()
    touches = conn.execute(
        """
        SELECT touched_at, channel, direction, note, source
        FROM touch_events
        WHERE person_id = ?
        ORDER BY touched_at DESC, id DESC
        LIMIT ?
        """,
        (row["person_id"], limit),
    ).fetchall()
    claims = conn.execute(
        """
        SELECT claim_type, predicate, object_value, claim_status, confidence, observed_at, source_kind, source_ref
        FROM semantic_claims
        WHERE person_id = ?
          AND channel = ?
        ORDER BY observed_at DESC, confidence DESC, id DESC
        LIMIT ?
        """,
        (row["person_id"], channel, limit),
    ).fetchall()
    conn.close()
    return {
        "person_id": row["person_id"],
        "display_name": row["display_name"],
        "channel": channel,
        "recent_messages": [dict(item) for item in messages],
        "recent_touches": [dict(item) for item in touches],
        "recent_claims": [dict(item) for item in claims],
    }


def person_network(db_path: Path, query: str, *, limit: int = 25) -> dict[str, Any] | None:
    row = resolve_person(db_path, query)
    if not row:
        return None
    conn = open_store(db_path)
    outgoing = conn.execute(
        """
        SELECT predicate, object_type, object_ref, confidence, observed_at
        FROM relationship_edges
        WHERE subject_type = 'person'
          AND subject_ref = ?
        ORDER BY confidence DESC, observed_at DESC
        LIMIT ?
        """,
        (row["person_id"], limit),
    ).fetchall()
    incoming = conn.execute(
        """
        SELECT subject_type, subject_ref, predicate, confidence, observed_at
        FROM relationship_edges
        WHERE object_type = 'person'
          AND object_ref = ?
        ORDER BY confidence DESC, observed_at DESC
        LIMIT ?
        """,
        (row["person_id"], limit),
    ).fetchall()
    facts = conn.execute(
        """
        SELECT fact_type, fact_value, confidence, observed_at, channel
        FROM person_facts
        WHERE person_id = ?
        ORDER BY confidence DESC, observed_at DESC
        LIMIT ?
        """,
        (row["person_id"], limit * 2),
    ).fetchall()
    fact_groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for fact in facts:
        fact_groups[fact["fact_type"]].append(dict(fact))
    outgoing_payload = []
    for edge in outgoing:
        payload = dict(edge)
        payload["object_name"] = resolve_ref_name(conn, edge["object_type"], edge["object_ref"])
        outgoing_payload.append(payload)
    incoming_payload = []
    for edge in incoming:
        payload = dict(edge)
        payload["subject_name"] = resolve_ref_name(conn, edge["subject_type"], edge["subject_ref"])
        incoming_payload.append(payload)
    conn.close()
    return {
        "person_id": row["person_id"],
        "display_name": row["display_name"],
        "outgoing_edges": outgoing_payload,
        "incoming_edges": incoming_payload,
        "facts": fact_groups,
    }


def candidate_claim_review(
    db_path: Path,
    *,
    limit: int = 50,
    person_query: str | None = None,
    channel: str = "all",
) -> list[dict[str, Any]]:
    conn = open_store(db_path)
    conditions = ["sc.claim_status = 'candidate'"]
    params: list[Any] = []
    if channel != "all":
        conditions.append("sc.channel = ?")
        params.append(channel)
    if person_query:
        row = resolve_person(db_path, person_query)
        if row:
            conditions.append("sc.person_id = ?")
            params.append(row["person_id"])
        else:
            like = f"%{normalize_text(person_query).casefold()}%"
            conditions.append("lower(p.display_name) LIKE ?")
            params.append(like)
    rows = conn.execute(
        f"""
        SELECT
            sc.person_id,
            p.display_name,
            sc.channel,
            sc.claim_type,
            sc.predicate,
            sc.object_value,
            sc.normalized_value,
            COUNT(*) AS support_count,
            MAX(sc.confidence) AS max_confidence,
            MAX(sc.observed_at) AS last_observed,
            group_concat(DISTINCT sc.source_message_id) AS source_message_ids
        FROM semantic_claims sc
        JOIN people p ON p.person_id = sc.person_id
        WHERE {' AND '.join(conditions)}
        GROUP BY sc.person_id, p.display_name, sc.channel, sc.claim_type, sc.predicate, sc.object_value, sc.normalized_value
        ORDER BY support_count DESC, max_confidence DESC, last_observed DESC
        LIMIT ?
        """,
        (*params, max(limit * 4, 50)),
    ).fetchall()
    conn.close()
    payload: list[dict[str, Any]] = []
    for row in rows:
        item = dict(row)
        if not person_query and is_probably_noise_name(item["display_name"]):
            continue
        payload.append(item)
        if len(payload) >= limit:
            break
    return payload


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
        if is_probably_noise_name(record.display_name) and not is_trusted_anchor_name(record.display_name):
            continue
        if record.display_name.startswith("+") and not is_trusted_anchor_name(record.display_name):
            continue
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


def operating_state(
    db_path: Path,
    *,
    reconnect_limit: int = 5,
    loop_limit: int = 5,
    doc_limit: int = 10,
) -> dict[str, Any]:
    brief = weekly_brief(db_path, reconnect_limit=reconnect_limit, loop_limit=loop_limit, date_limit=3)
    conn = open_store(db_path)
    docs = promoted_documents(conn, limit=max(doc_limit * 2, 12))
    conn.close()
    company_docs = [item for item in docs if item["domain"] == "company"][:doc_limit]
    personal_docs = [item for item in docs if item["domain"] == "personal"][:doc_limit]
    general_docs = [item for item in docs if item["domain"] == "general"][: max(doc_limit // 2, 3)]
    return {
        "generated_at": now_utc().isoformat(),
        "core_anchors": brief["core_anchors"],
        "priority_reconnect": brief["priority_reconnect"],
        "open_loops": brief["open_loops"],
        "important_dates": brief["important_dates"],
        "company_context_docs": company_docs,
        "personal_context_docs": personal_docs,
        "general_context_docs": general_docs,
    }


def prompt_surface_block(db_path: Path, reconnect_limit: int = 5, loop_limit: int = 5) -> str:
    brief = operating_state(db_path, reconnect_limit=reconnect_limit, loop_limit=loop_limit, doc_limit=4)
    lines = [
        "## Operating Snapshot",
        "",
        f"- Refreshed: {brief['generated_at']}",
        "- Keep this small. Full detail lives in `memory/RELATIONSHIP-RADAR.md`, `memory/PEOPLE-INDEX.md`, `memory/PROMOTED-DOCS.md`, and `memory/people/*.md`.",
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

    lines.extend(["", "### Company Context", ""])
    for item in brief["company_context_docs"]:
        lines.append(
            f"- {item['title']} — {item['source_channel']}; updated {item['updated_at'] or 'unknown'}; {item['excerpt'] or ', '.join(item['reasons'])}"
        )
    if not brief["company_context_docs"]:
        lines.append("- No promoted company docs yet.")

    lines.extend(["", "### Personal Context", ""])
    for item in brief["personal_context_docs"]:
        lines.append(
            f"- {item['title']} — {item['source_channel']}; updated {item['updated_at'] or 'unknown'}; {item['excerpt'] or ', '.join(item['reasons'])}"
        )
    if not brief["personal_context_docs"]:
        lines.append("- No promoted personal docs yet.")

    lines.extend(
        [
            "",
            "### Operating Rule",
            "",
            "- Archive broadly, promote selectively, and keep this surface compact.",
            "- Do not guess about network or company details when the relationship-intelligence toolchain can answer them; query it directly and keep this snapshot in sync.",
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
        summary = summarize_row_with_conn(conn, row)
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
    operating = operating_state(db_path, reconnect_limit=5, loop_limit=5, doc_limit=8)
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

    promoted = promoted_documents(conn, limit=24)
    promoted_lines = [
        "# Promoted Context",
        "",
        "High-value source documents that should influence Linus's operating context without flooding the prompt surface.",
        "",
        "## Company Context",
        "",
    ]
    company_docs = [item for item in promoted if item["domain"] == "company"]
    personal_docs = [item for item in promoted if item["domain"] == "personal"]
    general_docs = [item for item in promoted if item["domain"] == "general"]
    for item in company_docs[:10]:
        promoted_lines.append(
            f"- {item['title']} [{item['source_channel']}] — updated {item['updated_at'] or 'unknown'}; score {item['score']}; {item['excerpt'] or ', '.join(item['reasons'])}"
        )
    if not company_docs[:10]:
        promoted_lines.append("- None promoted yet.")
    promoted_lines.extend(["", "## Personal Context", ""])
    for item in personal_docs[:10]:
        promoted_lines.append(
            f"- {item['title']} [{item['source_channel']}] — updated {item['updated_at'] or 'unknown'}; score {item['score']}; {item['excerpt'] or ', '.join(item['reasons'])}"
        )
    if not personal_docs[:10]:
        promoted_lines.append("- None promoted yet.")
    promoted_lines.extend(["", "## General Context", ""])
    for item in general_docs[:6]:
        promoted_lines.append(
            f"- {item['title']} [{item['source_channel']}] — updated {item['updated_at'] or 'unknown'}; score {item['score']}; {item['excerpt'] or ', '.join(item['reasons'])}"
        )
    if not general_docs[:6]:
        promoted_lines.append("- None promoted yet.")
    write_text(memory_dir / "PROMOTED-DOCS.md", "\n".join(promoted_lines))

    hot_lines = [
        "# Operating State",
        "",
        "This is the compact prompt surface for Linus. Keep it small and action-biased.",
        "",
        f"- Refreshed: {operating['generated_at']}",
        "",
        "## Core Anchors",
        "",
    ]
    for item in operating["core_anchors"]:
        hot_lines.append(
            f"- {item['display_name']} — {item['relationship']}; last touch {item['last_touch_date'] or 'unknown'}; {item['open_loop'] or item['why_they_matter']}"
        )
    hot_lines.extend(["", "## Priority Reconnect", ""])
    for item in operating["priority_reconnect"][:5]:
        hot_lines.append(f"- {item['display_name']} — {item['recommended_action']} ({item['why_now']})")
    if not operating["priority_reconnect"][:5]:
        hot_lines.append("- None right now.")
    hot_lines.extend(["", "## Open Loops", ""])
    for item in operating["open_loops"][:5]:
        action = item["open_actions"][0]["description"] if item.get("open_actions") else item["recommended_action"]
        hot_lines.append(f"- {item['display_name']} — {action}")
    if not operating["open_loops"][:5]:
        hot_lines.append("- None right now.")
    hot_lines.extend(["", "## Company Context", ""])
    for item in operating["company_context_docs"][:6]:
        hot_lines.append(f"- {item['title']} — {item['excerpt'] or ', '.join(item['reasons'])}")
    if not operating["company_context_docs"][:6]:
        hot_lines.append("- None promoted yet.")
    hot_lines.extend(["", "## Personal Context", ""])
    for item in operating["personal_context_docs"][:6]:
        hot_lines.append(f"- {item['title']} — {item['excerpt'] or ', '.join(item['reasons'])}")
    if not operating["personal_context_docs"][:6]:
        hot_lines.append("- None promoted yet.")
    write_text(memory_dir / "HOT-STATE.md", "\n".join(hot_lines))

    doc_rows = conn.execute(
        """
        SELECT source_channel, source_kind, title, author, updated_at, excerpt
        FROM source_documents
        ORDER BY updated_at DESC, title ASC
        LIMIT 150
        """
    ).fetchall()
    doc_lines = [
        "# Knowledge Sources",
        "",
        "Imported evidence sources that Linus can use for longer-horizon recall and company/personal context.",
        "",
    ]
    for row in doc_rows:
        suffix = f" — {row['excerpt']}" if row["excerpt"] else ""
        author = f" | {row['author']}" if row["author"] else ""
        updated = row["updated_at"] or "unknown"
        doc_lines.append(
            f"- [{row['source_channel']}/{row['source_kind']}] {row['title']} | updated {updated}{author}{suffix}"
        )
    if not doc_rows:
        doc_lines.append("- No source documents imported yet.")
    write_text(memory_dir / "DOCS-INDEX.md", "\n".join(doc_lines))

    overview = "\n".join(
        [
            "# Relationship Intelligence",
            "",
            "Linus uses this subsystem to stay aware of Sunil's network, maintain warm relationships,",
            "track open loops, and suggest proactive outreach without flooding the prompt with raw archives.",
            "",
            "## Ground Rules",
            "",
            "- This is not a generic CRM. It is a relationship-awareness layer.",
            "- Archive broadly in raw evidence, but promote selectively into the graph and hot memory.",
            "- Prefer the compact operating surface in `memory/HOT-STATE.md` first, then `memory/RELATIONSHIP-RADAR.md`, then deeper indexes.",
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
            "- `import-whatsapp --source-file exported.ndjson`",
            "- `import-slack-archive --archive-dir slack-export-dir`",
            "- `import-google-gmail --account-email sunil@tribble.ai`",
            "- `import-google-calendar --account-email sunil@tribble.ai`",
            "- `import-google-drive --account-email sunil@tribble.ai` (strategic metadata + promoted bodies)",
            "- `import-roam-notes --notes-dir roam-export-dir`",
            "- `messages --channel whatsapp --days 2 --direction inbound`",
            "- `docs-search \"tribble brief\" --channel drive`",
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


def upsert_channel_contact(conn: sqlite3.Connection, record: dict[str, Any]) -> None:
    updated_at = record.get("updated_at") or now_utc().isoformat()
    conn.execute(
        """
        INSERT INTO channel_contacts (
            channel, account_id, contact_id, display_name, short_name, phone, email, raw_json, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(channel, account_id, contact_id) DO UPDATE SET
            display_name = excluded.display_name,
            short_name = excluded.short_name,
            phone = excluded.phone,
            email = excluded.email,
            raw_json = excluded.raw_json,
            updated_at = excluded.updated_at
        """,
        (
            record.get("channel", "whatsapp"),
            record.get("account_id", ""),
            record.get("contact_id", ""),
            normalize_text(record.get("display_name")),
            normalize_text(record.get("short_name")),
            normalize_phone(record.get("phone")),
            normalize_email(record.get("email")),
            safe_json(record.get("raw", {})),
            updated_at,
        ),
    )


def upsert_conversation_thread(conn: sqlite3.Connection, record: dict[str, Any]) -> None:
    updated_at = record.get("updated_at") or now_utc().isoformat()
    chat_id = normalize_text(record.get("chat_id"))
    if not chat_id:
        return
    chat_name = normalize_text(record.get("chat_name"))
    chat_phone = normalize_phone(record.get("chat_phone")) or phone_from_chat_id(chat_id)
    last_message_at = record.get("last_message_at")
    conn.execute(
        """
        INSERT INTO conversation_threads (
            channel, account_id, chat_id, chat_name, chat_phone, is_group, last_message_at, raw_json, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(channel, account_id, chat_id) DO UPDATE SET
            chat_name = CASE
                WHEN excluded.chat_name <> '' THEN excluded.chat_name
                ELSE conversation_threads.chat_name
            END,
            chat_phone = CASE
                WHEN excluded.chat_phone <> '' THEN excluded.chat_phone
                ELSE conversation_threads.chat_phone
            END,
            is_group = excluded.is_group,
            last_message_at = CASE
                WHEN conversation_threads.last_message_at IS NULL THEN excluded.last_message_at
                WHEN excluded.last_message_at IS NULL THEN conversation_threads.last_message_at
                WHEN excluded.last_message_at > conversation_threads.last_message_at THEN excluded.last_message_at
                ELSE conversation_threads.last_message_at
            END,
            raw_json = excluded.raw_json,
            updated_at = excluded.updated_at
        """,
        (
            record.get("channel", "whatsapp"),
            record.get("account_id", ""),
            chat_id,
            chat_name,
            chat_phone,
            1 if record.get("is_group") else 0,
            last_message_at,
            safe_json(record.get("raw", {})),
            updated_at,
        ),
    )


def person_hint_for_message(record: dict[str, Any]) -> tuple[str, str]:
    is_group = bool(record.get("is_group"))
    direction = normalize_text(record.get("direction")).lower() or "inbound"
    if is_group:
        if direction == "inbound":
            return normalize_text(record.get("sender_name")), normalize_phone(record.get("sender_phone"))
        return "", ""
    return (
        normalize_text(record.get("counterpart_name") or record.get("chat_name") or record.get("sender_name")),
        normalize_phone(record.get("counterpart_phone") or record.get("chat_phone") or record.get("sender_phone")),
    )


def import_whatsapp_export(
    db_path: Path,
    source_file: Path,
    *,
    memory_dir: Path | None = None,
    top_n: int = 250,
) -> dict[str, Any]:
    conn = open_store(db_path)
    imported_messages = 0
    imported_contacts = 0
    imported_threads = 0
    linked_people = 0
    created_people = 0

    for raw_line in source_file.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line:
            continue
        record = json.loads(line)
        kind = normalize_text(record.get("kind")).lower()
        if kind == "contact":
            upsert_channel_contact(conn, record)
            imported_contacts += 1
            continue
        if kind == "chat":
            upsert_conversation_thread(conn, record)
            imported_threads += 1
            continue
        if kind != "message":
            continue

        sent_at = normalize_text(record.get("sent_at")) or now_utc().isoformat()
        chat_id = normalize_text(record.get("chat_id"))
        message_id = normalize_text(record.get("message_id"))
        if not chat_id or not message_id:
            continue

        upsert_conversation_thread(
            conn,
            {
                "channel": record.get("channel", "whatsapp"),
                "account_id": record.get("account_id", ""),
                "chat_id": chat_id,
                "chat_name": record.get("chat_name", ""),
                "chat_phone": record.get("chat_phone", ""),
                "is_group": record.get("is_group", False),
                "last_message_at": sent_at,
                "raw": record.get("chat_raw", record),
                "updated_at": now_utc().isoformat(),
            },
        )

        person_name, person_phone = person_hint_for_message(record)
        person_id = None
        if person_name or person_phone:
            existing = match_person_row(conn, person_phone, person_name)
            person_id = ensure_person_for_counterparty(
                conn,
                display_name=person_name,
                phone=person_phone,
                channel=record.get("channel", "whatsapp"),
            )
            if person_id:
                linked_people += 1
                if not existing:
                    created_people += 1

        excerpt = trim_message_excerpt(record.get("text") or record.get("excerpt") or record.get("message_type", ""))
        cursor = conn.execute(
            """
            INSERT OR IGNORE INTO message_events (
                channel, account_id, chat_id, chat_name, chat_phone, is_group, message_id,
                sender_id, sender_name, sender_phone, sender_email, counterpart_name, counterpart_phone, counterpart_email,
                person_id, direction, sent_at, message_type, text, excerpt, is_history,
                raw_json, imported_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                record.get("channel", "whatsapp"),
                record.get("account_id", ""),
                chat_id,
                normalize_text(record.get("chat_name")),
                normalize_phone(record.get("chat_phone")) or phone_from_chat_id(chat_id),
                1 if record.get("is_group") else 0,
                message_id,
                normalize_text(record.get("sender_id")),
                normalize_text(record.get("sender_name")),
                normalize_phone(record.get("sender_phone")),
                normalize_email(record.get("sender_email")),
                normalize_text(record.get("counterpart_name")),
                normalize_phone(record.get("counterpart_phone")),
                normalize_email(record.get("counterpart_email")),
                person_id,
                normalize_text(record.get("direction")).lower() or "inbound",
                sent_at,
                normalize_text(record.get("message_type")) or "text",
                normalize_text(record.get("text")),
                excerpt,
                1 if record.get("is_history", True) else 0,
                safe_json(record),
                now_utc().isoformat(),
            ),
        )
        if cursor.rowcount <= 0:
            continue
        imported_messages += 1
        if person_id:
            maybe_insert_touch_event(
                conn,
                person_id=person_id,
                touched_at=sent_at,
                channel=record.get("channel", "whatsapp"),
                note=excerpt or normalize_text(record.get("message_type")) or "WhatsApp message",
                direction=normalize_text(record.get("direction")).lower() or "inbound",
                source="whatsapp-import",
            )

    conn.commit()
    conn.close()
    rerendered = maybe_rerender_after_touch(db_path, memory_dir, top_n)
    return {
        "source_file": str(source_file),
        "imported_messages": imported_messages,
        "imported_contacts": imported_contacts,
        "imported_threads": imported_threads,
        "linked_people": linked_people,
        "created_people": created_people,
        "rerendered_memory_dir": str(rerendered) if rerendered else None,
    }


def recent_messages(
    db_path: Path,
    *,
    days: int,
    limit: int,
    direction: str,
    person_query: str | None = None,
    chat_query: str | None = None,
    channel: str = "whatsapp",
) -> dict[str, Any]:
    conn = open_store(db_path)
    cutoff = (now_utc() - timedelta(days=days)).isoformat()
    conditions = ["m.channel = ?", "m.sent_at >= ?"]
    params: list[Any] = [channel, cutoff]

    normalized_direction = normalize_text(direction).lower()
    if normalized_direction and normalized_direction != "any":
        conditions.append("m.direction = ?")
        params.append(normalized_direction)

    if person_query:
        row = resolve_person(db_path, person_query)
        if row:
            conditions.append("m.person_id = ?")
            params.append(row["person_id"])
        else:
            like = f"%{normalize_text(person_query).casefold()}%"
            conditions.append(
                "(lower(m.counterpart_name) LIKE ? OR lower(m.sender_name) LIKE ? OR lower(m.chat_name) LIKE ?)"
            )
            params.extend([like, like, like])

    if chat_query:
        like = f"%{normalize_text(chat_query).casefold()}%"
        conditions.append("(lower(m.chat_name) LIKE ? OR lower(m.chat_id) LIKE ?)")
        params.extend([like, like])

    rows = conn.execute(
        f"""
        SELECT
            m.*,
            p.display_name AS person_display_name,
            p.relationship_label AS person_relationship_label
        FROM message_events m
        LEFT JOIN people p ON p.person_id = m.person_id
        WHERE {' AND '.join(conditions)}
        ORDER BY m.sent_at DESC
        LIMIT ?
        """,
        (*params, limit),
    ).fetchall()
    conn.close()

    messages = [
        {
            "sent_at": row["sent_at"],
            "direction": row["direction"],
            "chat_name": row["chat_name"],
            "chat_id": row["chat_id"],
            "is_group": bool(row["is_group"]),
            "person_id": row["person_id"],
            "person_display_name": row["person_display_name"] or row["counterpart_name"] or row["sender_name"],
            "person_relationship_label": row["person_relationship_label"] or "",
            "sender_name": row["sender_name"],
            "sender_phone": row["sender_phone"],
            "counterpart_name": row["counterpart_name"],
            "counterpart_phone": row["counterpart_phone"],
            "message_type": row["message_type"],
            "text": row["text"] or row["excerpt"],
            "is_history": bool(row["is_history"]),
        }
        for row in rows
    ]
    return {
        "channel": channel,
        "days": days,
        "direction": normalized_direction or "any",
        "count": len(messages),
        "messages": messages,
    }


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
    whatsapp_messages = conn.execute(
        "SELECT COUNT(*) AS c FROM message_events WHERE channel = 'whatsapp'"
    ).fetchone()["c"]
    whatsapp_threads = conn.execute(
        "SELECT COUNT(*) AS c FROM conversation_threads WHERE channel = 'whatsapp'"
    ).fetchone()["c"]
    semantic_claims = conn.execute(
        "SELECT COUNT(*) AS c FROM semantic_claims WHERE channel = 'whatsapp'"
    ).fetchone()["c"]
    relationship_edges = conn.execute(
        "SELECT COUNT(*) AS c FROM relationship_edges WHERE channel = 'whatsapp'"
    ).fetchone()["c"]
    entities = conn.execute(
        "SELECT COUNT(*) AS c FROM entities WHERE source_channel = 'whatsapp'"
    ).fetchone()["c"]
    claim_status_rows = conn.execute(
        """
        SELECT claim_status, COUNT(*) AS c
        FROM semantic_claims
        WHERE channel = 'whatsapp'
        GROUP BY claim_status
        ORDER BY c DESC, claim_status ASC
        """
    ).fetchall()
    signal_rows = conn.execute(
        """
        SELECT channel, COUNT(*) AS c
        FROM relationship_signals
        GROUP BY channel
        ORDER BY c DESC, channel ASC
        """
    ).fetchall()
    claims_by_channel = conn.execute(
        """
        SELECT channel, COUNT(*) AS c
        FROM semantic_claims
        GROUP BY channel
        ORDER BY c DESC, channel ASC
        """
    ).fetchall()
    edges_by_channel = conn.execute(
        """
        SELECT channel, COUNT(*) AS c
        FROM relationship_edges
        GROUP BY channel
        ORDER BY c DESC, channel ASC
        """
    ).fetchall()
    message_rows = conn.execute(
        """
        SELECT channel, COUNT(*) AS c
        FROM message_events
        GROUP BY channel
        ORDER BY c DESC, channel ASC
        """
    ).fetchall()
    document_rows = conn.execute(
        """
        SELECT source_channel, COUNT(*) AS c
        FROM source_documents
        GROUP BY source_channel
        ORDER BY c DESC, source_channel ASC
        """
    ).fetchall()
    conn.close()
    return {
        "total_people": total,
        "tiers": {row["tier"]: row["c"] for row in counts},
        "whatsapp_messages": whatsapp_messages,
        "whatsapp_threads": whatsapp_threads,
        "semantic_claims": semantic_claims,
        "semantic_claim_status": {row["claim_status"]: row["c"] for row in claim_status_rows},
        "relationship_edges": relationship_edges,
        "entities": entities,
        "relationship_signals_by_channel": {row["channel"]: row["c"] for row in signal_rows},
        "semantic_claims_by_channel": {row["channel"]: row["c"] for row in claims_by_channel},
        "relationship_edges_by_channel": {row["channel"]: row["c"] for row in edges_by_channel},
        "message_events_by_channel": {row["channel"]: row["c"] for row in message_rows},
        "source_documents_by_channel": {row["source_channel"]: row["c"] for row in document_rows},
    }


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

    claims_cmd = subparsers.add_parser("claims", help="Inspect semantic claims and edges for one person")
    claims_cmd.add_argument("query", help="Name, alias, email, or phone")
    claims_cmd.add_argument("--limit", type=int, default=50)
    claims_cmd.add_argument("--channel", default="all", help="Optional channel filter, defaults to all")
    claims_cmd.add_argument("--json", action="store_true")

    timeline_cmd = subparsers.add_parser("timeline", help="Inspect one person's recent messages, touches, and claims")
    timeline_cmd.add_argument("query", help="Name, alias, email, or phone")
    timeline_cmd.add_argument("--channel", default="whatsapp", help="Channel name, defaults to whatsapp")
    timeline_cmd.add_argument("--limit", type=int, default=20)
    timeline_cmd.add_argument("--json", action="store_true")

    network_cmd = subparsers.add_parser("network", help="Inspect one person's connected entities, edges, and facts")
    network_cmd.add_argument("query", help="Name, alias, email, or phone")
    network_cmd.add_argument("--limit", type=int, default=25)
    network_cmd.add_argument("--json", action="store_true")

    review_cmd = subparsers.add_parser("candidate-claims", help="Review low-confidence semantic claims that may need adjudication")
    review_cmd.add_argument("--person", help="Optional person filter")
    review_cmd.add_argument("--channel", default="all", help="Optional channel filter, defaults to all")
    review_cmd.add_argument("--limit", type=int, default=50)
    review_cmd.add_argument("--json", action="store_true")

    entity_search = subparsers.add_parser("entity-search", help="Search canonical entities in the relationship KG")
    entity_search.add_argument("query", help="Entity name or alias")
    entity_search.add_argument("--limit", type=int, default=25)
    entity_search.add_argument("--json", action="store_true")

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

    import_whatsapp = subparsers.add_parser("import-whatsapp", help="Import normalized WhatsApp export records")
    import_whatsapp.add_argument("--source-file", required=True, help="Path to NDJSON exported from the WhatsApp sync tool")
    import_whatsapp.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_whatsapp.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_whatsapp.add_argument("--json", action="store_true")

    import_imessage = subparsers.add_parser("import-imessage-profiles", help="Import curated iMessage profile summaries into the relationship store")
    import_imessage.add_argument("--profiles-dir", required=True, help="Directory containing iMessage profile JSON files")
    import_imessage.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_imessage.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_imessage.add_argument("--json", action="store_true")

    import_slack = subparsers.add_parser("import-slack-archive", help="Import a slackdump SQLite archive into the relationship store")
    import_slack.add_argument("--archive-dir", required=True, help="Directory containing slackdump.sqlite")
    import_slack.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_slack.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_slack.add_argument("--json", action="store_true")

    import_gmail = subparsers.add_parser("import-google-gmail", help="Import Gmail metadata into the relationship store")
    import_gmail.add_argument("--account-email", help="Google account email; defaults to first available credential")
    import_gmail.add_argument("--query", default="in:anywhere", help="Gmail search query for backfill")
    import_gmail.add_argument("--max-messages", type=int, default=0, help="Optional cap; 0 means no cap")
    import_gmail.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_gmail.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_gmail.add_argument("--json", action="store_true")

    guided_gmail = subparsers.add_parser(
        "gmail-guided",
        help="Use graph signal to retrieve high-value Gmail messages without broad mailbox import",
    )
    guided_gmail.add_argument("--account-email", help="Google account email; defaults to first available credential")
    guided_gmail.add_argument("--person", default="", help="Optional person anchor from the relationship graph")
    guided_gmail.add_argument("--objective", default="", help="Optional task/objective text used to shape retrieval")
    guided_gmail.add_argument("--days", type=int, default=365, help="How many trailing days of Gmail to search")
    guided_gmail.add_argument("--limit", type=int, default=20, help="Maximum ranked messages to return")
    guided_gmail.add_argument("--query-limit", type=int, default=8, help="Maximum graph-shaped Gmail queries to run")
    guided_gmail.add_argument("--json", action="store_true")

    import_calendar = subparsers.add_parser("import-google-calendar", help="Import Google Calendar events into the relationship store")
    import_calendar.add_argument("--account-email", help="Google account email; defaults to first available credential")
    import_calendar.add_argument("--past-days", type=int, default=365, help="How many days back to import")
    import_calendar.add_argument("--future-days", type=int, default=180, help="How many days forward to import")
    import_calendar.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_calendar.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_calendar.add_argument("--json", action="store_true")

    import_drive = subparsers.add_parser("import-google-drive", help="Import Google Drive metadata and text into the evidence store")
    import_drive.add_argument("--account-email", help="Google account email; defaults to first available credential")
    import_drive.add_argument("--metadata-only", action="store_true", help="Only import metadata/description, not promoted bodies")
    import_drive.add_argument("--body-limit", type=int, default=250, help="Maximum Drive docs to fetch full text for")
    import_drive.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_drive.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_drive.add_argument("--json", action="store_true")

    import_roam = subparsers.add_parser("import-roam-notes", help="Import markdown notes from a Roam export into the evidence store")
    import_roam.add_argument("--notes-dir", required=True, help="Directory containing extracted Roam markdown")
    import_roam.add_argument("--memory-dir", help="Optional memory directory to re-render after import")
    import_roam.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    import_roam.add_argument("--json", action="store_true")

    reconcile_whatsapp = subparsers.add_parser(
        "reconcile-whatsapp",
        help="Mass-reconcile WhatsApp people/message history into the relationship ontology",
    )
    reconcile_whatsapp.add_argument("--memory-dir", help="Optional memory directory to re-render after reconciliation")
    reconcile_whatsapp.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    reconcile_whatsapp.add_argument("--json", action="store_true")

    reconcile_identities_cmd = subparsers.add_parser(
        "reconcile-identities",
        help="Merge duplicate people across all imported sources using phone/email identity",
    )
    reconcile_identities_cmd.add_argument("--memory-dir", help="Optional memory directory to re-render after reconciliation")
    reconcile_identities_cmd.add_argument("--top-n", type=int, default=250, help="How many people pages to render when --memory-dir is set")
    reconcile_identities_cmd.add_argument("--json", action="store_true")

    messages = subparsers.add_parser("messages", help="Query recent channel messages from the relationship store")
    messages.add_argument("--channel", default="whatsapp", help="Channel name, defaults to whatsapp")
    messages.add_argument("--days", type=int, default=2, help="How many trailing days to inspect")
    messages.add_argument("--limit", type=int, default=25, help="Maximum messages to return")
    messages.add_argument(
        "--direction",
        default="any",
        help="inbound|outbound|any",
    )
    messages.add_argument("--person", help="Optional person name/alias filter")
    messages.add_argument("--chat", help="Optional chat name or chat id filter")
    messages.add_argument("--json", action="store_true")

    docs_search = subparsers.add_parser("docs-search", help="Search imported source documents")
    docs_search.add_argument("query", help="Document query")
    docs_search.add_argument("--channel", default="all", help="Optional source channel filter")
    docs_search.add_argument("--limit", type=int, default=20)
    docs_search.add_argument("--json", action="store_true")

    operating_cmd = subparsers.add_parser("operating-state", help="Show the promoted compact operating state")
    operating_cmd.add_argument("--reconnect-limit", type=int, default=5)
    operating_cmd.add_argument("--loop-limit", type=int, default=5)
    operating_cmd.add_argument("--doc-limit", type=int, default=8)
    operating_cmd.add_argument("--json", action="store_true")

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

    if args.command == "claims":
        payload = semantic_claims_for_person(db_path, args.query, args.limit, channel=args.channel)
        if not payload:
            print(f"No person found for query: {args.query}", file=sys.stderr)
            return 1
        if args.json:
            print_json(payload)
        else:
            print(f"# Semantic Claims: {payload['display_name']}")
            print()
            for item in payload["claims"]:
                print(f"- [{item['claim_type']}] {item['predicate']} -> {item['object_value']} ({item['confidence']})")
            if payload["edges"]:
                print()
                print("## Edges")
                for edge in payload["edges"]:
                    print(
                        f"- {edge['subject_type']}:{edge['subject_ref']} "
                        f"-[{edge['predicate']}]-> {edge['object_type']}:{edge['object_ref']} ({edge['confidence']})"
                    )
        return 0

    if args.command == "timeline":
        payload = person_timeline(db_path, args.query, limit=args.limit, channel=args.channel)
        if not payload:
            print(f"No person found for query: {args.query}", file=sys.stderr)
            return 1
        if args.json:
            print_json(payload)
        else:
            print(f"# Timeline: {payload['display_name']}")
            print()
            print("## Recent Messages")
            for item in payload["recent_messages"]:
                text = item["text"] or item["excerpt"] or item["message_type"]
                print(f"- {item['sent_at']} | {item['direction']} | {item['chat_name'] or item['chat_id']} | {text}")
            if not payload["recent_messages"]:
                print("- None")
            print()
            print("## Recent Touches")
            for item in payload["recent_touches"]:
                print(f"- {item['touched_at']} | {item['channel']} | {item['note']}")
            if not payload["recent_touches"]:
                print("- None")
            print()
            print("## Recent Claims")
            for item in payload["recent_claims"]:
                print(f"- {item['observed_at']} | [{item['claim_status']}] {item['predicate']} -> {item['object_value']}")
            if not payload["recent_claims"]:
                print("- None")
        return 0

    if args.command == "network":
        payload = person_network(db_path, args.query, limit=args.limit)
        if not payload:
            print(f"No person found for query: {args.query}", file=sys.stderr)
            return 1
        if args.json:
            print_json(payload)
        else:
            print(f"# Network: {payload['display_name']}")
            print()
            print("## Outgoing Edges")
            for edge in payload["outgoing_edges"]:
                print(f"- {edge['predicate']} -> {edge['object_type']}:{edge['object_name']} ({edge['confidence']})")
            if not payload["outgoing_edges"]:
                print("- None")
            print()
            print("## Incoming Edges")
            for edge in payload["incoming_edges"]:
                print(f"- {edge['subject_type']}:{edge['subject_name']} -[{edge['predicate']}]-> person ({edge['confidence']})")
            if not payload["incoming_edges"]:
                print("- None")
            print()
            print("## Facts")
            if not payload["facts"]:
                print("- None")
            else:
                for fact_type, values in sorted(payload["facts"].items()):
                    rendered = ", ".join(item["fact_value"] for item in values[:6])
                    print(f"- {fact_type}: {rendered}")
        return 0

    if args.command == "candidate-claims":
        payload = candidate_claim_review(db_path, limit=args.limit, person_query=args.person, channel=args.channel)
        if args.json:
            print_json(payload)
        else:
            for item in payload:
                print(
                    f"{item['display_name']} | {item['predicate']} -> {item['object_value']} | "
                    f"support={item['support_count']} | confidence={item['max_confidence']} | last={item['last_observed'] or 'unknown'}"
                )
        return 0

    if args.command == "entity-search":
        payload = search_entities(db_path, args.query, args.limit)
        if args.json:
            print_json(payload)
        else:
            for item in payload:
                aliases = item.get("aliases") or ""
                alias_suffix = f" | aliases={aliases}" if aliases else ""
                print(
                    f"{item['entity_type']} | {item['canonical_name']} | id={item['entity_id']} "
                    f"| confidence={item['confidence']}{alias_suffix}"
                )
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

    if args.command == "import-whatsapp":
        payload = import_whatsapp_export(
            db_path,
            Path(args.source_file).expanduser().resolve(),
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_messages']} WhatsApp messages from {payload['source_file']}")
            print(f"Linked people: {payload['linked_people']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "import-imessage-profiles":
        payload = import_imessage_profiles(
            db_path,
            Path(args.profiles_dir).expanduser().resolve(),
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_profiles']} iMessage profiles from {payload['profiles_dir']}")
            print(f"Linked people: {payload['linked_people']}")
            print(f"Relationship signals: {payload['relationship_signals']}")
            print(f"Semantic claims added: {payload['semantic_claims_added']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "import-slack-archive":
        payload = import_slack_archive(
            db_path,
            Path(args.archive_dir).expanduser().resolve(),
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_messages']} Slack messages from {payload['archive_dir']}")
            print(f"Linked people: {payload['linked_people']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "import-google-gmail":
        payload = import_google_gmail(
            db_path,
            account_email=args.account_email,
            query=args.query,
            max_messages=args.max_messages,
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_messages']} Gmail messages for {payload['account_email']}")
            print(f"Linked people: {payload['linked_people']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "gmail-guided":
        payload = guided_gmail_results(
            db_path,
            account_email=args.account_email,
            person_query=args.person,
            objective=args.objective,
            days=args.days,
            limit=args.limit,
            query_limit=args.query_limit,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"# Guided Gmail Retrieval ({payload['account_email']})")
            print()
            if payload.get("queries"):
                print("## Queries")
                for item in payload["queries"]:
                    print(f"- {item['query']}")
                    reasons = item.get("reasons") or []
                    if reasons:
                        print(f"  why: {', '.join(reasons[:4])}")
                print()
            else:
                print("- No graph-shaped queries could be built.")
                return 0
            print("## Results")
            if not payload.get("results"):
                print("- No matching Gmail messages found.")
                return 0
            for item in payload["results"]:
                matched_people = ", ".join(item.get("matched_people") or []) or "no direct people match"
                why = "; ".join(item.get("why") or []) or "graph/objective match"
                print(
                    f"- {item['sent_at']} | score={item['score']} | {item['direction']} | "
                    f"{item['subject']} | {matched_people}"
                )
                print(f"  why: {why}")
                print(f"  snippet: {item['snippet']}")
        return 0

    if args.command == "import-google-calendar":
        payload = import_google_calendar(
            db_path,
            account_email=args.account_email,
            past_days=args.past_days,
            future_days=args.future_days,
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_events']} calendar events for {payload['account_email']}")
            print(f"Linked people: {payload['linked_people']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "import-google-drive":
        payload = import_google_drive(
            db_path,
            account_email=args.account_email,
            metadata_only=args.metadata_only,
            body_limit=args.body_limit,
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_files']} Drive files for {payload['account_email']}")
            print(f"Fetched bodies for {payload['imported_bodies']} promoted docs")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "import-roam-notes":
        payload = import_roam_notes(
            db_path,
            Path(args.notes_dir).expanduser().resolve(),
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Imported {payload['imported_notes']} Roam notes from {payload['notes_dir']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "reconcile-whatsapp":
        payload = reconcile_whatsapp_graph(
            db_path,
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Merged people: {payload['merged_people']}")
            print(f"Relinked messages: {payload['relinked_messages']}")
            print(f"Rebuilt touch events: {payload['rebuilt_touch_events']}")
            ontology = payload.get("ontology") or {}
            if ontology:
                print(f"Person facts: {ontology.get('person_facts', 0)}")
                print(f"Relationship signals: {ontology.get('relationship_signals', 0)}")
                print(f"Updated people: {ontology.get('updated_people', 0)}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "reconcile-identities":
        payload = reconcile_identities(
            db_path,
            memory_dir=Path(args.memory_dir).expanduser().resolve() if args.memory_dir else None,
            top_n=args.top_n,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"Merged by phone: {payload['merged_by_phone']}")
            print(f"Merged by email: {payload['merged_by_email']}")
            if payload.get("rerendered_memory_dir"):
                print(f"Re-rendered memory into {payload['rerendered_memory_dir']}")
        return 0

    if args.command == "messages":
        payload = recent_messages(
            db_path,
            days=args.days,
            limit=args.limit,
            direction=args.direction,
            person_query=args.person,
            chat_query=args.chat,
            channel=args.channel,
        )
        if args.json:
            print_json(payload)
        else:
            print(f"# Recent {payload['channel']} messages")
            print()
            print(f"- Window: last {payload['days']} days")
            print(f"- Direction: {payload['direction']}")
            print(f"- Count: {payload['count']}")
            print()
            for item in payload["messages"]:
                text = item["text"] or item["message_type"]
                print(
                    f"- {item['sent_at']} | {item['direction']} | "
                    f"{item['person_display_name'] or item['chat_name'] or item['chat_id']} | {text}"
                )
        return 0

    if args.command == "docs-search":
        payload = search_documents(db_path, args.query, args.limit, channel=args.channel)
        if args.json:
            print_json(payload)
        else:
            for item in payload:
                print(
                    f"{item['source_channel']}/{item['source_kind']} | {item['title']} | "
                    f"{item['updated_at'] or 'unknown'} | {item['excerpt']}"
                )
        return 0

    if args.command == "operating-state":
        payload = operating_state(
            db_path,
            reconnect_limit=args.reconnect_limit,
            loop_limit=args.loop_limit,
            doc_limit=args.doc_limit,
        )
        if args.json:
            print_json(payload)
        else:
            print(prompt_surface_block(db_path, reconnect_limit=args.reconnect_limit, loop_limit=args.loop_limit))
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
