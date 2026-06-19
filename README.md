# Xenonite

Xenonite is an always-on local API daemon for semantic memory, code search, context artifacts, and lightweight code graph operations.

It runs as a single Rust service with embedded zvec vector storage. No Qdrant, Node runtime, or sidecar database is required.

## Features

- Embedded zvec vector storage with durable local data
- Scoped semantic memory store and recall
- Sequential LLM-assisted memory optimization, dedupe, cleanup, and scope reclassification
- Project code indexing and semantic search
- Context artifact indexing and search
- Lightweight graph build/query endpoints
- Docker Compose service for long-running local use
- Host path compatibility for `/host/...` paths used by tool runners

## Run with Docker

```bash
docker compose up -d --build
```

The API listens on `127.0.0.1:8700` by default.

Persistent data is stored in the `xenonite_data` Docker volume at `/var/lib/xenonite` inside the container.

## Configuration

Environment variables:

| Variable | Default | Description |
| --- | --- | --- |
| `XENONITE_PORT` | `8700` | API port inside the container/process |
| `XENONITE_DATA_DIR` | platform data dir or `/var/lib/xenonite` in Docker | Persistent zvec and manifest storage |
| `ROCKY_LLM_URL` | `http://host.docker.internal:7777/v1` | OpenAI-compatible LLM endpoint |
| `ROCKY_LLM_MODEL` | `gpt-oss:20b` | LLM model name used for memory optimization/classification |
| `ROCKY_LLM_API_KEY` | empty | LLM API key |
| `ROCKY_EMBED_URL` | `http://host.docker.internal:7778/v1` | OpenAI-compatible embedding endpoint |
| `ROCKY_EMBED_MODEL` | `text-embedding-3-small` | Embedding model name |
| `ROCKY_EMBED_API_KEY` | empty | Embedding API key |

## Local Development

```bash
cargo run
```

Run checks:

```bash
cargo fmt --check
cargo check
cargo test
```

## API

### Health

```bash
curl http://127.0.0.1:8700/health
```

### Store Memory

```bash
curl http://127.0.0.1:8700/v1/memory/store \
  -H 'content-type: application/json' \
  -d '{
    "text": "Xenonite stores semantic memory locally.",
    "source": "verified_durable_fact",
    "memoryScope": "project",
    "projectPath": "/host/Users/you/project",
    "workGroupId": "example-group",
    "workUnitId": "example-unit",
    "agentId": "example-agent"
  }'
```

### Recall Memory

```bash
curl http://127.0.0.1:8700/v1/memory/recall \
  -H 'content-type: application/json' \
  -d '{"query":"semantic memory locally","top_k":5,"memoryScope":"project","projectPath":"/host/Users/you/project"}'
```

### Optimize Memory

Dry-run first:

```bash
curl http://127.0.0.1:8700/v1/memory/optimize \
  -H 'content-type: application/json' \
  -d '{"dryRun":true,"maxFacts":200,"batchSize":8,"useLlm":true}'
```

Apply only after reviewing the dry-run:

```bash
curl http://127.0.0.1:8700/v1/memory/optimize \
  -H 'content-type: application/json' \
  -d '{"apply":true,"maxFacts":200,"batchSize":8,"useLlm":true}'
```

### Index Code

```bash
curl http://127.0.0.1:8700/v1/code/index \
  -H 'content-type: application/json' \
  -d '{"projectPath":"/host/Users/you/project"}'
```

### Search Code

```bash
curl http://127.0.0.1:8700/v1/code/search \
  -H 'content-type: application/json' \
  -d '{"projectPath":"/host/Users/you/project","query":"storage backend","limit":5}'
```

## Storage Model

Xenonite stores vectors in zvec collections and keeps compact sidecar manifests for recovery and deterministic fallback. Memory is physically separated by scope:

- `memory_global` — operator preferences/style only.
- `memory_project_*` — repo/project facts, keyed by project path.
- `memory_path_*` — folder/path facts, keyed by path namespace.

Work metadata is first-class:

- `projectId`
- `workGroupId`
- `workUnitId`
- `agentId`

This lets searches and memories remain attributable to the agent work unit that produced them.

Global memory should not contain repo- or folder-specific facts. Project/folder facts must be written with `memoryScope: "project"` or `memoryScope: "path"` plus the corresponding `projectPath`, `path`, or `pathId`.

## Docker Operations

Start:

```bash
docker compose up -d --build
```

Logs:

```bash
docker compose logs -f xenonite
```

Stop:

```bash
docker compose down
```
