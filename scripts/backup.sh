#!/usr/bin/env bash
# Back up a Docker Compose deployment: PostgreSQL dump plus package storage.
#
#   scripts/backup.sh <backup-dir>
#
# The database is dumped first and storage second. Every committed row points
# at a blob that was written earlier, so this order never yields a database that
# references a missing archive. The server can keep running during the backup.
# Repeating the command against the same directory only copies new packages.
#
# COMPOSE overrides the Compose command, e.g. COMPOSE="docker compose -p nushelf".
set -euo pipefail

dir=${1:?usage: scripts/backup.sh <backup-dir>}
COMPOSE=${COMPOSE:-docker compose}
mkdir -p "$dir/storage"
dir=$(cd "$dir" && pwd)

echo "Dumping PostgreSQL..."
$COMPOSE exec -T postgres pg_dump --username nushelf --dbname nushelf \
  --format custom --no-owner >"$dir/db.dump.part"
mv "$dir/db.dump.part" "$dir/db.dump"

echo "Copying package storage..."
$COMPOSE run --rm --no-deps --user "$(id -u):$(id -g)" \
  -v "$dir/storage:/backup" app storage-export /backup

echo "Verifying the copy..."
$COMPOSE run --rm --no-deps --user "$(id -u):$(id -g)" \
  -v "$dir/storage:/backup:ro" app storage-verify /backup

echo "Backup complete: $dir"
