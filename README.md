# Xenonite

Xenonite is the MCP-first project intelligence core for amaze and external project hosts.

## MCP bridge

Use Xenonite as the primary MCP/App SDK surface for ChatGPT Pro projects, Claude Desktop, and other external MCP clients:

```bash
npm run mcp
# or, when installed as a bin:
xenonite mcp
```

HTTP MCP JSON-RPC is available as a compatibility transport for clients that cannot use stdio:

```bash
npm run start
curl -s http://127.0.0.1:8700/v1/mcp/manifest
```

The HTTP service intentionally exposes only `/health`, `/v1/mcp/manifest`, and `/v1/mcp`. Memory and code intelligence are accessed through MCP tools, not separate REST management APIs.

Tool modes:

- `minimal` (default): `xenonite_server_config`, `xenonite_health`
- `standard`: read-only memory/code intelligence (`xenonite_memory_recall`, `xenonite_semantic_search`, graph/context tools)
- `full`: state-mutating tools (`xenonite_code_index`, `xenonite_code_graph_build`, `xenonite_memory_store`)

Example:

```bash
XENONITE_MCP_TOOL_MODE=standard npm run mcp
```

Browser/computer-use automation is intentionally not part of Xenonite. Keep it in amaze's opt-in `desk_*` tool family and use Xenonite MCP for project memory, semantic search, graph, and context.

Skills management is intentionally not part of Xenonite. Reusable agent procedures belong in amaze's skill/runtime system or in durable memory as explicit project facts, not in a separate Xenonite management plane.
