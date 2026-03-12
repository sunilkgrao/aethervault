#!/usr/bin/env python3
"""Extract WhatsApp artifacts from an encrypted iPhone backup.

Uses the `iOSbackup` package inside the local venv created for this repo.
The goal is to produce a clean, auditable archive that can feed Linus's
relationship-intelligence and transcript parsing pipelines.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import Any

from iOSbackup import iOSbackup


WHATSAPP_DOMAIN_MARKERS = (
    "whatsapp",
)

KEY_FILE_NAMES = {
    "ChatStorage.sqlite",
    "ContactsV2.sqlite",
    "CallHistory.sqlite",
}


def normalize_text(value: Any) -> str:
    if value is None:
        return ""
    return str(value).strip()


def list_whatsapp_domains(backup: iOSbackup) -> list[str]:
    domains = backup.getDomains()
    return sorted(domain for domain in domains if any(marker in domain.casefold() for marker in WHATSAPP_DOMAIN_MARKERS))


def backup_entries(backup: iOSbackup, domains: list[str]) -> list[dict[str, Any]]:
    selected = []
    domain_set = set(domains)
    for entry in backup.getBackupFilesList():
        if entry["domain"] in domain_set:
            selected.append(
                {
                    "domain": entry["domain"],
                    "relativePath": entry["relativePath"],
                    "fileID": entry["fileID"],
                    "flags": entry.get("flags"),
                    "file": entry.get("file"),
                }
            )
    return selected


def extract_domains(backup: iOSbackup, domains: list[str], output_dir: Path) -> dict[str, list[dict[str, Any]]]:
    extracted: dict[str, list[dict[str, Any]]] = {}
    for domain in domains:
        safe_domain = domain.replace("/", "_")
        target = output_dir / "domains" / safe_domain
        target.mkdir(parents=True, exist_ok=True)
        extracted[domain] = backup.getFolderDecryptedCopy(
            relativePath="",
            targetFolder=str(target),
            includeDomains=[domain],
        )
    return extracted


def copy_key_files(extracted_root: Path, entries: list[dict[str, Any]]) -> list[str]:
    copied: list[str] = []
    key_dir = extracted_root / "key-files"
    key_dir.mkdir(parents=True, exist_ok=True)

    for entry in entries:
        rel = normalize_text(entry["relativePath"])
        name = Path(rel).name
        if name not in KEY_FILE_NAMES:
            continue
        domain = normalize_text(entry["domain"]).replace("/", "_")
        source = extracted_root / "domains" / domain / rel
        if not source.exists():
            continue
        dest = key_dir / f"{domain}__{name}"
        dest.write_bytes(source.read_bytes())
        copied.append(str(dest))
    return copied


def main() -> int:
    parser = argparse.ArgumentParser(description="Extract WhatsApp artifacts from an encrypted iPhone backup")
    parser.add_argument("--backup-root", required=True, help="Directory containing iPhone backup folders")
    parser.add_argument("--udid", required=True, help="iPhone backup UDID")
    parser.add_argument("--password", required=False, help="Encrypted iPhone backup password")
    parser.add_argument("--out-dir", required=True, help="Output directory for extracted artifacts")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    password = args.password or os.environ.get("IPHONE_BACKUP_PASSWORD")
    if not password:
        raise SystemExit("Backup password required via --password or IPHONE_BACKUP_PASSWORD")

    backup_root = Path(args.backup_root).expanduser().resolve()
    out_dir = Path(args.out_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    backup = iOSbackup(udid=args.udid, cleartextpassword=password, backuproot=str(backup_root))
    basic_info = iOSbackup.getDeviceBasicInfo(udid=args.udid, backuproot=str(backup_root))
    domains = list_whatsapp_domains(backup)
    entries = backup_entries(backup, domains)

    manifest_path = out_dir / "whatsapp-manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "udid": args.udid,
                "backup_root": str(backup_root),
                "domains": domains,
                "entry_count": len(entries),
                "device": basic_info,
                "entries": entries,
            },
            indent=2,
            ensure_ascii=True,
        )
        + "\n",
        encoding="utf-8",
    )

    extracted = extract_domains(backup, domains, out_dir)
    copied = copy_key_files(out_dir, entries)

    summary = {
        "udid": args.udid,
        "device": basic_info,
        "domains": domains,
        "domain_count": len(domains),
        "entry_count": len(entries),
        "key_files": copied,
        "output_dir": str(out_dir),
    }
    summary_path = out_dir / "summary.json"
    summary_path.write_text(json.dumps(summary, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")

    if args.json:
        print(json.dumps(summary, indent=2, ensure_ascii=True))
    else:
        print(summary_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
