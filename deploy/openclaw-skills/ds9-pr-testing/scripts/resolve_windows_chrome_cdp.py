#!/usr/bin/env python3
import json
import subprocess
import sys
import urllib.request
from pathlib import Path
from typing import Optional


def host_ip() -> Optional[str]:
    try:
        output = subprocess.check_output(
            ["ip", "route", "show", "default"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
        for line in output.splitlines():
            parts = line.split()
            if len(parts) >= 3 and parts[0] == "default" and parts[1] == "via":
                return parts[2]
    except Exception:
        pass
    try:
        for line in Path("/etc/resolv.conf").read_text().splitlines():
            if line.startswith("nameserver "):
                return line.split()[1]
    except OSError:
        return None
    return None


def probe(endpoint: str) -> bool:
    url = endpoint.rstrip("/") + "/json/version"
    try:
        with urllib.request.urlopen(url, timeout=2) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            return bool(data.get("webSocketDebuggerUrl"))
    except Exception:
        return False


def main() -> int:
    candidates = []
    override = None
    try:
        override = Path("/tmp/linus_chrome_cdp_override.txt").read_text().strip()
    except OSError:
        override = None
    if override:
        candidates.append(override)
    candidates.extend(["http://127.0.0.1:9222", "http://127.0.0.1:9223"])
    ip = host_ip()
    if ip:
        candidates.extend([f"http://{ip}:9222", f"http://{ip}:9223"])

    for endpoint in candidates:
        if probe(endpoint):
            print(endpoint)
            return 0

    print("no-working-cdp-endpoint", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
