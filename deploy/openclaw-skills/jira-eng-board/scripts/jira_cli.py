#!/usr/bin/env python3
"""Jira ENG Board helper CLI.

Use env vars:
- JIRA_BASE_URL (default https://tribble-ai.atlassian.net)
- JIRA_EMAIL
- JIRA_API_TOKEN

Examples:
  python3 scripts/jira_cli.py search --jql "project = ENG ORDER BY created DESC" --pretty
  python3 scripts/jira_cli.py get ENG-123 --fields summary,status --pretty
  python3 scripts/jira_cli.py create --project ENG --type Task --summary "Title" --description "Body"
  python3 scripts/jira_cli.py attach ENG-123 --file /path/to/file.pdf
  python3 scripts/jira_cli.py update ENG-123 --summary "New title" --description "Updated"
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional


DEFAULT_BASE_URL = "https://tribble-ai.atlassian.net"


def die(message: str, code: int = 1) -> None:
    print(f"[jira-cli] {message}", file=sys.stderr)
    raise SystemExit(code)


def load_env_file_value(path: Path, key: str) -> str:
    if not path.exists():
        return ""
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        current_key, value = line.split("=", 1)
        if current_key.strip() == key:
            return value.strip().strip('"').strip("'")
    return ""


def find_env_value(key: str, default: str = "") -> str:
    candidates = [
        os.environ.get(key, "").strip(),
        load_env_file_value(Path("/root/.secrets/master.env"), key),
        load_env_file_value(Path.home() / ".secrets" / "master.env", key),
        load_env_file_value(Path.cwd() / ".secrets" / "master.env", key),
    ]
    for candidate in candidates:
        if candidate:
            return candidate
    return default


def text_to_adf(text: str) -> Dict[str, Any]:
    lines = text.splitlines()
    content = []
    for line in lines:
        if line.strip() == "":
            content.append({"type": "paragraph", "content": [{"type": "text", "text": ""}]})
        else:
            content.append({"type": "paragraph", "content": [{"type": "text", "text": line}]})
    if not content:
        content = [{"type": "paragraph", "content": [{"type": "text", "text": ""}]}]
    return {"type": "doc", "version": 1, "content": content}


def load_text_file(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")


def load_json_arg(value: str) -> Dict[str, Any]:
    raw = value
    if value.startswith("@"):
        raw = Path(value[1:]).read_text(encoding="utf-8")
    elif Path(value).is_file():
        raw = Path(value).read_text(encoding="utf-8")
    return json.loads(raw)


def split_csv(value: Optional[str]) -> List[str]:
    if not value:
        return []
    return [v.strip() for v in value.split(",") if v.strip()]


def require_auth(email: Optional[str], token: Optional[str], dry_run: bool) -> None:
    if dry_run:
        return
    if not email or not token:
        die("Missing auth. Set JIRA_EMAIL and JIRA_API_TOKEN (or pass --email/--token).")


def build_base_url(base_url: Optional[str]) -> str:
    return (base_url or DEFAULT_BASE_URL).rstrip("/")


def redact_args(args: List[str]) -> List[str]:
    redacted = list(args)
    for i, arg in enumerate(redacted):
        if arg == "-u" and i + 1 < len(redacted):
            auth = redacted[i + 1]
            if ":" in auth:
                user, _ = auth.split(":", 1)
                redacted[i + 1] = f"{user}:***"
    return redacted


def run_curl(args: List[str], dry_run: bool) -> str:
    if dry_run:
        cmd = " ".join(shlex.quote(a) for a in redact_args(args))
        print(f"DRY RUN: {cmd}")
        return ""
    result = subprocess.run(args, text=True, capture_output=True)
    if result.returncode != 0:
        die(result.stderr.strip() or "Request failed")
    return result.stdout.strip()


def print_output(out: str, pretty: bool) -> None:
    if not out:
        return
    if not pretty:
        print(out)
        return
    try:
        data = json.loads(out)
    except json.JSONDecodeError:
        print(out)
        return
    print(json.dumps(data, indent=2))


def curl_json(
    base_url: str,
    email: str,
    token: str,
    method: str,
    path: str,
    body: Optional[Dict[str, Any]] = None,
    headers: Optional[List[str]] = None,
    dry_run: bool = False,
) -> str:
    url = f"{base_url}{path}"
    auth_email = email or "user@example.com"
    auth_token = token or "token"
    args = ["curl", "-sS", "-X", method, "-u", f"{auth_email}:{auth_token}"]
    if headers:
        for header in headers:
            args.extend(["-H", header])
    if body is not None:
        args.extend(["-H", "Content-Type: application/json", "-d", json.dumps(body)])
    args.append(url)
    return run_curl(args, dry_run)


def cmd_projects(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    out = curl_json(args.base_url, args.email, args.token, "GET", "/rest/api/3/project", dry_run=args.dry_run)
    print_output(out, args.pretty)


def cmd_fields(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    out = curl_json(args.base_url, args.email, args.token, "GET", "/rest/api/3/field", dry_run=args.dry_run)
    if args.dry_run:
        return
    if not args.name:
        print_output(out, args.pretty)
        return
    try:
        data = json.loads(out)
    except json.JSONDecodeError:
        print(out)
        return
    name = args.name
    if args.exact:
        filtered = [f for f in data if f.get("name") == name]
    else:
        filtered = [f for f in data if name.lower() in (f.get("name") or "").lower()]
    print(json.dumps(filtered, indent=2))


def cmd_search(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    fields = split_csv(args.fields) or ["summary", "status", "assignee", "priority", "duedate", "updated"]
    body = {"jql": args.jql, "maxResults": args.max_results, "fields": fields}
    out = curl_json(args.base_url, args.email, args.token, "POST", "/rest/api/3/search/jql", body=body, dry_run=args.dry_run)
    print_output(out, args.pretty)


def cmd_get(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    fields = split_csv(args.fields)
    path = f"/rest/api/3/issue/{args.issue}"
    if fields:
        path = f"{path}?fields={','.join(fields)}"
    out = curl_json(args.base_url, args.email, args.token, "GET", path, dry_run=args.dry_run)
    print_output(out, args.pretty)


def apply_fields_json(fields: Dict[str, Any], fields_json: Optional[str]) -> None:
    if not fields_json:
        return
    extra = load_json_arg(fields_json)
    if isinstance(extra, dict) and "fields" in extra and isinstance(extra["fields"], dict):
        extra = extra["fields"]
    if not isinstance(extra, dict):
        die("--fields-json must be a JSON object or an object with a top-level 'fields' key")
    fields.update(extra)


def cmd_create(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    if not args.summary:
        die("--summary is required for create")
    fields: Dict[str, Any] = {
        "project": {"key": args.project},
        "issuetype": {"name": args.type},
        "summary": args.summary,
    }
    description = args.description
    if args.description_file:
        description = load_text_file(args.description_file)
    if description is not None:
        fields["description"] = text_to_adf(description)
    if args.labels:
        fields["labels"] = split_csv(args.labels)
    if args.assignee:
        fields["assignee"] = {"id": args.assignee}
    if args.parent:
        fields["parent"] = {"key": args.parent}
    if args.epic_key:
        if not args.epic_field:
            die("--epic-field is required when using --epic-key")
        fields[args.epic_field] = args.epic_key
    apply_fields_json(fields, args.fields_json)
    body = {"fields": fields}
    out = curl_json(args.base_url, args.email, args.token, "POST", "/rest/api/3/issue", body=body, dry_run=args.dry_run)
    print_output(out, args.pretty)
    if args.dry_run or not args.attach:
        return
    if not out:
        die("Create response was empty; cannot attach files")
    try:
        data = json.loads(out)
        issue_key = data.get("key")
    except json.JSONDecodeError:
        issue_key = None
    if not issue_key:
        die("Could not determine issue key from create response; attach skipped")
    attach_files(issue_key, args.attach, args)


def cmd_update(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    fields: Dict[str, Any] = {}
    updates: Dict[str, Any] = {}
    if args.summary:
        fields["summary"] = args.summary
    description = args.description
    if args.description_file:
        description = load_text_file(args.description_file)
    if description is not None:
        fields["description"] = text_to_adf(description)
    if args.labels:
        fields["labels"] = split_csv(args.labels)
    if args.assignee:
        fields["assignee"] = {"id": args.assignee}
    if args.epic_key:
        if not args.epic_field:
            die("--epic-field is required when using --epic-key")
        fields[args.epic_field] = args.epic_key
    add_labels = split_csv(args.add_label)
    remove_labels = split_csv(args.remove_label)
    if add_labels or remove_labels:
        updates["labels"] = [{"add": l} for l in add_labels] + [{"remove": l} for l in remove_labels]
    apply_fields_json(fields, args.fields_json)
    body: Dict[str, Any] = {}
    if fields:
        body["fields"] = fields
    if updates:
        body["update"] = updates
    if not body:
        die("No updates specified")
    out = curl_json(args.base_url, args.email, args.token, "PUT", f"/rest/api/3/issue/{args.issue}", body=body, dry_run=args.dry_run)
    print_output(out, args.pretty)


def cmd_comment(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    body_text = args.body
    if args.body_file:
        body_text = load_text_file(args.body_file)
    if body_text is None:
        die("--body or --body-file is required")
    body = {"body": text_to_adf(body_text)}
    out = curl_json(args.base_url, args.email, args.token, "POST", f"/rest/api/3/issue/{args.issue}/comment", body=body, dry_run=args.dry_run)
    print_output(out, args.pretty)


def attach_files(issue: str, files: List[str], args: argparse.Namespace) -> None:
    base_url = build_base_url(args.base_url)
    url = f"{base_url}/rest/api/3/issue/{issue}/attachments"
    auth_email = args.email or "user@example.com"
    auth_token = args.token or "token"
    curl_args = ["curl", "-sS", "-X", "POST", "-u", f"{auth_email}:{auth_token}", "-H", "X-Atlassian-Token: no-check"]
    for path in files:
        path_obj = Path(path)
        if not path_obj.exists():
            die(f"Attachment not found: {path}")
        curl_args.extend(["-F", f"file=@{path}"])
    curl_args.append(url)
    out = run_curl(curl_args, args.dry_run)
    print_output(out, args.pretty)


def cmd_attach(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    if not args.file:
        die("--file is required")
    attach_files(args.issue, args.file, args)


def cmd_transitions(args: argparse.Namespace) -> None:
    require_auth(args.email, args.token, args.dry_run)
    if args.id:
        body = {"transition": {"id": args.id}}
        out = curl_json(
            args.base_url,
            args.email,
            args.token,
            "POST",
            f"/rest/api/3/issue/{args.issue}/transitions",
            body=body,
            dry_run=args.dry_run,
        )
        print_output(out, args.pretty)
        return
    out = curl_json(
        args.base_url,
        args.email,
        args.token,
        "GET",
        f"/rest/api/3/issue/{args.issue}/transitions",
        dry_run=args.dry_run,
    )
    print_output(out, args.pretty)


def build_parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--base-url", default=find_env_value("JIRA_BASE_URL", DEFAULT_BASE_URL))
    common.add_argument("--email", default=find_env_value("JIRA_EMAIL"))
    common.add_argument("--token", default=find_env_value("JIRA_API_TOKEN"))
    common.add_argument("--dry-run", action="store_true")
    common.add_argument("--pretty", action="store_true")

    parser = argparse.ArgumentParser(description="Jira ENG Board helper CLI")
    sub = parser.add_subparsers(dest="command", required=True)

    projects = sub.add_parser("projects", help="List projects", parents=[common])
    projects.set_defaults(func=cmd_projects)

    fields = sub.add_parser("fields", help="List fields", parents=[common])
    fields.add_argument("--name", help="Filter by name (substring)")
    fields.add_argument("--exact", action="store_true", help="Match name exactly")
    fields.set_defaults(func=cmd_fields)

    search = sub.add_parser("search", help="Search issues with JQL", parents=[common])
    search.add_argument("--jql", required=True)
    search.add_argument("--max-results", type=int, default=50)
    search.add_argument("--fields", help="Comma-separated field list")
    search.set_defaults(func=cmd_search)

    get = sub.add_parser("get", help="Get issue by key", parents=[common])
    get.add_argument("issue")
    get.add_argument("--fields", help="Comma-separated field list")
    get.set_defaults(func=cmd_get)

    create = sub.add_parser("create", help="Create issue", parents=[common])
    create.add_argument("--project", default="ENG")
    create.add_argument("--type", default="Task")
    create.add_argument("--summary")
    create.add_argument("--description")
    create.add_argument("--description-file")
    create.add_argument("--labels", help="Comma-separated labels")
    create.add_argument("--assignee", help="AccountId of assignee")
    create.add_argument("--parent", help="Parent issue key (sub-task)")
    create.add_argument("--epic-key", help="Epic key for linking")
    create.add_argument("--epic-field", help="Epic link field id (customfield_####)")
    create.add_argument("--fields-json", help="Extra fields JSON (string or @file)")
    create.add_argument("--attach", action="append", default=[], help="Attachment file path (repeatable)")
    create.set_defaults(func=cmd_create)

    update = sub.add_parser("update", help="Update issue", parents=[common])
    update.add_argument("issue")
    update.add_argument("--summary")
    update.add_argument("--description")
    update.add_argument("--description-file")
    update.add_argument("--labels", help="Comma-separated labels")
    update.add_argument("--add-label", help="Comma-separated labels to add")
    update.add_argument("--remove-label", help="Comma-separated labels to remove")
    update.add_argument("--assignee", help="AccountId of assignee")
    update.add_argument("--epic-key", help="Epic key for linking")
    update.add_argument("--epic-field", help="Epic link field id (customfield_####)")
    update.add_argument("--fields-json", help="Extra fields JSON (string or @file)")
    update.set_defaults(func=cmd_update)

    comment = sub.add_parser("comment", help="Add comment", parents=[common])
    comment.add_argument("issue")
    comment.add_argument("--body")
    comment.add_argument("--body-file")
    comment.set_defaults(func=cmd_comment)

    attach = sub.add_parser("attach", help="Attach files", parents=[common])
    attach.add_argument("issue")
    attach.add_argument("--file", action="append", default=[], help="Attachment file path (repeatable)")
    attach.set_defaults(func=cmd_attach)

    transitions = sub.add_parser("transition", help="List or apply transitions", parents=[common])
    transitions.add_argument("issue")
    transitions.add_argument("--id", help="Transition id to apply")
    transitions.set_defaults(func=cmd_transitions)

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.base_url = build_base_url(args.base_url)
    args.func(args)


if __name__ == "__main__":
    main()
