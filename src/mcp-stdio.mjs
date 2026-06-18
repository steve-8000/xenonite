#!/usr/bin/env node
import { createInterface } from "node:readline";
import { handleMcpJsonRpc } from "./mcp-core.mjs";

async function main() {
	const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
	for await (const line of rl) {
		if (!line.trim()) continue;
		try {
			const response = await handleMcpJsonRpc(JSON.parse(line));
			process.stdout.write(`${JSON.stringify(response)}\n`);
		} catch (error) {
			process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32000, message: String(error?.message ?? error) } })}\n`);
		}
	}
}

main().catch((error) => {
	console.error(error);
	process.exit(1);
});
