#!/usr/bin/env python3
import json
import subprocess
import sys

SUBSCRIPTION_ID = "30fa7a54-784e-4b28-9b96-84c4aa004d45"
APP_ID = "1fe2887e-c793-4533-b458-e6063bd394cb"
KEY_VAULT = "KV-tribble-prod"
READONLY_SECRET = "DATABASEURL-READONLY"
RESOURCE_GROUP = "RG-prod"
VM_NAME = "vm-prod-pg-tunnel"


def run(cmd):
    return subprocess.run(cmd, check=False, capture_output=True, text=True)


def main() -> int:
    subprocess.run(
        ["az", "account", "set", "--subscription", SUBSCRIPTION_ID],
        check=True,
        stdout=subprocess.DEVNULL,
    )

    checks = {}

    ai = run(
        [
            "az",
            "rest",
            "--method",
            "POST",
            "--url",
            f"https://api.applicationinsights.io/v1/apps/{APP_ID}/query",
            "--headers",
            "Content-Type=application/json",
            "--body",
            json.dumps({"query": "requests | where timestamp > ago(30m) | summarize count()"}),
            "--output",
            "json",
        ]
    )
    checks["app_insights"] = ai.returncode == 0

    kv = run(
        [
            "az",
            "keyvault",
            "secret",
            "show",
            "--vault-name",
            KEY_VAULT,
            "--name",
            READONLY_SECRET,
            "--query",
            "id",
            "-o",
            "tsv",
        ]
    )
    checks["key_vault"] = kv.returncode == 0

    db = run(
        [
            sys.executable,
            "/home/sunil/.local/share/linus/ds9-prod-debug/scripts/query_prod_read_db.py",
            "select count(*) from tribble.allowed_bot;",
        ]
    )
    checks["read_db"] = db.returncode == 0

    print(json.dumps(checks, indent=2))
    if not all(checks.values()):
        if ai.stderr:
            print(ai.stderr, file=sys.stderr)
        if kv.stderr:
            print(kv.stderr, file=sys.stderr)
        if db.stderr:
            print(db.stderr, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
