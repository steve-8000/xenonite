import { config } from "./config.mjs";
import { callCodeOp } from "./code.mjs";
import * as memory from "./memory.mjs";
import { rockyHealth } from "./rocky.mjs";

export function memoryOptions(args = {}) {
	return {
		sessionId: args.session_id,
		memory_scope: args.memory_scope,
		memoryScope: args.memoryScope,
		scope: args.scope,
		namespace: args.namespace,
		path_id: args.path_id,
		pathId: args.pathId,
		memory_path: args.memory_path,
		memoryPath: args.memoryPath,
		projectPath: args.projectPath,
		source: args.source,
	};
}

export async function serviceConfig({ includeRockyHealth = true } = {}) {
	const health = includeRockyHealth ? await rockyHealth().catch((error) => ({ error: String(error) })) : undefined;
	return {
		name: "xenonite-mcp",
		service: "xenonite",
		port: config.port,
		dataDir: config.dataDir,
		rocky: {
			llmBaseUrl: config.rocky.llmBaseUrl,
			embedBaseUrl: config.rocky.embedBaseUrl,
			health,
		},
	};
}

export async function health() {
	return {
		ok: true,
		service: "xenonite",
		rocky: await rockyHealth().catch((error) => ({ error: String(error) })),
	};
}

export async function recallMemory(args = {}) {
	return await memory.prefetch(String(args.query ?? ""), { ...memoryOptions(args), topK: Number(args.top_k ?? 6) });
}

export async function storeMemory(args = {}) {
	return await memory.manualStore(String(args.text ?? ""), {
		...memoryOptions(args),
		source: args.source ?? "verified_durable_fact",
	});
}

export async function codeStatus(args = {}) {
	return await callCodeOp("codebase_status", args);
}

export async function semanticSearch(args = {}) {
	return await callCodeOp("codebase_search", args);
}

export async function graphQuery(args = {}) {
	return await callCodeOp("codebase_graph_query", args);
}

export async function graphSymbol(args = {}) {
	return await callCodeOp("codebase_symbol", args);
}

export async function contextBundle(args = {}) {
	const op = args.query ? "codebase_context_search" : "codebase_context";
	return await callCodeOp(op, args);
}

export async function codeIndex(args = {}) {
	return await callCodeOp("codebase_index", args);
}

export async function codeGraphBuild(args = {}) {
	return await callCodeOp("codebase_graph_build", args);
}

export async function codeOp(op, args = {}) {
	return await callCodeOp(String(op ?? ""), args && typeof args === "object" ? args : {});
}
