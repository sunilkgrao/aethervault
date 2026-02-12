#!/usr/bin/env python3
"""Proxy for Moonshot API that rewrites model names to kimi-k2.5"""
import json
import os
import urllib.request
import urllib.error
from http.server import HTTPServer, BaseHTTPRequestHandler

MOONSHOT_API = os.environ.get("MOONSHOT_API_URL", "https://api.moonshot.ai/v1")
API_KEY = os.environ.get("MOONSHOT_API_KEY", "")
TARGET_MODEL = os.environ.get("MOONSHOT_MODEL", "kimi-k2.5")
MOONSHOT_PORT = int(os.environ.get("MOONSHOT_PROXY_PORT", "11437"))
MOONSHOT_BIND = os.environ.get("MOONSHOT_BIND_ADDR", "127.0.0.1")

if not API_KEY:
    print("WARNING: MOONSHOT_API_KEY environment variable not set")

class MoonshotProxy(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/v1/models":
            models = {
                "object": "list",
                "data": [
                    {"id": "gpt-4o", "object": "model", "owned_by": "moonshot"},
                    {"id": "gpt-4", "object": "model", "owned_by": "moonshot"},
                ]
            }
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(models).encode())
        else:
            self.forward_request("GET")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length).decode() if length > 0 else "{}"
        try:
            data = json.loads(body)
            data["model"] = TARGET_MODEL
            body = json.dumps(data)
        except:
            pass
        self.forward_request("POST", body)

    def forward_request(self, method, body=None):
        url = MOONSHOT_API + self.path
        headers = {
            "Authorization": f"Bearer {API_KEY}",
            "Content-Type": "application/json"
        }
        try:
            req = urllib.request.Request(url, data=body.encode() if body else None, headers=headers, method=method)
            with urllib.request.urlopen(req, timeout=300) as resp:
                response_body = resp.read()
                self.send_response(resp.status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(response_body)
        except urllib.error.HTTPError as e:
            self.send_response(e.code)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(e.read())
        except Exception as e:
            self.send_response(500)
            self.end_headers()
            self.wfile.write(json.dumps({"error": str(e)}).encode())

    def log_message(self, format, *args):
        print(f"[{args[1]}] {args[0]}")

if __name__ == "__main__":
    print(f"Moonshot proxy on {MOONSHOT_BIND}:{MOONSHOT_PORT} -> {TARGET_MODEL}")
    HTTPServer((MOONSHOT_BIND, MOONSHOT_PORT), MoonshotProxy).serve_forever()
