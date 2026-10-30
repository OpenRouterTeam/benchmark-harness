# LiveMCPBench sandbox: the upstream image (Apache-2.0, icip-cas/LiveMCPBench)
# with its 69 npx/uvx MCP servers, fronted by the /list-tools + /call-tool shim.
FROM hysdhlx/livemcpbench:latest

WORKDIR /LiveMCPBench
COPY server.py /LiveMCPBench/sandbox_server.py

ENV SANDBOX_PORT=1985
EXPOSE 1985
CMD ["uv", "run", "python", "/LiveMCPBench/sandbox_server.py"]
