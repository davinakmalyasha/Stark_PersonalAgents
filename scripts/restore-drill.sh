#!/usr/bin/env bash
# Restore drill: prove a snapshot is usable without touching live data.
#   bash restore-drill.sh /var/lib/stark-data/.deep_agents/exports/stark-backup-....zip[.enc]
# For .enc files set STARK_SNAPSHOT_PASSPHRASE in the environment.
set -euo pipefail

SRC="${1:?usage: restore-drill.sh <backup.zip|backup.zip.enc>}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ "$SRC" == *.enc ]]; then
  : "${STARK_SNAPSHOT_PASSPHRASE:?set STARK_SNAPSHOT_PASSPHRASE to decrypt}"
  PLAIN="$WORK/backup.zip"
  openssl enc -d -aes-256-cbc -pbkdf2 -salt -in "$SRC" -out "$PLAIN" \
    -pass env:STARK_SNAPSHOT_PASSPHRASE
  SRC="$PLAIN"
fi

unzip -Z1 "$SRC" | grep -qx 'manifest.json' || { echo "missing manifest.json"; exit 1; }
manifest="$(unzip -p "$SRC" manifest.json)"
app="$(jq -r '.app // empty' <<<"$manifest")"
[ "$app" = "stark" ] || { echo "not a Stark backup: $manifest"; exit 1; }
unzip -Z1 "$SRC" | grep -qx '.deep_agents/chats.db' || { echo "missing chats.db"; exit 1; }

mkdir -p "$WORK/db"
unzip -p "$SRC" .deep_agents/chats.db > "$WORK/db/chats.db"
total="$(sqlite3 "$WORK/db/chats.db" 'select count(*) from chats;')"
tables="$(sqlite3 "$WORK/db/chats.db" "select count(*) from sqlite_master where type='table';")"
integrity="$(sqlite3 "$WORK/db/chats.db" 'pragma integrity_check;')"

echo "manifest: app=$app format=$(jq -r '.format // empty' <<<"$manifest") exported=$(jq -r '.exported_at // empty' <<<"$manifest")"
echo "projects root at export: $(jq -r '.projects_root // empty' <<<"$manifest")"
echo "database: $total chat(s), $tables table(s) — integrity: $integrity"

echo "restore drill OK"
