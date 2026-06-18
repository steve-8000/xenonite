#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const server = join(root, "src", "server.mjs");
const major = Number.parseInt(process.versions.node.split(".")[0] ?? "", 10);

if (major === 22) {
	await import(server);
} else {
	const candidates = [
		process.env.XENONITE_NODE22,
		"/opt/homebrew/opt/node@22/bin/node",
		"/usr/local/opt/node@22/bin/node",
		"node22",
	].filter(Boolean);

	let launched = false;
	for (const candidate of candidates) {
		if (candidate.includes("/") && !existsSync(candidate)) continue;
		const result = spawnSync(candidate, [server], {
			cwd: root,
			env: process.env,
			stdio: "inherit",
		});
		if (result.error && result.error.code === "ENOENT") continue;
		launched = true;
		process.exit(result.status ?? (result.signal ? 1 : 0));
	}

	if (!launched) {
		console.error("xenonite: Node 22 is required for the code engine Qdrant client.");
		console.error("Set XENONITE_NODE22=/path/to/node22 or install node@22.");
		process.exit(1);
	}
}
