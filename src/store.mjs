import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { config, ensureDir } from "./config.mjs";

// Minimal file-backed vector store with cosine similarity. No external DB:
// memory facts are few and small, so an in-process scan is sufficient and
// keeps Xenonite dependency-free for the memory path.

function cosine(a, b) {
	let dot = 0;
	let na = 0;
	let nb = 0;
	for (let i = 0; i < a.length; i++) {
		dot += a[i] * b[i];
		na += a[i] * a[i];
		nb += b[i] * b[i];
	}
	return na && nb ? dot / (Math.sqrt(na) * Math.sqrt(nb)) : 0;
}

export class VectorStore {
	constructor(name) {
		this.path = join(ensureDir(join(config.dataDir, "vectors")), `${name}.json`);
		this.items = existsSync(this.path) ? JSON.parse(readFileSync(this.path, "utf-8")) : [];
	}

	add(id, text, vector, meta = {}) {
		this.items = this.items.filter((it) => it.id !== id);
		this.items.push({ id, text, vector, meta, ts: Date.now() });
		this.flush();
	}

	remove(predicate) {
		const before = this.items.length;
		this.items = this.items.filter((it) => !predicate(it));
		this.flush();
		return before - this.items.length;
	}

	search(queryVector, topK = 5) {
		return this.items
			.map((it) => ({ ...it, score: cosine(queryVector, it.vector) }))
			.sort((a, b) => b.score - a.score)
			.slice(0, topK);
	}

	all() {
		return this.items;
	}

	flush() {
		writeFileSync(this.path, JSON.stringify(this.items));
	}
}
