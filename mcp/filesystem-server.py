#!/usr/bin/env python3
"""Filesystem MCP Server for AetherVault.

Provides file operations restricted to allowed directories.
Uses the MCP Python SDK with FastMCP for stdio transport.
"""

import os
import fnmatch
from pathlib import Path
from mcp.server import FastMCP

ALLOWED_ROOTS = [
    Path("/root/.aethervault/workspace"),
    Path("/tmp"),
]

mcp = FastMCP("aethervault-filesystem", log_level="WARNING")


def _validate_path(path_str: str) -> Path:
    """Resolve and validate a path is within allowed roots."""
    p = Path(path_str).resolve()
    for root in ALLOWED_ROOTS:
        try:
            p.relative_to(root.resolve())
            return p
        except ValueError:
            continue
    allowed = ", ".join(str(r) for r in ALLOWED_ROOTS)
    raise ValueError(f"Access denied: {path_str} is outside allowed directories ({allowed})")


@mcp.tool()
def read_file(path: str) -> str:
    """Read the contents of a file. Path must be within /root/.aethervault/workspace or /tmp."""
    p = _validate_path(path)
    if not p.exists():
        return f"Error: File not found: {path}"
    if not p.is_file():
        return f"Error: Not a file: {path}"
    try:
        return p.read_text(encoding="utf-8")
    except Exception as e:
        return f"Error reading file: {e}"


@mcp.tool()
def write_file(path: str, content: str) -> str:
    """Write content to a file. Path must be within /root/.aethervault/workspace or /tmp."""
    p = _validate_path(path)
    try:
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")
        return f"Successfully wrote {len(content)} bytes to {path}"
    except Exception as e:
        return f"Error writing file: {e}"


@mcp.tool()
def list_directory(path: str) -> str:
    """List contents of a directory. Path must be within /root/.aethervault/workspace or /tmp."""
    p = _validate_path(path)
    if not p.exists():
        return f"Error: Directory not found: {path}"
    if not p.is_dir():
        return f"Error: Not a directory: {path}"
    entries = []
    try:
        for item in sorted(p.iterdir()):
            kind = "dir" if item.is_dir() else "file"
            size = item.stat().st_size if item.is_file() else 0
            suffix = f"  ({size} bytes)" if kind == "file" else ""
            entries.append(f"  [{kind}] {item.name}{suffix}")
        if not entries:
            return f"Directory {path} is empty"
        return f"Contents of {path}:\n" + "\n".join(entries)
    except Exception as e:
        return f"Error listing directory: {e}"


@mcp.tool()
def search_files(directory: str, pattern: str) -> str:
    """Search for files matching a glob pattern within a directory.

    Args:
        directory: Root directory to search in (must be within allowed paths)
        pattern: Glob pattern to match (e.g. *.py, *.md, README*)
    """
    p = _validate_path(directory)
    if not p.exists() or not p.is_dir():
        return f"Error: Invalid directory: {directory}"
    matches = []
    try:
        for root_dir, dirs, files in os.walk(p):
            for fname in files:
                if fnmatch.fnmatch(fname, pattern):
                    full = os.path.join(root_dir, fname)
                    matches.append(full)
        if not matches:
            return f"No files matching '{pattern}' found in {directory}"
        result = f"Found {len(matches)} file(s) matching '{pattern}':\n"
        result += "\n".join(f"  {m}" for m in sorted(matches))
        return result
    except Exception as e:
        return f"Error searching files: {e}"


if __name__ == "__main__":
    import asyncio
    asyncio.run(mcp.run_stdio_async())
