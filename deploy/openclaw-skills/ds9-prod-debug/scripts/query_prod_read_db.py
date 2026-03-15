#!/usr/bin/env python3
import json
import subprocess
import sys
from typing import Tuple

SUBSCRIPTION_ID = "30fa7a54-784e-4b28-9b96-84c4aa004d45"
RESOURCE_GROUP = "RG-prod"
KEY_VAULT = "KV-tribble-prod"
SECRET_NAME = "DATABASEURL-READONLY"
VM_NAME = "vm-prod-pg-tunnel"


def get_readonly_url() -> str:
    return subprocess.check_output(
        [
            "az",
            "keyvault",
            "secret",
            "show",
            "--vault-name",
            KEY_VAULT,
            "--name",
            SECRET_NAME,
            "--query",
            "value",
            "-o",
            "tsv",
        ],
        text=True,
    ).strip()


def extract_message_sections(payload: dict) -> Tuple[str, str]:
    value = payload.get("value") or []
    if not value:
        return "", ""

    message = value[0].get("message") or ""
    if "[stdout]" not in message:
        return message.strip(), ""

    _, _, after_stdout = message.partition("[stdout]")
    stdout_part, sep, after_stderr = after_stdout.partition("[stderr]")
    stdout_text = stdout_part.strip()
    stderr_text = after_stderr.strip() if sep else ""
    return stdout_text, stderr_text


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: query_prod_read_db.py '<readonly sql>'", file=sys.stderr)
        return 2

    sql = sys.argv[1].strip()
    if not sql:
        print("sql must be non-empty", file=sys.stderr)
        return 2

    banned = ("insert ", "update ", "delete ", "alter ", "drop ", "truncate ", "grant ", "revoke ")
    lowered = sql.lower()
    if any(token in lowered for token in banned):
        print("refusing non-readonly sql", file=sys.stderr)
        return 2

    subprocess.run(
        ["az", "account", "set", "--subscription", SUBSCRIPTION_ID],
        check=True,
        stdout=subprocess.DEVNULL,
    )

    readonly_url = get_readonly_url()
    script_lines = [
        "set -e",
        f"psql {json.dumps(readonly_url)} -v ON_ERROR_STOP=1 -P pager=off -c {json.dumps(sql)}",
    ]
    cmd = [
        "az",
        "vm",
        "run-command",
        "invoke",
        "-g",
        RESOURCE_GROUP,
        "-n",
        VM_NAME,
        "--command-id",
        "RunShellScript",
    ]
    for line in script_lines:
        cmd.extend(["--scripts", line])
    cmd.extend(["--output", "json"])

    res = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if res.returncode != 0:
        if res.stdout:
            print(res.stdout)
        if res.stderr:
            print(res.stderr, file=sys.stderr)
        return res.returncode

    try:
        payload = json.loads(res.stdout)
    except json.JSONDecodeError:
        if res.stdout:
            print(res.stdout)
        if res.stderr:
            print(res.stderr, file=sys.stderr)
        return 1

    stdout_text, stderr_text = extract_message_sections(payload)
    if stdout_text:
        print(stdout_text)
    if stderr_text:
        print(stderr_text, file=sys.stderr)
        return 1
    if res.stderr:
        print(res.stderr, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
