FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --manifest-path packages/env-aws-microvm/runtime/Cargo.toml --locked --release -p environment-driver

FROM node:22.23.2-bookworm-slim AS tools
WORKDIR /src
COPY . .
RUN npm ci && npm run build --workspace @aexhq/tools

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 65532 environment \
    && useradd --system --uid 65532 --gid 65532 --no-create-home --home-dir /nonexistent environment
COPY --from=build /src/packages/env-aws-microvm/runtime/target/release/environment-driver /usr/local/bin/environment-driver
COPY --from=tools /src/packages/tools/dist/runtime /usr/local/share/aex-tools
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/environment-driver"]
