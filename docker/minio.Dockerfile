# syntax=docker/dockerfile:1
# Local development/test fixture. MinIO community images are no longer distributed.
# Build the pinned upstream source releases instead of relying on vanished image tags.
FROM golang:1.25-bookworm AS minio-build
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build CGO_ENABLED=0 go install github.com/minio/minio@RELEASE.2025-10-15T17-29-55Z

FROM golang:1.25-bookworm AS mc-build
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build CGO_ENABLED=0 go install github.com/minio/mc@RELEASE.2025-08-13T08-35-41Z

FROM debian:bookworm-slim AS minio
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=minio-build /go/bin/minio /usr/local/bin/minio
EXPOSE 9000 9001
HEALTHCHECK --interval=5s --timeout=3s CMD curl -fsS http://127.0.0.1:9000/minio/health/ready || exit 1
ENTRYPOINT ["minio"]

FROM debian:bookworm-slim AS mc
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=mc-build /go/bin/mc /usr/local/bin/mc
ENTRYPOINT ["mc"]
