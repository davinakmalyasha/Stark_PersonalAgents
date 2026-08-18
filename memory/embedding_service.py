"""
Embedding Service — shared vector infrastructure for Pillars 76, 97, 109.

Uses BAAI/bge-large-en-v1.5 (1024-dim) via fastembed ONNX runtime.
Zero API cost, fully local. Model is lazy-loaded on first embed() call.

Collections:
  - "checkpoints": Checkpoint summaries for semantic history search (Pillar 76)
  - "past_fixes": Bug-fix signatures cross-session (Pillar 97)
  - "routes": Request → route mappings (Pillar 109)

Pillar 87: Zero-Overhead Memory Mapping — vectors.db uses SQLite mmap for
  zero-copy reads, letting the OS page the file directly into the process
  address space. Retrieval is microsecond-fast with minimal RAM.

Pillar 119: Memory-Mapped Ephemeral Vector Checkpoints — writes are batched
  in an in-memory buffer and flushed to disk periodically (every N writes or
  on search). Reduces physical write overhead and fsync stalls.
"""
import os
import sqlite3
import json
import time
import threading
from typing import Optional

import numpy as np

# ── Lazy-loaded model ───────────────────────────────────────────────────────

_model: Optional[object] = None
_model_lock = threading.Lock()
_EMBEDDING_DIM = 1024  # bge-large-en-v1.5


def _get_model():
    """Lazy-load the embedding model (thread-safe)."""
    global _model
    if _model is not None:
        return _model
    with _model_lock:
        if _model is not None:
            return _model
        from fastembed import TextEmbedding
        # Suppress HF symlink warnings on Windows
        os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
        # Default the fastembed cache to D: — the model is ~1.3GB and C: is often
        # too full. Respect an explicit FASTEMBED_CACHE_PATH if the operator set one.
        if not os.environ.get("FASTEMBED_CACHE_PATH"):
            harness_root = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
            os.environ["FASTEMBED_CACHE_PATH"] = os.path.join(harness_root, ".deep_agents", "fastembed_cache")
        _model = TextEmbedding(model_name="BAAI/bge-large-en-v1.5")
        return _model


# ── Embedding API ────────────────────────────────────────────────────────────

# ── Pillar 117: Embedding Cache ─────────────────────────────────────────────
# LRU cache for embed() results. Query strings repeat often (similar search
# queries, re-indexing unchanged code), so caching avoids redundant ONNX passes.

import hashlib as _hashlib
_EMBED_CACHE: dict[str, np.ndarray] = {}
_EMBED_CACHE_MAX = 500  # ~500 * 1024 * 4 bytes = 2MB — negligible

def embed(text: str) -> np.ndarray:
    """Convert a single text string to a 1024-dim float32 vector."""
    model = _get_model()
    results = list(model.embed([text]))
    return np.asarray(results[0], dtype=np.float32)


def embed_cached(text: str) -> np.ndarray:
    """
    Pillar 117: Cached embedding lookup.
    Returns cached vector if text was previously embedded, otherwise embeds and caches.
    LRU eviction when cache exceeds _EMBED_CACHE_MAX entries.
    """
    key = _hashlib.sha256(text.encode()).hexdigest()
    cached = _EMBED_CACHE.get(key)
    if cached is not None:
        return cached

    vec = embed(text)

    # LRU eviction if full
    if len(_EMBED_CACHE) >= _EMBED_CACHE_MAX:
        # Remove oldest 20% of entries
        remove_count = max(1, _EMBED_CACHE_MAX // 5)
        for old_key in list(_EMBED_CACHE.keys())[:remove_count]:
            del _EMBED_CACHE[old_key]

    _EMBED_CACHE[key] = vec
    return vec


def clear_embed_cache() -> None:
    """Clear the embedding cache (useful when switching models or testing)."""
    _EMBED_CACHE.clear()


def embed_batch(texts: list[str]) -> list[np.ndarray]:
    """Convert multiple texts to vectors (more efficient than calling embed() N times)."""
    model = _get_model()
    results = list(model.embed(texts))
    return [np.asarray(r, dtype=np.float32) for r in results]


def cosine_similarity(a: np.ndarray, b: np.ndarray) -> float:
    """Cosine similarity between two vectors. Returns 0.0-1.0."""
    dot = float(np.dot(a, b))
    norm = float(np.linalg.norm(a)) * float(np.linalg.norm(b))
    if norm == 0.0:
        return 0.0
    return dot / norm


# ── Vector Store (in-SQLite, mmap-backed) ───────────────────────────────────

_VECTOR_DB_PATH = os.path.join(os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)), ".deep_agents", "vectors.db")
_VECTOR_CONN: Optional[sqlite3.Connection] = None
_MMAP_SIZE = 268435456  # 256MB — Pillar 87


def _get_conn() -> sqlite3.Connection:
    """
    Get the SQLite connection for the vector store.

    Pillar 87: Uses memory-mapped I/O (mmap_size=256MB) for zero-copy reads.
    The OS pages the database file directly into the process address space,
    eliminating read() syscalls. Combined with WAL journaling for crash-safe writes.
    """
    global _VECTOR_CONN
    if _VECTOR_CONN is not None:
        return _VECTOR_CONN
    os.makedirs(os.path.dirname(_VECTOR_DB_PATH), exist_ok=True)
    _VECTOR_CONN = sqlite3.connect(_VECTOR_DB_PATH, check_same_thread=False)
    # Pillar 87: Memory-mapped I/O — the OS manages caching, zero-copy reads
    _VECTOR_CONN.execute(f"PRAGMA mmap_size={_MMAP_SIZE};")
    _VECTOR_CONN.execute("PRAGMA journal_mode=WAL;")
    _VECTOR_CONN.execute("PRAGMA cache_size=-8000;")   # 8MB page cache
    _VECTOR_CONN.execute("PRAGMA synchronous=NORMAL;")  # Safe with WAL, faster writes
    _VECTOR_CONN.execute("PRAGMA temp_store=MEMORY;")   # Temp tables in RAM
    return _VECTOR_CONN


def _ensure_table(collection: str) -> None:
    """Create vector table for a collection if it doesn't exist."""
    conn = _get_conn()
    conn.execute(f"""
        CREATE TABLE IF NOT EXISTS vec_{collection} (
            id TEXT PRIMARY KEY,
            vector BLOB NOT NULL,     -- float32[] as bytes
            metadata TEXT,            -- JSON metadata
            created_at REAL NOT NULL  -- unix timestamp
        )
    """)
    conn.commit()


def _vector_to_blob(vec: np.ndarray) -> bytes:
    """Serialize a numpy float32 vector to bytes."""
    return vec.astype(np.float32).tobytes()


def _blob_to_vector(blob: bytes) -> np.ndarray:
    """Deserialize bytes back to numpy float32 vector."""
    return np.frombuffer(blob, dtype=np.float32)


# ── Pillar 119: In-Memory Write Buffer ──────────────────────────────────────
# Batches writes to reduce fsync() calls and physical disk I/O.
# Flushed on: vector_search(), flush_vector_buffer(), or when buffer is full.

_WRITE_BUFFER: dict[str, list[tuple]] = {}  # collection → [(id, blob, meta_json, ts)]
_BUFFER_LOCK = threading.Lock()
_FLUSH_THRESHOLD = 10  # Flush after this many pending writes per collection
_AUTO_FLUSH_ENABLED = True


def vector_store(collection: str, key_id: str, text: str,
                 metadata: Optional[dict] = None) -> None:
    """
    Store a text as an embedding vector in a collection.
    Replaces existing entry with the same key_id (upsert).

    Pillar 119: Write goes to in-memory buffer first. Flushed to disk
    when buffer reaches threshold or on the next search.
    """
    _ensure_table(collection)
    vec = embed(text)
    blob = _vector_to_blob(vec)
    meta_json = json.dumps(metadata or {}, ensure_ascii=False)
    ts = time.time()

    with _BUFFER_LOCK:
        buf = _WRITE_BUFFER.setdefault(collection, [])
        buf.append((key_id, blob, meta_json, ts))

        # Pillar 119: Flush if buffer is full
        if _AUTO_FLUSH_ENABLED and len(buf) >= _FLUSH_THRESHOLD:
            _flush_collection_locked(collection)


def _flush_collection_locked(collection: str) -> None:
    """Flush buffered writes for one collection to disk. Must hold _BUFFER_LOCK."""
    buf = _WRITE_BUFFER.get(collection, [])
    if not buf:
        return

    conn = _get_conn()
    # Batch insert — single transaction for all pending writes
    conn.execute("BEGIN IMMEDIATE")
    try:
        conn.executemany(
            f"INSERT OR REPLACE INTO vec_{collection} (id, vector, metadata, created_at) "
            f"VALUES (?, ?, ?, ?)",
            buf,
        )
        conn.commit()
        buf.clear()
    except Exception:
        conn.rollback()
        raise


def flush_vector_buffer(collection: Optional[str] = None) -> None:
    """
    Pillar 119: Force-flush the write buffer to disk.
    If collection is None, flushes all collections.
    Called at major graph milestones and before graph END.
    """
    with _BUFFER_LOCK:
        if collection is not None:
            _flush_collection_locked(collection)
        else:
            for col in list(_WRITE_BUFFER.keys()):
                _flush_collection_locked(col)


def set_auto_flush(enabled: bool) -> None:
    """Enable/disable automatic buffer flushing. Disable during bulk imports."""
    global _AUTO_FLUSH_ENABLED
    _AUTO_FLUSH_ENABLED = enabled


def _search_buffer(collection: str, query_vec: np.ndarray,
                   threshold: float) -> list[dict]:
    """Search unflushed buffer entries for a collection."""
    buf = _WRITE_BUFFER.get(collection, [])
    if not buf:
        return []

    results = []
    for key_id, blob, meta_json, ts in buf:
        stored_vec = _blob_to_vector(blob)
        sim = cosine_similarity(query_vec, stored_vec)
        if sim >= threshold:
            results.append({
                "id": key_id,
                "score": round(sim, 4),
                "metadata": json.loads(meta_json) if meta_json else {},
                "created_at": ts,
            })
    return results


def vector_search(collection: str, query_text: str, top_k: int = 5,
                  threshold: float = 0.60) -> list[dict]:
    """
    Search a collection for entries semantically similar to query_text.
    Returns list of {id, score, metadata} sorted by similarity descending.

    Pillar 119: Flushes the write buffer before searching to ensure
    all stored entries are visible. Also searches unflushed buffer entries.

    Uses brute-force cosine similarity (OK for up to ~50K entries per collection).
    Pillar 87: The SQLite scan benefits from mmap — the OS has already paged
    the database file into memory, making this scan microsecond-fast.
    """
    _ensure_table(collection)

    # Pillar 119: Flush before search for consistency
    with _BUFFER_LOCK:
        _flush_collection_locked(collection)

    query_vec = embed(query_text)
    conn = _get_conn()
    rows = conn.execute(
        f"SELECT id, vector, metadata, created_at FROM vec_{collection}"
    ).fetchall()

    results = []

    # Brute-force similarity search over disk rows (mmap-accelerated — Pillar 87)
    if rows:
        for row_id, blob, meta_json, created_at in rows:
            stored_vec = _blob_to_vector(blob)
            sim = cosine_similarity(query_vec, stored_vec)
            if sim >= threshold:
                results.append({
                    "id": row_id,
                    "score": round(sim, 4),
                    "metadata": json.loads(meta_json) if meta_json else {},
                    "created_at": created_at,
                })

    results.sort(key=lambda x: x["score"], reverse=True)
    return results[:top_k]


def vector_remove(collection: str, key_id: str) -> None:
    """Remove a single entry from a collection (disk + buffer)."""
    _ensure_table(collection)

    # Remove from buffer
    with _BUFFER_LOCK:
        buf = _WRITE_BUFFER.get(collection, [])
        _WRITE_BUFFER[collection] = [e for e in buf if e[0] != key_id]
        _flush_collection_locked(collection)

    # Remove from disk
    conn = _get_conn()
    conn.execute(f"DELETE FROM vec_{collection} WHERE id = ?", (key_id,))
    conn.commit()


def vector_clear(collection: str) -> None:
    """Remove all entries from a collection (disk + buffer)."""
    _ensure_table(collection)

    # Clear buffer
    with _BUFFER_LOCK:
        _WRITE_BUFFER.pop(collection, None)

    # Clear disk
    conn = _get_conn()
    conn.execute(f"DELETE FROM vec_{collection}")
    conn.commit()


def vector_count(collection: str) -> int:
    """Return number of entries in a collection (disk + buffer)."""
    _ensure_table(collection)
    conn = _get_conn()
    row = conn.execute(f"SELECT COUNT(*) FROM vec_{collection}").fetchone()
    disk_count = row[0] if row else 0
    with _BUFFER_LOCK:
        buf_count = len(_WRITE_BUFFER.get(collection, []))
    return disk_count + buf_count


def vector_stats() -> dict:
    """Return statistics for all collections in the vector store."""
    # Flush all before reporting
    flush_vector_buffer()
    conn = _get_conn()
    tables = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'vec_%'"
    ).fetchall()
    stats = {}
    for (name,) in tables:
        collection = name[4:]  # Remove "vec_" prefix
        row = conn.execute(f"SELECT COUNT(*) FROM {name}").fetchone()
        stats[collection] = row[0] if row else 0
    return stats
