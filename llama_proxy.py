#!/usr/bin/env python3
"""HTTP Proxy for llama.cpp via SSH to Windows PC"""
import shlex
import subprocess
import json
import os
import uuid
from http.server import HTTPServer, BaseHTTPRequestHandler

# Configurable via environment variables
LLAMA_PORT = int(os.environ.get("LLAMA_PROXY_PORT", "11434"))
LLAMA_BACKEND_PORT = int(os.environ.get("LLAMA_BACKEND_PORT", "11434"))
LLAMA_SSH_PORT = os.environ.get("LLAMA_SSH_PORT", "2222")
LLAMA_SSH_USER = os.environ.get("LLAMA_SSH_USER", "user")
LLAMA_SSH_HOST = os.environ.get("LLAMA_SSH_HOST", "localhost")
LLAMA_BACKEND_HOST = os.environ.get("LLAMA_BACKEND_HOST", "127.0.0.1")

class LlamaProxy(BaseHTTPRequestHandler):
    def forward_request(self, method, body=None):
        # Sanitize path to prevent command injection
        path = self.path
        if not path.startswith("/"):
            path = "/" + path
        # Only allow URL-safe characters in path
        import re
        if not re.match(r'^/[a-zA-Z0-9/_\-%.?&=]*$', path):
            self.send_response(400)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"error": "Invalid path"}).encode())
            return

        backend_url = f"http://{LLAMA_BACKEND_HOST}:{LLAMA_BACKEND_PORT}{path}"

        if body:
            # Write body to a file that Windows can access via WSL
            req_id = str(uuid.uuid4())[:8]
            local_file = f"/tmp/req_{req_id}.json"
            with open(local_file, 'w') as f:
                f.write(body)

            # Copy to WSL's /tmp
            copy_cmd = [
                "scp", "-o", "StrictHostKeyChecking=no", "-P", LLAMA_SSH_PORT,
                local_file, f"{LLAMA_SSH_USER}@{LLAMA_SSH_HOST}:/tmp/"
            ]
            subprocess.run(copy_cmd, capture_output=True, timeout=30)
            os.unlink(local_file)

            # Windows can access WSL files via \\wsl.localhost\Ubuntu
            # Use cmd.exe to run curl with the WSL file path
            wsl_file = f"\\\\\\\\wsl.localhost\\\\Ubuntu\\\\tmp\\\\req_{req_id}.json"
            curl_cmd = (
                f'/mnt/c/Windows/System32/cmd.exe /c '
                f'"curl -s -X {shlex.quote(method)} -H \\"Content-Type: application/json\\" '
                f'-d @{wsl_file} {shlex.quote(backend_url)}"'
            )
        else:
            # Use cmd.exe to call curl on Windows
            curl_cmd = (
                f'/mnt/c/Windows/System32/cmd.exe /c '
                f'"curl -s -X {shlex.quote(method)} {shlex.quote(backend_url)}"'
            )

        ssh_cmd = [
            "ssh", "-o", "StrictHostKeyChecking=no", "-p", LLAMA_SSH_PORT,
            f"{LLAMA_SSH_USER}@{LLAMA_SSH_HOST}", curl_cmd
        ]

        try:
            result = subprocess.run(ssh_cmd, capture_output=True, text=True, timeout=600)
            response = result.stdout.strip()

            # Clean up temp file on remote
            if body:
                cleanup_cmd = ["ssh", "-o", "StrictHostKeyChecking=no", "-p", LLAMA_SSH_PORT,
                              f"{LLAMA_SSH_USER}@{LLAMA_SSH_HOST}", f"rm -f /tmp/req_{req_id}.json"]
                subprocess.run(cleanup_cmd, capture_output=True, timeout=10)

            if result.returncode != 0 or not response:
                self.send_response(502)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                err = result.stderr.strip() if result.stderr else "empty response from backend"
                self.wfile.write(json.dumps({"error": f"Backend error: {err}"}).encode())
                return

            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(response.encode())
        except subprocess.TimeoutExpired:
            self.send_response(504)
            self.end_headers()
            self.wfile.write(json.dumps({"error": "Gateway timeout"}).encode())
        except Exception as e:
            self.send_response(500)
            self.end_headers()
            self.wfile.write(json.dumps({"error": str(e)}).encode())

    def do_GET(self):
        self.forward_request("GET")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length).decode() if length > 0 else None
        self.forward_request("POST", body)

    def log_message(self, format, *args):
        print(f"[{args[1]}] {args[0]}")

if __name__ == "__main__":
    bind = os.environ.get("LLAMA_BIND_ADDR", "0.0.0.0")
    print(f"Llama proxy v2 on {bind}:{LLAMA_PORT}")
    HTTPServer((bind, LLAMA_PORT), LlamaProxy).serve_forever()
