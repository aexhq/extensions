FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p environment-driver

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 65532 environment \
    && useradd --system --uid 65532 --gid 65532 --no-create-home --home-dir /nonexistent environment
COPY --from=build /src/target/release/environment-driver /usr/local/bin/environment-driver
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/environment-driver"]
