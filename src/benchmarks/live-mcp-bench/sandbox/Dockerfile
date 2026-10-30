# LiveMCPBench sandbox: the upstream runtime image (Apache-2.0, icip-cas/LiveMCPBench)
# carries uv, node and the project venv but not the repository, so the repository
# (MCPClient, the 69 npx/uvx server configs) is checked out on top of it, fronted
# by the /list-tools + /call-tool shim.
FROM hysdhlx/livemcpbench:latest
ARG LIVEMCPBENCH_COMMIT=36a51a1065dda49c1e503b87edf33a1e0b331a74
WORKDIR /LiveMCPBench
RUN git clone -q --filter=blob:none https://github.com/icip-cas/LiveMCPBench.git /tmp/upstream \
  && git -C /tmp/upstream checkout -q "${LIVEMCPBENCH_COMMIT}" \
  && rm -rf /tmp/upstream/.git \
  && cp -a /tmp/upstream/. /LiveMCPBench/ \
  && rm -rf /tmp/upstream
COPY server.py /LiveMCPBench/sandbox_server.py
# MCPClient spawns servers with a minimal env (PATH but no UV_* vars), npx lives
# under nvm, and the uvx servers predate mcp 2.x, so uv/uvx wrappers pin mcp<2.
RUN mkdir -p /opt/sandbox-bin \
  && ln -s /root/.nvm/versions/node/*/bin/node /root/.nvm/versions/node/*/bin/npm /root/.nvm/versions/node/*/bin/npx /opt/sandbox-bin/ \
  && printf 'mcp<2\n' > /etc/uv-constraints.txt \
  && printf '#!/bin/sh\nUV_CONSTRAINT=/etc/uv-constraints.txt exec /root/.local/bin/uv "$@"\n' > /opt/sandbox-bin/uv \
  && printf '#!/bin/sh\nUV_CONSTRAINT=/etc/uv-constraints.txt exec /root/.local/bin/uvx "$@"\n' > /opt/sandbox-bin/uvx \
  && chmod +x /opt/sandbox-bin/uv /opt/sandbox-bin/uvx
ENV PATH="/opt/sandbox-bin:/root/.local/bin:/LiveMCPBench/.venv/bin:${PATH}"
ENV SANDBOX_PORT=1985
EXPOSE 1985
CMD ["/LiveMCPBench/.venv/bin/python", "/LiveMCPBench/sandbox_server.py"]
