# copenai

OpenAI-compatible HTTP API backed by the Cursor agent CLI (`cursor agent acp`).

Drop-in for OpenAI SDKs: point `base_url` at copenai, use a wrapper API key from `copenai keys add`. Cursor credentials stay separate (`copenai auth login` or `CURSOR_API_KEY`).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/beeinger/copenai/main/install.sh | bash
```

Requires Rust and the Cursor agent CLI on `PATH`.

Data directory: `~/.copenai` (override with `COPENAI_HOME`).

## Quick start

```bash
copenai doctor
copenai auth login          # or: copenai auth api-key --key "$CURSOR_API_KEY"
copenai keys add --name dev
copenai start
```

Default server: `http://127.0.0.1:9241`

## OpenAI compatibility & test coverage

Single reference for what works, how it maps to OpenAI, what does not, and where it is tested.

| Area | OpenAI surface | Parity | How copenai implements it | Not supported / limits | Tests |
|------|----------------|--------|---------------------------|------------------------|-------|
| **Auth** | `Authorization: Bearer …` on `/v1/*` | ✅ Same header shape | Wrapper keys (`sk-copenai-…`) in SQLite; validated per request | Keys are **not** OpenAI platform keys — generate via `copenai keys add` | `chat_mock::missing_bearer_401`, `e2e_16` |
| **Health** | *(extension)* | ➕ Extra | `GET /health` — no auth; reports cursor auth, active sessions, resume mode | Not part of OpenAI API | `e2e_01`, `chat_mock::health_no_auth` |
| **Models** | `GET /v1/models` | ✅ List shape | `object: list`, `data[].id/object/created/owned_by` | Model list is **configured** (`composer-2.5`, `auto`), not fetched from OpenAI; unknown model → **400** | `e2e_01`, `e2e_15`, `model::tests` |
| **Chat sync** | `POST /v1/chat/completions` | ✅ | Standard JSON body; `chat.completion` response with `choices[].message.content`, `usage` | Backend is Cursor agent, not OpenAI models | `chat_mock::chat_sync_mock`, `e2e_02` |
| **Chat stream** | `stream: true` → SSE | ✅ | `text/event-stream`; `chat.completion.chunk` deltas + final `finish_reason` + `[DONE]` | Chunks come from live ACP `AgentMessageChunk`, not fake token split | `chat_mock::stream_mock_emits_chunks`, `e2e_03` |
| **Messages** | `messages[]` roles | ✅ Mostly | `system` + `developer` merged; `user`/`assistant` history replayed on **cold** session; hot session uses ACP memory | History replay is **text-only** (multimodal prior turns not re-sent); unknown roles skipped | `messages::tests` (4), `e2e_04`, `e2e_06`, `e2e_07` |
| **Conversation id** | Threads / Assistants API elsewhere | ⚠️ Different hook | `X-Conversation-Id` header or `metadata.conversation_id`; auto UUID if omitted | OpenAI `user` field is **abuse metadata only**, not conversation routing | `e2e_19` |
| **Session continuity** | Stateful threads via separate API | ⚠️ Extension | ACP `load` / `resume` / degraded replay; `cursor_chat_id` in SQLite; hot session skips replay **only** for incremental turns (empty prefix `messages[]`) | Full `messages[]` always replayed when client sends prior turns | `resume::tests`, `messages::tests`, `e2e_04`, `e2e_18`, `e2e_19` |
| **Sampling** | `temperature`, `max_tokens` / `max_output_tokens` | ⚠️ Best effort | Applied via ACP `SetSessionConfigOption` when agent exposes matching option ids | Unsupported values are **skipped** (debug log), not rejected | `e2e_08`, `e2e_09` |
| **Other sampling** | `top_p`, penalties, `seed`, `n`, `logprobs`, `response_format`, … | ❌ | Request fields accepted in JSON but **not forwarded** to ACP | Cursor ACP has no OpenAI-equivalent knobs | — |
| **Usage** | `usage.{prompt,completion,total}_tokens` | ⚠️ Best-effort | ACP `UsageUpdate` when agent sends it; else char÷4 estimate | Counts may differ from OpenAI tokenizer | `e2e_02` |
| **Finish reason** | `stop`, `length`, `content_filter`, `tool_calls` | ✅ | Mapped from ACP + tool orchestration (`tool_calls` when client mode stops for tools) | Server mode may end with `stop` after webhook loop | `chat_mock::chat_tools_client_sync` |
| **Tool calling (chat)** | `tools`, `tool_choice`, `functions`, `function_call` on `/v1/chat/completions` | ✅ | Shared tool orchestrator; client + server (`tool_webhook`) modes; `metadata.tool_execution` / `X-Tool-Execution` | ACP uses prompt + JSON parse adapter, not native tool RPC | `chat_mock::chat_tools_*` |
| **Responses API** | `POST/GET/DELETE /v1/responses`, `GET /v1/responses` list | ✅ | OpenAI Responses wire format; sync + SSE; local SQLite store (`store: true`, `previous_response_id`) | Backend is Cursor ACP, not OpenAI models | `responses_mock::*` |
| **Responses tools** | `tools[]`, `tool_choice`, `parallel_tool_calls`, `truncation`, `function_call` / `function_call_output` | ✅ | Client + server modes; `tool_choice` / `parallel_tool_calls`; `truncation: auto`; `incomplete_details` | No OpenAI-hosted tool runtimes; ACP has no native tool RPC | `responses_mock::*` |
| **Responses WebSocket** | `GET /v1/responses/ws` + `response.create` | ✅ | Same event vocabulary as SSE; per-connection `previous_response_id` cache (60m) | One in-flight response per connection | `e2e_26` |
| **Responses structured output** | `text.format.type = json_schema` | ✅ | Schema injected into prompt; output validated with `jsonschema` | Best-effort on agent compliance | `responses::parser::tests` |
| **Responses reasoning** | `reasoning`, `include: ["reasoning"]` | ⚠️ Best-effort | ACP `AgentThoughtChunk` → `reasoning` output items / stream deltas | Shape may differ from OpenAI | `responses_mock::responses_agent_tool_observability` |
| **ACP agent tool observability** | *(extension)* | ➕ Extra | Cursor internal `ToolCall` streamed as `function_call` with `agent_` name prefix | Not request `tools[]`; config `[responses].stream_agent_tools` | `responses_mock::responses_agent_tool_observability` |
| **Multimodal image** | `image_url` content parts | ✅ | `data:` URLs + `http(s)://` download → session assets → ACP image blocks | No `file://`; remote images ≤ 20 MB; must be `image/*` | `e2e_10`, `e2e_12`, `multimodal::tests` |
| **Multimodal file** | `input_file` / `file` + `file_id` | ✅ | Stage via `/v1/files`, copy into session assets for ACP | Agent must accept file attachments | `e2e_10`, `e2e_11` |
| **Multimodal audio** | `input_audio` inline base64 | ✅ | Decode to temp file (e.g. `.wav`) → ACP audio block | Agent capability-dependent; not OpenAI `/v1/audio` API | `e2e_11` |
| **Files upload** | `POST /v1/files` multipart | ⚠️ Partial | Returns `id`, `object`, `bytes`, `filename` | No `purpose`, fine-tune, or batch semantics | `e2e_10`, `e2e_14` |
| **Files metadata** | `GET /v1/files/{id}` | ⚠️ Partial | `id`, `object`, `bytes` | Missing OpenAI fields (`purpose`, `status`, timestamps, …) | `e2e_14` |
| **Files content** | `GET /v1/files/{id}/content` | ✅ | Raw `application/octet-stream` | Local staging only (not cloud storage) | `e2e_14` |
| **Files list/delete** | `GET /v1/files`, `DELETE /v1/files/{id}` | ✅ | List: `object`, `data[]`, `first_id`, `last_id`, `has_more`; delete: `id`, `object`, `deleted` | `purpose` query ignored; upload has no `purpose` field; local staging only | `chat_mock::files_*`, `files::tests`, `e2e_14` |
| **Permissions** | *(extension)* | ➕ Extra | `GET /v1/permissions/pending`, `POST /v1/permissions/{id}/respond`; optional webhook when `auto_approve = false` | Cursor agent permission UX, not OpenAI | manual / config |
| **Other OpenAI APIs** | Embeddings, Images, Audio transcribe/TTS, Assistants, Batches, Fine-tuning, Moderations, … | ❌ | Catch-all **501** `endpoint not implemented` | Out of scope — wrapper targets chat + minimal files | — |
| **Errors** | `{ "error": { "message", "type", "code" } }` | ✅ | OpenAI-shaped JSON errors; proper HTTP status (401, 400, 404, 501, 500) | Message text is copenai-specific | mock + e2e error cases |
| **CLI / daemon** | — | ➕ Extra | `copenai doctor`, `auth`, `keys`, `start/stop/status`, `logs` | Process management is local-only | `core::daemon::tests`, `paths::tests` |

### Test layers

| Layer | Command | Cursor needed | What it covers |
|-------|---------|---------------|----------------|
| **Unit** | `cargo test --all --all-features` (CI on every push) | No | Message parsing, files validation, SSE framing, resume probe, API keys, multimodal mapping, skip harness |
| **Mock HTTP** | `cargo test -p copenai-server --features test-utils --test chat_mock` and `--test responses_mock` | No | Health, sync chat, SSE chunks, 401, chat tools, files list/delete, Responses sync/stream/store/tools/webhook | `MockSupervisor` backend |
| **Live E2E** | `COPENAI_E2E=1 cargo test -p copenai-e2e -- --ignored --test-threads=1 --show-output` | Yes (login or `CURSOR_API_KEY`) | 26 scenarios: chat, stream, roles, sampling, files, multimodal, Responses API, tools, WebSocket, errors | `e2e_17`–`e2e_26` |
| **Status probe** | `cargo test -p copenai-e2e live_e2e_status` | No | Always runs; prints READY/SKIPPED + reason (visible on TTY) |
| **Nightly E2E** | `.github/workflows/e2e.yml` | Optional secret | Same live suite; skips pass if no auth |

Live E2E skips (pass, no fail) when `COPENAI_E2E≠1` or agent not authenticated — not silent; see `live_e2e_status` and `SKIP` lines with `--show-output`.

## OpenAI SDK example

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:9241/v1",
    api_key="sk-copenai-...",  # from `copenai keys add`
)

client.chat.completions.create(
    model="composer-2.5",
    messages=[{"role": "user", "content": "hello"}],
    extra_headers={"X-Conversation-Id": "my-thread-1"},
)

# Responses API (OpenAI SDK 1.x+)
client.responses.create(
    model="composer-2.5",
    input="hello",
    store=True,
    extra_headers={"X-Conversation-Id": "my-thread-1"},
)
```

## CLI

| Command | Description |
|---------|-------------|
| `copenai doctor` | Versions, cursor auth, ACP resume capabilities |
| `copenai auth login` | Browser login via `cursor agent login` |
| `copenai auth api-key` | Store `CURSOR_API_KEY` in `~/.copenai/cursor.env` |
| `copenai keys add/list/delete` | Wrapper API keys |
| `copenai start/stop/status` | Background daemon |
| `copenai logs -f` | Tail `~/.copenai/logs/server.log` |

## Config

`~/.copenai/config.toml`:

```toml
[cursor]
agent_bin = "agent"

[server]
bind = "0.0.0.0:9241"
max_concurrent_agents = 32
idle_timeout_secs = 1800

[permissions]
auto_approve = true
webhook_url = ""
webhook_timeout_secs = 30

[responses]
tool_execution = "client"          # "client" | "server"
tool_webhook = ""                  # required for server mode
tool_webhook_timeout_secs = 30
tool_webhook_fallback = "none"     # "none" | "agent"
max_tool_steps = 8
stream_agent_tools = true
```

### Responses API extensions

| Mechanism | Purpose |
|-----------|---------|
| `metadata.tool_execution` | `"client"` or `"server"` tool loop |
| `X-Tool-Execution` header | Same as above (header wins over config default) |
| `[responses].tool_webhook` | HTTP endpoint for server-mode tool execution |
| `metadata.conversation_id` / `X-Conversation-Id` | Session routing (same as chat) |

## Examples

### Files + vision

```bash
curl -H "Authorization: Bearer $KEY" -F file=@image.png http://127.0.0.1:9241/v1/files
```

```json
{
  "model": "composer-2.5",
  "messages": [{
    "role": "user",
    "content": [
      {"type": "text", "text": "What's in this image?"},
      {"type": "input_file", "file_id": "file-..."}
    ]
  }]
}
```

### Inline audio

```json
{
  "type": "input_audio",
  "input_audio": {"data": "<base64>", "format": "wav"}
}
```

### Permissions (`auto_approve = false`)

- `GET /v1/permissions/pending?conversation_id=...`
- `POST /v1/permissions/{id}/respond` with `{"option_id": "..."}` or `{"cancel": true}`
- Optional `webhook_url` POST on new permission requests

## Docker

Image: [`ghcr.io/beeinger/copenai`](https://github.com/beeinger/copenai/pkgs/container/copenai)

```bash
docker run -d --name copenai \
  -p 9241:9241 \
  -v copenai-data:/data \
  -e HOME=/data \
  -e COPENAI_HOME=/data \
  -e CURSOR_API_KEY="$CURSOR_API_KEY" \
  ghcr.io/beeinger/copenai:0.1.0
```

Browser login instead of API key (session stored on the volume):

```bash
docker run -it --rm \
  -v copenai-data:/data \
  -e HOME=/data \
  -e COPENAI_HOME=/data \
  --entrypoint copenai \
  ghcr.io/beeinger/copenai:0.1.0 auth login
```

### CI releases

Pushes to **`main`** read `version` from `Cargo.toml`. If git tag `v{version}` does not exist yet, CI:

1. Builds and pushes `ghcr.io/beeinger/copenai:{version}` (and `v{version}`) plus `:latest`
2. Creates git tag `v{version}` and a GitHub release
3. Records a GitHub deployment pointing at the container package

Bump `version` in `Cargo.toml` before pushing to `main` to publish a new image.

## Kubernetes

Manifests live in [`k8s/`](k8s/).

| File | Purpose |
|------|---------|
| `k8s/deployment.yaml` | API-key auth via `CURSOR_API_KEY` secret |
| `k8s/deployment-login.yaml` | Cursor CLI browser/device login (no API key) |
| `k8s/secret.example.yaml` | Template for `copenai-secrets` |
| `k8s/bootstrap-login.sh` | Apply login deployment + interactive `copenai auth login` |

Pin the image tag to the workspace version (currently `0.1.0`).

**API key:**

```bash
kubectl apply -f k8s/secret.example.yaml   # set real key first
kubectl apply -f k8s/deployment.yaml
kubectl exec -it deploy/copenai -n copenai -- copenai keys add --name default
kubectl port-forward -n copenai svc/copenai 9241:9241
```

**Browser login:**

```bash
./k8s/bootstrap-login.sh
# or: kubectl apply -f k8s/deployment-login.yaml && kubectl exec -it deploy/copenai -n copenai -- copenai auth login
```

Data (SQLite, wrapper keys, Cursor OAuth under `.cursor/`) persists on the `copenai-data` PVC at `/data`.

## License

MIT — see [LICENSE](LICENSE).
