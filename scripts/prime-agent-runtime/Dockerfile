FROM ubuntu:24.04@sha256:33ceb71981b602c1a7443a53469e4dba065f7503eab3078a2d7a57a2ab987517

ARG NODE_VERSION=22.23.2
ARG NODE_SHA256=d60acfe00a2932254bb0ad20e01b0d74397a0875595de719654b214f4b03f307
ARG PRIME_AGENT_VERSION=0.8.0
ARG PRIME_AGENT_PACKAGE_URL=https://pub-728493de92a943e2a9b2d17b4719f318.r2.dev/releases/v0.8.0/prime-agent-0.8.0.tgz
ARG PRIME_AGENT_PACKAGE_SHA256=f5b0093c7e0fddb73f94773d74383585456adfa84f12a4082d3098f23bb8fab6

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl git xz-utils \
  && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /opt/prime-agent/node \
  && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" -o /tmp/node.tar.xz \
  && echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - \
  && tar -xJf /tmp/node.tar.xz -C /opt/prime-agent/node --strip-components=1 \
  && rm /tmp/node.tar.xz

ENV PATH="/opt/prime-agent/node/bin:/root/.local/bin:${PATH}"
WORKDIR /opt/prime-agent/app

COPY package.json package-lock.json ./

RUN curl -fsSL "${PRIME_AGENT_PACKAGE_URL}" -o /tmp/prime-agent.tgz \
  && echo "${PRIME_AGENT_PACKAGE_SHA256}  /tmp/prime-agent.tgz" | sha256sum -c -

RUN PRIME_AGENT_BOOTSTRAP_TOOLS_ON_INSTALL=1 \
  PRIME_AGENT_BOOTSTRAP_KERNEL_ON_INSTALL=1 \
  PRIME_AGENT_INSTALL_UV=1 \
  npm ci --omit=dev --no-fund --no-audit --loglevel=error --progress=false \
  && rm /tmp/prime-agent.tgz \
  && rm -rf /root/.cache /root/.npm

RUN printf '%s\n' \
  "prime-agent-version=${PRIME_AGENT_VERSION}" \
  "prime-agent-package-sha256=${PRIME_AGENT_PACKAGE_SHA256}" \
  "node-version=${NODE_VERSION}" \
  > /opt/prime-agent/MANIFEST \
  && ln -sf /opt/prime-agent/app/node_modules/.bin/prime-agent /usr/local/bin/prime-agent \
  && ln -sf /opt/prime-agent/node/bin/node /usr/local/bin/node \
  && ln -sf /opt/prime-agent/node/bin/npm /usr/local/bin/npm \
  && ln -sf /opt/prime-agent/node/bin/npx /usr/local/bin/npx \
  && prime-agent --version \
  && node --version \
  && uv --version \
  && /root/.prime/agent/kernel-venv/bin/python --version
