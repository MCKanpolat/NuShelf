#!/usr/bin/env bash
# Restore a backup made by scripts/backup.sh into a Docker Compose deployment.
#
#   scripts/restore.sh <backup-dir> [--yes]
#
# This REPLACES the current database contents. Storage is restored first and
# the database second, then the server is started again. Existing storage
# objects are never deleted. Provider settings (.env) must already point at the
# storage you want to restore into; restoring into a different provider or
# bucket than the backup came from is supported.
#
# COMPOSE overrides the Compose command, e.g. COMPOSE="docker compose -p nushelf".
set -euo pipefail

dir=${1:?usage: scripts/restore.sh <backup-dir> [--yes]}
COMPOSE=${COMPOSE:-docker compose}
[ -f "$dir/db.dump" ] && [ -f "$dir/storage/manifest.json" ] || {
  echo "$dir is not a NuShelf backup (db.dump or storage/manifest.json is missing)" >&2
  exit 1
}
dir=$(cd "$dir" && pwd)

if [ "${2:-}" != "--yes" ]; then
  read -r -p "Replace the current database with $dir/db.dump? [y/N] " answer
  [ "$answer" = "y" ] || [ "$answer" = "Y" ] || { echo "Aborted."; exit 1; }
fi

echo "Stopping the application..."
$COMPOSE stop app
$COMPOSE up -d --wait postgres

echo "Restoring package storage..."
$COMPOSE run --rm --no-deps -v "$dir/storage:/backup:ro" app storage-import /backup

echo "Restoring PostgreSQL..."
$COMPOSE exec -T postgres pg_restore --username nushelf --dbname nushelf \
  --clean --if-exists --no-owner --exit-on-error <"$dir/db.dump"

echo "Starting the application..."
$COMPOSE up -d app
echo "Restore complete."
