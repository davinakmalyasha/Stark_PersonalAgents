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

python3 - "$SRC" "$WORK" <<'PY'
import json, sqlite3, sys, zipfile, pathlib
archive, out = sys.argv[1], pathlib.Path(sys.argv[2])
with zipfile.ZipFile(archive) as z:
    names = z.namelist()
    assert "manifest.json" in names, "missing manifest.json"
    manifest = json.loads(z.read("manifest.json"))
    assert manifest.get("app") == "stark", f"not a Stark backup: {manifest}"
    assert ".deep_agents/chats.db" in names, "missing chats.db"
    db_dir = out / "db"
    db_dir.mkdir()
    for name in names:
        if name.endswith("chats.db"):
            (db_dir / "chats.db").write_bytes(z.read(name))
    chats = sqlite3.connect(db_dir / "chats.db")
    total = chats.execute("select count(*) from chats").fetchone()[0]
    tables = chats.execute(
        "select count(*) from sqlite_master where type='table'"
    ).fetchone()[0]
    print(f"manifest: app={manifest['app']} format={manifest.get('format')} "
          f"exported={manifest.get('exported_at')}")
    print(f"projects root at export: {manifest.get('projects_root')}")
    print(f"database: {total} chat(s), {tables} table(s) — integrity:", end=" ")
    print(chats.execute("pragma integrity_check").fetchone()[0])
PY

echo "restore drill OK"
