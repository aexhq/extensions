# Compose installed extensions with an immutable public Brain image.
ARG BRAIN_IMAGE
FROM node:22.23.2-bookworm-slim AS extensions
WORKDIR /src
COPY package.json package-lock.json ./
COPY packages packages
COPY shared shared
COPY tools tools
RUN npm ci --ignore-scripts \
    && npm run build --workspace @aexhq/agentloop-pi --workspace @aexhq/agentloop-codex --workspace @aexhq/agentloop-opencode \
    && npm ci --omit=dev --ignore-scripts --workspace @aexhq/agentloop-pi --workspace @aexhq/agentloop-codex --workspace @aexhq/agentloop-opencode

FROM oven/bun:1.3.14 AS bun
FROM ${BRAIN_IMAGE}
COPY --from=extensions /src/node_modules /opt/agentloops/node_modules
COPY --from=extensions /src/packages/loop-pi /opt/agentloops/packages/loop-pi
COPY --from=extensions /src/packages/loop-codex /opt/agentloops/packages/loop-codex
COPY --from=extensions /src/packages/loop-opencode /opt/agentloops/packages/loop-opencode
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
ENV PATH=/opt/agentloops/node_modules/.bin:$PATH \
    BRAIN_AGENTLOOPS='{"pi":{"command":"brain-agentloop-pi"},"codex":{"command":"brain-agentloop-codex"},"opencode":{"command":"brain-agentloop-opencode"}}'
