FROM node:22.23.2-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends bash ripgrep python3 python3-venv && rm -rf /var/lib/apt/lists/*
COPY packages/tools/dist/runtime /opt/aex/tools
COPY packages/env-local/image/runner.mjs /opt/aex/runner.mjs
RUN printf '{}' > /opt/aex/python-projects.json
WORKDIR /workspace
ENTRYPOINT ["node", "/opt/aex/runner.mjs"]
