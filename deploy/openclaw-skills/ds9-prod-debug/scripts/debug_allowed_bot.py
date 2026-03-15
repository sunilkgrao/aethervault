#!/usr/bin/env python3
import json
import subprocess
import sys

BASE = "/home/sunil/.local/share/linus/ds9-prod-debug/scripts"


def run(cmd):
    return subprocess.run(cmd, check=False, capture_output=True, text=True)


def main() -> int:
    if len(sys.argv) not in (1, 2, 3):
        print("usage: debug_allowed_bot.py [slack_user_id] [slack_team_id]", file=sys.stderr)
        return 2

    slack_user_id = sys.argv[1] if len(sys.argv) >= 2 else ""
    slack_team_id = sys.argv[2] if len(sys.argv) >= 3 else ""

    where = []
    if slack_user_id:
        where.append(f"slack_user_id = '{slack_user_id}'")
    if slack_team_id:
        where.append(f"slack_team_id = '{slack_team_id}'")

    sql = (
        "select id, tribble_user_id, tribble_client_id, slack_bot_id, "
        "slack_user_id, slack_team_id, created_at from tribble.allowed_bot"
    )
    if where:
        sql += " where " + " and ".join(where)
    sql += " order by created_at desc limit 20;"

    db = run([f"{BASE}/query_prod_read_db.py", sql])
    ai = run(
        [
            f"{BASE}/query_prod_app_insights.py",
            "traces | where timestamp > ago(2h) and message has 'findAllowedBot' | order by timestamp desc | take 20",
        ]
    )

    print(json.dumps({"db_rc": db.returncode, "ai_rc": ai.returncode}, indent=2))
    print("\n== allowed_bot rows ==\n")
    if db.stdout:
        print(db.stdout.strip())
    if db.stderr:
        print(db.stderr.strip(), file=sys.stderr)

    print("\n== recent App Insights traces for findAllowedBot ==\n")
    if ai.stdout:
        print(ai.stdout.strip())
    if ai.stderr:
        print(ai.stderr.strip(), file=sys.stderr)

    return 0 if db.returncode == 0 and ai.returncode == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
