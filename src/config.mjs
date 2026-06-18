import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

function interpolate(value) {
	if (typeof value === "string") return value.replace(/\$\{([^}]+)\}/g, (_, k) => process.env[k] ?? "");
	return value;
}

function fromTomlIfPresent() {
	const path = process.env.XENONITE_CONFIG ?? join(homedir(), ".config", "xenonite", "xenonite.toml");
	if (!existsSync(path)) return {};
	const raw = readFileSync(path, "utf-8");
	const out = {};
	for (const line of raw.split("\n")) {
		const m = line.match(/^\s*([a-z_]+)\s*=\s*"?([^"#]+?)"?\s*(#.*)?$/i);
		if (m) out[m[1]] = interpolate(m[2].trim());
	}
	return out;
}

const t = fromTomlIfPresent();

export const config = {
	port: Number(process.env.XENONITE_PORT ?? t.port ?? 8700),
	dataDir: process.env.XENONITE_DATA_DIR ?? t.data_dir ?? join(homedir(), ".local", "share", "xenonite"),
	rocky: {
		llmBaseUrl: process.env.ROCKY_LLM_URL ?? t.llm_url ?? "http://127.0.0.1:7777/v1",
		llmModel: process.env.ROCKY_LLM_MODEL ?? t.llm_model ?? "mlx-community/gemma-4-12B-it-qat-4bit",
		llmApiKey: process.env.ROCKY_LLM_KEY ?? t.llm_key ?? "x",
		embedBaseUrl: process.env.ROCKY_EMBED_URL ?? t.embed_url ?? "http://127.0.0.1:7778/v1",
		embedModel: process.env.ROCKY_EMBED_MODEL ?? t.embed_model ?? "default",
		embedApiKey: process.env.ROCKY_EMBED_KEY ?? t.embed_key ?? "x",
	},
};

export function ensureDir(p) {
	if (!existsSync(p)) mkdirSync(p, { recursive: true });
	return p;
}
ensureDir(config.dataDir);
