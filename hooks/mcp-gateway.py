#!/usr/bin/env python3
"""MCP Gateway CLI for AetherVault.

Unified command-line interface for managing and invoking MCP servers.
This is the primary interface for AetherVault's agent to call MCP tools.

Usage:
    python3 mcp-gateway.py list-servers          List registered MCP servers
    python3 mcp-gateway.py list-tools             List all tools across all servers
    python3 mcp-gateway.py call <server> <tool> '<args-json>'   Invoke a tool
"""

import asyncio
import json
import sys
from pathlib import Path

from mcp import ClientSession
from mcp.client.stdio import stdio_client, StdioServerParameters

CONFIG_PATH = Path("/root/.aethervault/config/mcp-servers.json")

# Timeouts for MCP server operations (seconds)
MCP_INIT_TIMEOUT = 15       # Time to wait for server initialization
MCP_TOOL_CALL_TIMEOUT = 60  # Time to wait for a tool call response
MCP_LIST_TIMEOUT = 10       # Time to wait for listing tools


def load_config() -> dict:
    """Load the MCP server registry."""
    if not CONFIG_PATH.exists():
        print(f"Error: Config not found at {CONFIG_PATH}", file=sys.stderr)
        sys.exit(1)
    with open(CONFIG_PATH) as f:
        return json.load(f)


def list_servers():
    """List all registered MCP servers."""
    config = load_config()
    servers = config.get("servers", {})
    if not servers:
        print("No servers registered.")
        return

    print(f"Registered MCP servers ({len(servers)}):\n")
    for name, info in servers.items():
        enabled = info.get("enabled", False)
        status = "ENABLED" if enabled else "DISABLED"
        desc = info.get("description", "No description")
        note = info.get("note", "")
        print(f"  [{status}] {name}")
        print(f"    Description: {desc}")
        if note:
            print(f"    Note: {note}")
        print()


async def _connect_and_list_tools(name: str, info: dict) -> list[dict]:
    """Connect to a server and list its tools (with timeouts)."""
    server_params = StdioServerParameters(
        command=info["command"],
        args=info.get("args", []),
    )
    tools = []
    async with stdio_client(server_params) as (read, write):
        async with ClientSession(read, write) as session:
            await asyncio.wait_for(session.initialize(), timeout=MCP_INIT_TIMEOUT)
            result = await asyncio.wait_for(session.list_tools(), timeout=MCP_LIST_TIMEOUT)
            for tool in result.tools:
                tools.append({
                    "server": name,
                    "name": tool.name,
                    "description": tool.description or "No description",
                })
    return tools


async def list_tools_async():
    """List all tools from all enabled servers."""
    config = load_config()
    servers = config.get("servers", {})
    enabled = {k: v for k, v in servers.items() if v.get("enabled", False)}

    if not enabled:
        print("No enabled servers.")
        return

    all_tools = []
    for name, info in enabled.items():
        try:
            tools = await _connect_and_list_tools(name, info)
            all_tools.extend(tools)
        except Exception as e:
            print(f"  Error connecting to {name}: {e}", file=sys.stderr)

    if not all_tools:
        print("No tools available.")
        return

    print(f"Available MCP tools ({len(all_tools)}):\n")
    current_server = None
    for tool in all_tools:
        if tool["server"] != current_server:
            current_server = tool["server"]
            print(f"  [{current_server}]")
        print(f"    {tool['name']}: {tool['description']}")
    print()


async def call_tool_async(server_name: str, tool_name: str, args_json: str):
    """Call a specific tool on a specific server."""
    config = load_config()
    servers = config.get("servers", {})

    if server_name not in servers:
        print(f"Error: Server '{server_name}' not found. Available: {', '.join(servers.keys())}", file=sys.stderr)
        sys.exit(1)

    info = servers[server_name]
    if not info.get("enabled", False):
        print(f"Error: Server '{server_name}' is disabled.", file=sys.stderr)
        sys.exit(1)

    # Parse args
    try:
        args = json.loads(args_json) if args_json else {}
    except json.JSONDecodeError as e:
        print(f"Error: Invalid JSON arguments: {e}", file=sys.stderr)
        sys.exit(1)

    server_params = StdioServerParameters(
        command=info["command"],
        args=info.get("args", []),
    )

    exit_code = 0
    try:
        async with stdio_client(server_params) as (read, write):
            async with ClientSession(read, write) as session:
                await asyncio.wait_for(session.initialize(), timeout=MCP_INIT_TIMEOUT)
                result = await asyncio.wait_for(
                    session.call_tool(tool_name, args),
                    timeout=MCP_TOOL_CALL_TIMEOUT,
                )

                # Format output
                for content in result.content:
                    if hasattr(content, "text"):
                        print(content.text)
                    else:
                        print(str(content))

                if result.isError:
                    exit_code = 1
    except BaseExceptionGroup as eg:
        # Extract the actual error message from exception groups
        for exc in eg.exceptions:
            if isinstance(exc, BaseExceptionGroup):
                for inner in exc.exceptions:
                    if not isinstance(inner, SystemExit):
                        print(f"Error: {inner}", file=sys.stderr)
            elif not isinstance(exc, SystemExit):
                print(f"Error: {exc}", file=sys.stderr)
        exit_code = 1
    except Exception as e:
        print(f"Error calling {server_name}.{tool_name}: {e}", file=sys.stderr)
        exit_code = 1

    if exit_code:
        sys.exit(exit_code)


def usage():
    print("MCP Gateway - AetherVault Tool Interface")
    print()
    print("Usage:")
    print("  python3 mcp-gateway.py list-servers                    List registered servers")
    print("  python3 mcp-gateway.py list-tools                      List all available tools")
    print("  python3 mcp-gateway.py call <server> <tool> '<json>'   Call a tool")
    print()
    print("Examples:")
    print('  python3 mcp-gateway.py call filesystem read_file \'{"path": "/root/.aethervault/workspace/SOUL.md"}\'')
    print('  python3 mcp-gateway.py call system get_system_info \'{}\'')
    print('  python3 mcp-gateway.py call system get_service_status \'{"service_name": "aethervault"}\'')
    print('  python3 mcp-gateway.py call filesystem list_directory \'{"path": "/root/.aethervault/workspace"}\'')


def main():
    if len(sys.argv) < 2:
        usage()
        sys.exit(1)

    command = sys.argv[1]

    if command == "list-servers":
        list_servers()
    elif command == "list-tools":
        asyncio.run(list_tools_async())
    elif command == "call":
        if len(sys.argv) < 4:
            print("Error: 'call' requires: <server> <tool> [args-json]", file=sys.stderr)
            sys.exit(1)
        server = sys.argv[2]
        tool = sys.argv[3]
        args_json = sys.argv[4] if len(sys.argv) > 4 else "{}"
        asyncio.run(call_tool_async(server, tool, args_json))
    elif command in ("help", "--help", "-h"):
        usage()
    else:
        print(f"Unknown command: {command}", file=sys.stderr)
        usage()
        sys.exit(1)


if __name__ == "__main__":
    main()
