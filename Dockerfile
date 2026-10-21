# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS dev
RUN rustup component add rustfmt clippy
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY migrations ./migrations
COPY feed-migrations ./feed-migrations
COPY web ./web
COPY tests ./tests
COPY examples ./examples
CMD ["cargo", "test", "--locked", "--", "--include-ignored"]

FROM dev AS build
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git --mount=type=cache,target=/app/target cargo build --release --locked --bin nushelf && cp target/release/nushelf /usr/local/bin/nushelf

FROM mcr.microsoft.com/dotnet/sdk:10.0-noble AS apicompat
RUN dotnet tool install Microsoft.DotNet.ApiCompat.Tool --version 10.0.402 --allow-roll-forward --tool-path /opt/apicompat

FROM mcr.microsoft.com/dotnet/runtime:10.0-noble AS runtime
COPY --from=apicompat /opt/apicompat /opt/apicompat
ENV APICOMPAT_PATH=/opt/apicompat/apicompat
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home nushelf && mkdir -p /data/packages && chown -R nushelf:nushelf /data
COPY --from=build /usr/local/bin/nushelf /usr/local/bin/nushelf
USER nushelf
ENV BIND_ADDRESS=0.0.0.0:5000 STORAGE_PATH=/data/packages
EXPOSE 5000
HEALTHCHECK --interval=15s --timeout=3s CMD curl -fsS http://127.0.0.1:5000/health/ready || exit 1
ENTRYPOINT ["nushelf"]
