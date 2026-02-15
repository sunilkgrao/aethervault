#!/usr/bin/env python3
"""Grok API search tool for AetherVault — Twitter/X and web search via xAI.

Usage:
  python3 grok-search.py twitter "query"              # Search Twitter/X
  python3 grok-search.py web "query"                   # Search the web
  python3 grok-search.py both "query"                  # Search both
  python3 grok-search.py twitter "query" --handles elonmusk,OpenAI
  python3 grok-search.py twitter "query" --from 2026-02-01 --to 2026-02-09
"""
import sys, json, os, requests

# Auto-load .env file if vars not in environment
ENV_FILE = "/root/.aethervault/.env"
if not os.environ.get("XAI_API_KEY") and not os.environ.get("GROK_API_KEY"):
    if os.path.exists(ENV_FILE):
        with open(ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    key, _, value = line.partition("=")
                    os.environ.setdefault(key.strip(), value.strip())

API_KEY = os.environ.get("XAI_API_KEY") or os.environ.get("GROK_API_KEY")
RESPONSES_URL = "https://api.x.ai/v1/responses"
CHAT_URL = "https://api.x.ai/v1/chat/completions"
MODEL = "grok-4-1-fast-non-reasoning"


def search_responses_api(mode, query, handles=None, from_date=None, to_date=None):
    """Try Responses API first (supports x_search)."""
    tools = []
    if mode in ("twitter", "both"):
        tool = {"type": "x_search"}
        if handles:
            tool["allowed_x_handles"] = handles
        if from_date:
            tool["from_date"] = from_date
        if to_date:
            tool["to_date"] = to_date
        tools.append(tool)
    if mode in ("web", "both"):
        tools.append({"type": "web_search"})

    payload = {
        "model": MODEL,
        "input": [{"role": "user", "content": query}],
        "tools": tools,
        "inline_citations": True
    }

    resp = requests.post(
        RESPONSES_URL,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {API_KEY}",
        },
        json=payload,
        timeout=60
    )
    resp.raise_for_status()
    result = resp.json()

    # Extract text from Responses API output
    texts = []
    for item in result.get("output", []):
        if item.get("type") == "message" and item.get("role") == "assistant":
            for content in item.get("content", []):
                if content.get("type") == "output_text":
                    texts.append(content["text"])

    if texts:
        return "\n\n".join(texts)
    return json.dumps(result, indent=2)[:5000]


def search_chat_api(mode, query, handles=None, from_date=None, to_date=None):
    """Fallback: Chat Completions API with search_parameters."""
    search_params = {}
    if mode in ("twitter", "both"):
        search_params["sources"] = [{"type": "x"}]
    if mode in ("web", "both"):
        search_params.setdefault("sources", []).append({"type": "web"})
    if from_date:
        search_params["from_date"] = from_date
    if to_date:
        search_params["to_date"] = to_date

    payload = {
        "model": MODEL,
        "messages": [{"role": "user", "content": query}],
        "search_parameters": search_params if search_params else {"mode": "auto"}
    }

    resp = requests.post(
        CHAT_URL,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {API_KEY}",
        },
        json=payload,
        timeout=60
    )
    resp.raise_for_status()
    result = resp.json()

    # Extract from chat completions response
    choices = result.get("choices", [])
    if choices:
        msg = choices[0].get("message", {})
        return msg.get("content", json.dumps(result, indent=2)[:5000])
    return json.dumps(result, indent=2)[:5000]


def search(mode, query, handles=None, from_date=None, to_date=None):
    if not API_KEY:
        return "ERROR: No XAI_API_KEY or GROK_API_KEY found in environment"

    # Try Responses API first, fall back to Chat Completions
    try:
        return search_responses_api(mode, query, handles, from_date, to_date)
    except requests.exceptions.HTTPError as e:
        err_detail = ""
        try:
            err_detail = e.response.text[:300]
        except Exception:
            pass
        # If Responses API fails, try Chat Completions
        try:
            return search_chat_api(mode, query, handles, from_date, to_date)
        except requests.exceptions.HTTPError as e2:
            err2 = ""
            try:
                err2 = e2.response.text[:300]
            except Exception:
                pass
            return f"ERROR: Both APIs failed.\nResponses API: {e} {err_detail}\nChat API: {e2} {err2}"
    except Exception as e:
        return f"ERROR: {e}"


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)

    mode = sys.argv[1]
    if mode not in ("twitter", "web", "both"):
        print(f"ERROR: Invalid mode '{mode}'. Use: twitter, web, both")
        sys.exit(1)

    query = sys.argv[2]
    handles = None
    from_date = None
    to_date = None

    i = 3
    while i < len(sys.argv):
        if sys.argv[i] == "--handles" and i + 1 < len(sys.argv):
            handles = sys.argv[i + 1].split(",")
            i += 2
        elif sys.argv[i] == "--from" and i + 1 < len(sys.argv):
            from_date = sys.argv[i + 1]
            i += 2
        elif sys.argv[i] == "--to" and i + 1 < len(sys.argv):
            to_date = sys.argv[i + 1]
            i += 2
        else:
            i += 1

    print(search(mode, query, handles, from_date, to_date))


if __name__ == "__main__":
    main()
