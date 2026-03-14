#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


REQUIRED_ENV = [
    "SUBSCRIPTION_ID",
    "RESOURCE_GROUP",
]


def load_env(env_path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for raw in env_path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        env[key] = value.strip().strip('"').strip("'")
    return env


def run(*args: str, capture: bool = False) -> str:
    if capture:
        return subprocess.check_output(args, text=True).strip()
    subprocess.check_call(args)
    return ""


def run_json(*args: str) -> object:
    return json.loads(subprocess.check_output(args, text=True))


def ensure_required(env: dict[str, str]) -> None:
    missing = [k for k in REQUIRED_ENV if not env.get(k)]
    if missing:
        raise SystemExit(f"missing required env keys: {', '.join(missing)}")


def ensure_login() -> str:
    user = run("az", "account", "show", "--query", "user.name", "-o", "tsv", capture=True)
    if not user.endswith("tribble.ai"):
        raise SystemExit("Azure login is not a tribble.ai account")
    return user


def main() -> int:
    repo_root = Path.cwd()
    env_path = repo_root / ".env"
    if not env_path.exists():
        raise SystemExit(f".env not found at {env_path}")

    env = load_env(env_path)
    ensure_required(env)
    user = ensure_login()
    subscription_id = env["SUBSCRIPTION_ID"]
    resource_group = env["RESOURCE_GROUP"]
    ip_address = run("curl", "-s", "ifconfig.me", capture=True)
    username = os.environ.get("USER", "sunil")

    run("az", "account", "set", "--subscription", subscription_id)

    print(f"user={user}")
    print(f"ip={ip_address}")

    existing_rule = run(
        "az",
        "postgres",
        "flexible-server",
        "firewall-rule",
        "list",
        "-g",
        resource_group,
        "-n",
        "tribble-test",
        "--query",
        f"[?startIpAddress=='{ip_address}' && endIpAddress=='{ip_address}'].name",
        "-o",
        "tsv",
        capture=True,
    )
    if existing_rule:
        print(f"postgres_test=already_present:{existing_rule}")
    else:
        run(
            "az",
            "postgres",
            "flexible-server",
            "firewall-rule",
            "create",
            "-g",
            resource_group,
            "-n",
            "tribble-test",
            "--rule-name",
            username,
            "--start-ip-address",
            ip_address,
            "--end-ip-address",
            ip_address,
            "--output",
            "none",
        )
        print("postgres_test=created")

    run(
        "az",
        "keyvault",
        "network-rule",
        "add",
        "--ip-address",
        f"{ip_address}/32",
        "--name",
        "KV-tribble-test",
        "--resource-group",
        resource_group,
        "--output",
        "none",
    )
    print("keyvault_test=ok")

    ssh_rule = run_json(
        "az",
        "network",
        "nsg",
        "rule",
        "show",
        "-g",
        "RG-test",
        "--nsg-name",
        "vm-test-airbyte-nsg",
        "-n",
        "SSH",
        "-o",
        "json",
    )
    current_prefixes = ssh_rule.get("sourceAddressPrefixes") or []
    if isinstance(current_prefixes, str):
        current_prefixes = [current_prefixes]
    if ip_address in current_prefixes:
        print("airbyte=already_present")
    else:
        updated = current_prefixes + [ip_address]
        run(
            "az",
            "network",
            "nsg",
            "rule",
            "update",
            "-g",
            "RG-test",
            "--nsg-name",
            "vm-test-airbyte-nsg",
            "-n",
            "SSH",
            "--access",
            "Allow",
            "--source-address-prefixes",
            *updated,
            "--output",
            "none",
        )
        print("airbyte=updated")

    print("cognitiveservices=skipped")
    print(
        "note=repo setupNetworkRulesDev.sh currently misparses .env and uses a stale Cognitive Services API path; "
        "this helper covers the needed DS9 local-dev test resources safely"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
