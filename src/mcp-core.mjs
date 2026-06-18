import * as core from "./core.mjs";

const PROTOCOL_VERSION = "2024-11-05";
const TOOL_MODE = parseToolMode(process.env.XENONITE_MCP_TOOL_MODE);
const MAX_TEXT_BYTES = numberFromEnv("XENONITE_MCP_MAX_TEXT_BYTES", 300_000, 10_000, 2_000_000);

function parseToolMode(value) {
	if (value === "standard" || value === "full") return value;
	return "minimal";
}

function numberFromEnv(name, fallback, min, max) {
	const value = Number(process.env[name] ?? fallback);
	if (!Number.isFinite(value)) return fallback;
	return Math.max(min, Math.min(max, Math.floor(value)));
}

function redact(text) {
	return String(text)
		.replace(/\bsk-[A-Za-z0-9_-]{10,}\b/g, "[REDACTED_SECRET]")
		.replace(
			/\b[A-Za-z0-9_]*(?:API[_-]?KEY|TOKEN|SECRET|PASSWORD|PRIVATE[_-]?KEY)[A-Za-z0-9_]*\s*=\s*(?:"[^"\r\n]{12,}"|'[^'\r\n]{12,}'|`[^`\r\n]{12,}`|[A-Za-z0-9_./+=-]{20,})/gi,
			(match) => `${match.slice(0, Math.max(0, match.indexOf("="))).trimEnd()}= [REDACTED_SECRET]`,
		);
}

function truncate(text) {
	const redacted = redact(text);
	const bytes = Buffer.byteLength(redacted);
	if (bytes <= MAX_TEXT_BYTES) return redacted;
	return `${Buffer.from(redacted).subarray(0, MAX_TEXT_BYTES).toString("utf8")}\n\n[truncated ${bytes - MAX_TEXT_BYTES} bytes]`;
}

function textResult(text) {
	return { content: [{ type: "text", text: truncate(text) }] };
}

function jsonText(value) {
	return truncate(JSON.stringify(value, null, 2));
}

function tool(name, description, inputSchema, handler, modes = ["minimal", "standard", "full"]) {
	return { name, description, inputSchema, handler, modes };
}

const baseTools = [
	tool(
		"xenonite_server_config",
		"Show Xenonite MCP bridge configuration and service status without revealing secrets.",
		{ type: "object", properties: { includeRockyHealth: { type: "boolean" } } },
		async (args) => {
			return textResult(jsonText({
				...(await core.serviceConfig({ includeRockyHealth: args.includeRockyHealth !== false })),
				toolMode: TOOL_MODE,
				maxTextBytes: MAX_TEXT_BYTES,
			}));
		},
	),
	tool(
		"xenonite_health",
		"Check Xenonite and Rocky health for external MCP clients.",
		{ type: "object", properties: {} },
		async () => textResult(jsonText(await core.health())),
	),
];

const readOnlyTools = [
	tool(
		"xenonite_memory_recall",
		"Recall bounded durable memory from Xenonite. Use this for ChatGPT Pro project context.",
		{
			type: "object",
			properties: {
				query: { type: "string" },
				top_k: { type: "number" },
				memory_scope: { type: "string" },
				namespace: { type: "string" },
				projectPath: { type: "string" },
			},
			required: ["query"],
		},
		async (args) => textResult(jsonText(await core.recallMemory(args))),
		["standard", "full"],
	),
	tool(
		"xenonite_code_status",
		"Report codebase index status for a project.",
		{ type: "object", properties: { projectPath: { type: "string" } } },
		async (args) => textResult(String(await core.codeStatus(args))),
		["standard", "full"],
	),
	tool(
		"xenonite_semantic_search",
		"Search an indexed codebase by meaning through Xenonite's native code engine.",
		{
			type: "object",
			properties: {
				query: { type: "string" },
				projectPath: { type: "string" },
				limit: { type: "number" },
			},
			required: ["query"],
		},
		async (args) => textResult(String(await core.semanticSearch(args))),
		["standard", "full"],
	),
	tool(
		"xenonite_graph_query",
		"Query a file's dependency/symbol graph relationships.",
		{
			type: "object",
			properties: {
				filePath: { type: "string" },
				projectPath: { type: "string" },
			},
			required: ["filePath"],
		},
		async (args) => textResult(String(await core.graphQuery(args))),
		["standard", "full"],
	),
	tool(
		"xenonite_graph_symbol",
		"Look up a symbol definition and references in the code graph.",
		{
			type: "object",
			properties: {
				name: { type: "string" },
				file: { type: "string" },
				projectPath: { type: "string" },
			},
			required: ["name"],
		},
		async (args) => textResult(String(await core.graphSymbol(args))),
		["standard", "full"],
	),
	tool(
		"xenonite_context_bundle",
		"Return Xenonite context artifacts or context search results for external model review.",
		{
			type: "object",
			properties: {
				query: { type: "string", description: "When present, search context artifacts." },
				projectPath: { type: "string" },
				limit: { type: "number" },
			},
		},
		async (args) => {
			return textResult(String(await core.contextBundle(args)));
		},
		["standard", "full"],
	),
];

const writeTools = [
	tool(
		"xenonite_code_op",
		"Run a Xenonite code-engine operation through the unified MCP contract. Intended for amaze compatibility; prefer named tools for external clients.",
		{
			type: "object",
			properties: {
				op: { type: "string" },
				args: { type: "object" },
			},
			required: ["op"],
		},
		async (args) => textResult(String(await core.codeOp(args.op, args.args ?? {}))),
		["full"],
	),
	tool(
		"xenonite_code_index",
		"Start or refresh a Xenonite codebase index. Full mode only because it mutates code-engine state.",
		{
			type: "object",
			properties: {
				projectPath: { type: "string" },
				extraExtensions: { type: "string" },
			},
		},
		async (args) => textResult(String(await core.codeIndex(args))),
		["full"],
	),
	tool(
		"xenonite_code_graph_build",
		"Build the dependency/symbol graph for a project. Full mode only because it mutates code-engine state.",
		{ type: "object", properties: { projectPath: { type: "string" } } },
		async (args) => textResult(String(await core.codeGraphBuild(args))),
		["full"],
	),
	tool(
		"xenonite_memory_store",
		"Store one verified durable memory in Xenonite. Full mode only because it writes durable memory.",
		{
			type: "object",
			properties: {
				text: { type: "string" },
				source: { type: "string" },
				memory_scope: { type: "string" },
				namespace: { type: "string" },
				projectPath: { type: "string" },
			},
			required: ["text"],
		},
		async (args) => textResult(jsonText(await core.storeMemory(args))),
		["full"],
	),
];

const allTools = [...baseTools, ...readOnlyTools, ...writeTools];

export function activeMcpTools(mode = TOOL_MODE) {
	return allTools.filter((candidate) => candidate.modes.includes(mode));
}

export function mcpManifest() {
	return {
		name: "xenonite-mcp",
		title: "Xenonite MCP",
		description: "Xenonite native MCP/App SDK bridge for ChatGPT Pro projects and external MCP clients.",
		toolMode: TOOL_MODE,
		transports: ["stdio", "http-json-rpc"],
		tools: activeMcpTools().map(({ name, description, inputSchema }) => ({ name, description, inputSchema })),
	};
}

function rpcResult(id, result) {
	return { jsonrpc: "2.0", id, result };
}

function rpcError(id, code, message) {
	return { jsonrpc: "2.0", id, error: { code, message } };
}

export async function handleMcpJsonRpc(request) {
	if (!request || typeof request !== "object") return rpcError(null, -32600, "Invalid Request");
	const id = request.id ?? null;
	if (request.method === "initialize") {
		return rpcResult(id, {
			protocolVersion: PROTOCOL_VERSION,
			capabilities: { tools: {} },
			serverInfo: { name: "xenonite-mcp", version: "0.1.0" },
			instructions: "Use Xenonite MCP for project memory, semantic code search, graph queries, and context bundles. Use browser-control legacy paths only for UI automation.",
		});
	}
	if (request.method === "tools/list") {
		return rpcResult(id, { tools: mcpManifest().tools });
	}
	if (request.method === "tools/call") {
		const params = request.params ?? {};
		const toolDef = activeMcpTools().find((candidate) => candidate.name === params.name);
		if (!toolDef) return rpcError(id, -32602, `Unknown tool: ${params.name ?? "(missing)"}`);
		try {
			return rpcResult(id, await toolDef.handler(params.arguments ?? {}));
		} catch (error) {
			return rpcError(id, -32000, String(error?.stack ?? error).slice(0, 800));
		}
	}
	return rpcError(id, -32601, `Method not found: ${request.method ?? "(missing)"}`);
}
