import { serve } from "@hono/node-server";
import { Hono } from "hono";
import * as core from "./core.mjs";
import { handleMcpJsonRpc, mcpManifest } from "./mcp-core.mjs";
import { config } from "./config.mjs";

const app = new Hono();

app.get("/health", async (c) => c.json(await core.health()));
app.get("/v1/mcp/manifest", (c) => c.json(mcpManifest()));
app.post("/v1/mcp", async (c) => c.json(await handleMcpJsonRpc(await c.req.json().catch(() => null))));

if (process.argv[2] === "mcp") {
	await import("./mcp-stdio.mjs");
} else {
	serve({ fetch: app.fetch, port: config.port }, (info) => {
		console.log(`[xenonite] listening on :${info.port}`);
		console.log(`[xenonite] rocky LLM: ${config.rocky.llmBaseUrl} | embeddings: ${config.rocky.embedBaseUrl}`);
		console.log(`[xenonite] data: ${config.dataDir}`);
	});
}
