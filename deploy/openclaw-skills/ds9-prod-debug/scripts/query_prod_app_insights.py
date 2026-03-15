#!/usr/bin/env python3
import json
import subprocess
import sys

SUBSCRIPTION_ID = "30fa7a54-784e-4b28-9b96-84c4aa004d45"
APP_ID = "1fe2887e-c793-4533-b458-e6063bd394cb"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: query_prod_app_insights.py '<kql query>'", file=sys.stderr)
        return 2

    query = sys.argv[1]
    subprocess.run(
        ["az", "account", "set", "--subscription", SUBSCRIPTION_ID],
        check=True,
        stdout=subprocess.DEVNULL,
    )

    body = json.dumps({"query": query})
    res = subprocess.run(
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
            body,
            "--output",
            "json",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if res.stdout:
        print(res.stdout)
    if res.stderr:
        print(res.stderr, file=sys.stderr)
    return res.returncode


if __name__ == "__main__":
    raise SystemExit(main())
