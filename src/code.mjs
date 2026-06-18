import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createJiti } from "jiti";

// Code engine adapter: Xenonite owns socraticode (engine/socraticode) and
// exposes it only through Xenonite core/MCP tools. There is no public /v1/code
// REST surface; external clients should use the MCP contract.

const here = dirname(fileURLToPath(import.meta.url));
const socRoot = join(here, "..", "engine", "socraticode");

// socraticode reads configuration from env at module load.
process.env.QDRANT_MODE ??= "managed";
process.env.QDRANT_HOST ??= "localhost";
process.env.QDRANT_PORT ??= "16333";
process.env.QDRANT_GRPC_PORT ??= "16334";
process.env.EMBEDDING_PROVIDER ??= "ollama";
process.env.EMBEDDING_MODEL ??= "nomic-embed-text";
process.env.EMBEDDING_DIMENSIONS ??= "768";
process.env.OLLAMA_MODE ??= "auto";

const jiti = createJiti(import.meta.url, { moduleCache: true });

async function loadHandlers() {
	const t = join(socRoot, "src", "tools");
	const [index, query, graph, context, manage] = await Promise.all([
		jiti.import(join(t, "index-tools.ts")),
		jiti.import(join(t, "query-tools.ts")),
		jiti.import(join(t, "graph-tools.ts")),
		jiti.import(join(t, "context-tools.ts")),
		jiti.import(join(t, "manage-tools.ts")),
	]);
	return {
		Index: index.handleIndexTool,
		Query: query.handleQueryTool,
		Graph: graph.handleGraphTool,
		Context: context.handleContextTool,
		Manage: manage.handleManageTool,
	};
}

let handlersPromise;
const handlers = () => (handlersPromise ??= loadHandlers());

const OP_HANDLER = {
	codebase_index: "Index", codebase_update: "Index", codebase_remove: "Index", codebase_stop: "Index", codebase_watch: "Index",
	codebase_search: "Query", codebase_status: "Query",
	codebase_graph_build: "Graph", codebase_graph_query: "Graph", codebase_graph_stats: "Graph", codebase_graph_circular: "Graph",
	codebase_graph_visualize: "Graph", codebase_graph_remove: "Graph", codebase_graph_status: "Graph",
	codebase_impact: "Graph", codebase_flow: "Graph", codebase_symbol: "Graph", codebase_symbols: "Graph",
	codebase_context: "Context", codebase_context_search: "Context", codebase_context_index: "Context", codebase_context_remove: "Context",
	codebase_health: "Manage", codebase_list_projects: "Manage",
};

export async function callCodeOp(op, args = {}) {
	const hk = OP_HANDLER[op];
	if (!hk) throw new Error(`unknown op: ${op}`);
	const h = await handlers();
	return await h[hk](op, args);
}
