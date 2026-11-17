# 15 — Provider affinity headers (opencode-go)

## Why

opencode-go rejects requests without a conversation affinity header:

```json
{"type":"error","error":{"type":"MissingSessionID",
 "message":"Request is missing x-opencode-session and cannot be routed efficiently."}}
```

With a stable `x-opencode-session` value the same request returns `200`. The
value is not an auth token. It is the routing and prompt-cache affinity key.

## Semantics

opencode derives affinity as:

```text
parentID ?? fork.sessionID ?? session.id
```

Children therefore share the parent's cache lane. The other headers set by
opencode are:

```text
x-opencode-session-id : concrete session id
x-opencode-session    : affinity
x-session-affinity    : affinity
X-Session-Id          : affinity
x-opencode-client     : client name
```

`x-opencode-session-id` alone is **not** enough; the gateway specifically
requires `x-opencode-session`.

A live cache check against `deepseek-v4.1-flash`:

- same affinity, second request → `cached_tokens: 4992`
- different affinity, same prompt → `cached_tokens: 0`

A random value per request would remove the 400 but lose the cache.

## Stark implementation

- `Provider::stream_chat` remains the no-header wrapper.
- `Provider::stream_chat_for_session` adds the affinity headers when
  `ProviderConfig.mode == "opencode"`.
- `ChatStore::chat_affinity` returns `parent_chat_id` when present, otherwise
  the chat id.
- `RunContext::affinity` delegates to the store.
- Both the main agent loop and compaction summary requests pass the same
  `(chat_id, affinity)` pair.
- Other provider modes (OpenRouter, DeepSeek, CommandCode) send no opencode
  headers.

## Compression note

Request-body gzip/zstd is not enabled. Both encodings were tested against
opencode-go, OpenRouter, DeepSeek, and CommandCode and were rejected with
`400`. The standard `Content-Encoding` mechanism works only when the endpoint
decodes it.
