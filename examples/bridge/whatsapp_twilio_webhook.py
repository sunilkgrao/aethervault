#!/usr/bin/env python3
"""Twilio WhatsApp webhook bridge (stdlib-only)."""
import os
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

from agent_runner import run_with_subagents

HOST = os.environ.get("WHATSAPP_BIND", "0.0.0.0")
PORT = int(os.environ.get("WHATSAPP_PORT", "8080"))


def xml_escape(text):
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
    )


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def do_POST(self):  # noqa: N802
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length).decode("utf-8")
        params = urllib.parse.parse_qs(body)
        message = (params.get("Body") or [""])[0]
        sender = (params.get("From") or [""])[0]

        if not message:
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"<Response></Response>")
            return

        session = f"whatsapp:{sender}"
        response, _ = run_with_subagents(message, session)
        reply = xml_escape(response)

        twiml = f"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response><Message>{reply}</Message></Response>"
        self.send_response(200)
        self.send_header("Content-Type", "application/xml")
        self.end_headers()
        self.wfile.write(twiml.encode("utf-8"))


if __name__ == "__main__":
    server = HTTPServer((HOST, PORT), Handler)
    print(f"WhatsApp webhook listening on http://{HOST}:{PORT}")
    server.serve_forever()
