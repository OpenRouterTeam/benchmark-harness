"""HTTP front for the LiveMCPBench MCP servers.

Runs inside the upstream hysdhlx/livemcpbench image, connects every server in
tools/LiveMCPTool/all_config.json through the upstream MCPClient and exposes the
same two endpoints the MCP-Atlas sandbox has, so the harness can drive both
benchmarks through one client:

  POST /list-tools  -> [{name, description, inputSchema}]
  POST /call-tool   {tool_name, tool_args} -> [{type, text}]

Both endpoints require `Authorization: Bearer $SANDBOX_AUTH_TOKEN`. Tool names
are the upstream names; a name served by more than one server is exposed as
`<server>__<tool>` for each of them.
"""

import asyncio
import json
import logging
import os
import sys
from collections import defaultdict
from http import HTTPStatus

import uvicorn
from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import JSONResponse, PlainTextResponse

sys.path.insert(0, os.environ.get("LIVEMCPBENCH_ROOT", "/LiveMCPBench"))

from utils.mcp_client import MCPClient  # noqa: E402

CONFIG_PATH = os.environ.get(
    "LIVEMCPBENCH_CONFIG", "/LiveMCPBench/tools/LiveMCPTool/all_config.json"
)
PORT = int(os.environ.get("SANDBOX_PORT", "1985"))
TOOL_TIMEOUT_S = int(os.environ.get("SANDBOX_TOOL_TIMEOUT", "180"))
CONNECT_TIMEOUT_S = int(os.environ.get("SANDBOX_CONNECT_TIMEOUT", "120"))
CONNECT_CONCURRENCY = int(os.environ.get("SANDBOX_CONNECT_CONCURRENCY", "8"))
SEPARATOR = "__"

logger = logging.getLogger("livemcpbench-sandbox")


def load_server_configs(path):
    with open(path, encoding="utf-8") as handle:
        entries = json.load(handle)
    merged = {}
    for entry in entries:
        merged.update(entry["config"]["mcpServers"])
    return merged


def build_tool_index(tools_by_server):
    owners = defaultdict(list)
    for server_id, tools in tools_by_server.items():
        for tool in tools:
            owners[tool["name"]].append(server_id)
    index = {}
    for server_id, tools in tools_by_server.items():
        for tool in tools:
            exposed = (
                tool["name"]
                if len(owners[tool["name"]]) == 1
                else f"{server_id}{SEPARATOR}{tool['name']}"
            )
            index[exposed] = (server_id, tool)
    return index


def content_blocks(result):
    blocks = []
    for item in getattr(result, "content", []) or []:
        text = getattr(item, "text", None)
        blocks.append(
            {"type": getattr(item, "type", "text"), **({"text": text} if text is not None else {})}
        )
    if getattr(result, "isError", False) and not blocks:
        blocks.append({"type": "text", "text": "Error: tool call failed"})
    return blocks


class Sandbox:
    def __init__(self, token):
        self.token = token
        self.client = MCPClient(timeout=TOOL_TIMEOUT_S, max_sessions=9999)
        self.index = {}

    async def connect(self):
        servers = load_server_configs(CONFIG_PATH)
        gate = asyncio.Semaphore(CONNECT_CONCURRENCY)
        listed_by_server = await asyncio.gather(
            *(self.connect_one(gate, server_id, config) for server_id, config in servers.items())
        )
        tools_by_server = {
            server_id: tools for server_id, tools in listed_by_server if tools is not None
        }
        self.index = build_tool_index(tools_by_server)
        logger.info("%d servers, %d tools", len(tools_by_server), len(self.index))

    async def connect_one(self, gate, server_id, config):
        async with gate:
            try:
                listed = await asyncio.wait_for(self.list_server_tools(server_id, config), CONNECT_TIMEOUT_S)
            except Exception as error:  # noqa: BLE001 - one bad server must not sink the rest
                logger.warning("server %s skipped: %r", server_id, error)
                return server_id, None
        return server_id, listed

    async def list_server_tools(self, server_id, config):
        await self.client.config_connect({"mcpServers": {server_id: config}})
        listed = await self.client.list_tools(server_id)
        return [
            {
                "name": tool.name,
                "description": tool.description or "",
                "inputSchema": tool.inputSchema or {},
            }
            for tool in listed.values()
        ]

    def check_token(self, authorization):
        if authorization != f"Bearer {self.token}":
            raise HTTPException(status_code=HTTPStatus.UNAUTHORIZED, detail="bad token")

    def tool_list(self):
        return [
            {"name": name, "description": tool["description"], "inputSchema": tool["inputSchema"]}
            for name, (_, tool) in sorted(self.index.items())
        ]

    async def call(self, name, args):
        entry = self.index.get(name)
        if entry is None:
            return PlainTextResponse(f"unknown tool {name}", status_code=HTTPStatus.NOT_FOUND)
        server_id, tool = entry
        try:
            result = await asyncio.wait_for(
                self.client.tool_execute(server_id, tool["name"], args), TOOL_TIMEOUT_S
            )
        except asyncio.TimeoutError:
            return PlainTextResponse(f"{name} timed out", status_code=HTTPStatus.GATEWAY_TIMEOUT)
        except Exception as error:  # noqa: BLE001 - surfaced to the model as a tool error
            return PlainTextResponse(f"{name} failed: {error}", status_code=HTTPStatus.BAD_GATEWAY)
        return JSONResponse(content_blocks(result))


def make_app():
    token = os.environ.get("SANDBOX_AUTH_TOKEN")
    if not token:
        raise SystemExit("SANDBOX_AUTH_TOKEN is required")
    sandbox = Sandbox(token)
    app = FastAPI()

    @app.on_event("startup")
    async def connect():
        await sandbox.connect()

    @app.post("/list-tools")
    async def list_tools(authorization: str | None = Header(default=None)):
        sandbox.check_token(authorization)
        return sandbox.tool_list()

    @app.post("/call-tool")
    async def call_tool(request: Request, authorization: str | None = Header(default=None)):
        sandbox.check_token(authorization)
        body = await request.json()
        return await sandbox.call(body.get("tool_name"), body.get("tool_args") or {})

    return app


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    uvicorn.run(make_app(), host="0.0.0.0", port=PORT)
