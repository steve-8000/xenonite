import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { config, ensureDir } from "./config.mjs";
import { chat, embed } from "./rocky.mjs";
import { VectorStore } from "./store.mjs";

// Honcho-style memory:
//  - durable observations live in a human-readable MEMORY.md plus a vector index.
//  - recall returns a bounded "working context" blended from semantic hits and
//    recent observations. The full MEMORY.md is never injected into the prompt.
//  - each turn can add new durable observations through a small extraction pass.

const memoryRoot = ensureDir(join(config.dataDir, "memory"));
const storeCache = new Map();

function safeScopeName(value) {
	const raw = String(value ?? "").trim();
	if (!raw) return "default";
	return raw.replace(/[^a-zA-Z0-9_.-]/g, "_").slice(0, 96);
}

function hashScope(value) {
	return createHash("sha256").update(String(value ?? "")).digest("hex").slice(0, 24);
}

export function memoryScope(options = {}) {
	const scope = options.memory_scope ?? options.memoryScope ?? options.scope;
	const namespace = options.namespace ?? options.path_id ?? options.pathId;
	const path = options.memory_path ?? options.memoryPath ?? options.projectPath;
	if (scope === "path" || scope === "project" || namespace || path) {
		return {
			kind: "project",
			key: safeScopeName(namespace || hashScope(path || "default")),
			memoryPath: path,
		};
	}
	return { kind: "global", key: "global" };
}

function scopeStoreName(scope) {
	return scope.kind === "global" ? "memory" : `memory__project__${scope.key}`;
}

function scopePaths(scope) {
	const dir = scope.kind === "global" ? memoryRoot : ensureDir(join(memoryRoot, "projects", scope.key));
	return {
		dir,
		memoryMd: join(dir, "MEMORY.md"),
		canonicalJson: join(dir, "canonical.json"),
	};
}

function storeForScope(scope) {
	const name = scopeStoreName(scope);
	ensureDir(join(config.dataDir, "vectors"));
	if (!storeCache.has(name)) storeCache.set(name, new VectorStore(name));
	return storeCache.get(name);
}

export function recallScopes(options = {}) {
	const requested = memoryScope(options);
	return [requested];
}

export const MEMORY_LIMITS = {
	defaultTopK: 6,
	maxTopK: 20,
	recentFallback: 3,
	scoreThreshold: 0.25,
	maxItemChars: 360,
	maxContextChars: 2400,
};

const SOURCE_PRIORITY = {
	sync: 1,
	manual: 2,
	direct_user_request: 3,
	verified_durable_fact: 4,
};

function readMd(scope = memoryScope()) {
	const { memoryMd } = scopePaths(scope);
	return existsSync(memoryMd) ? readFileSync(memoryMd, "utf-8") : "";
}

function normalizeFact(text) {
	return String(text ?? "")
		.replace(/^(verified project fact|functional requirement|requirement|memory candidate)\s*:\s*/i, "")
		.replace(/^[-*\d.\s]+/, "")
		.replace(/\s+/g, " ")
		.trim();
}

function normalizeForCompare(text) {
	return normalizeFact(text)
		.toLowerCase()
		.replace(/[^\p{L}\p{N}\s]/gu, " ")
		.replace(/\s+/g, " ")
		.trim();
}

function tokenSet(text) {
	return new Set(
		normalizeForCompare(text)
			.split(" ")
			.filter((token) => token.length > 2),
	);
}

function tokenOverlap(a, b) {
	const aa = tokenSet(a);
	const bb = tokenSet(b);
	if (!aa.size || !bb.size) return 0;
	let shared = 0;
	for (const token of aa) if (bb.has(token)) shared++;
	return shared / Math.min(aa.size, bb.size);
}

export function topicKeyForFact(text) {
	const tokens = [...tokenSet(text)].filter(
		(token) =>
			![
				"the",
				"and",
				"for",
				"with",
				"that",
				"this",
				"must",
				"should",
				"uses",
				"use",
				"using",
				"automatically",
				"verified",
				"project",
				"fact",
				"requirement",
				"system",
			].includes(token),
	);
	return tokens.slice(0, 8).join("-");
}

export function classifyMemoryText(text, options = {}) {
	const raw = String(text ?? "").trim();
	const fact = normalizeFact(raw);
	if (!fact) return { accepted: false, reason: "empty", fact, topicKey: "" };
	const source = options.source ?? "manual";
	const lowerRaw = raw.toLowerCase();
	const lowerFact = fact.toLowerCase();
	const transientPatterns = [
		/\b(assistant|thinking|investigating|currently|in progress|todo|next step|next test objective|test result|cleanup|will be regenerated|will start|will be|plans to|restart|rebuild|compressed into|only canonical facts|temporary|transient|이번 세션|진행 중|조사 중|재빌드|정리|압축)\b/i,
		/\b(memory-store|memory-search|tool result|thinking\.\.\.)\b/i,
		/\b(user wants|the user wants|there is a known issue|failed because|finished normally|terminated without)\b/i,
		/\b(plan|scope|review findings|severity):\s/i,
	];
	if (transientPatterns.some((pattern) => pattern.test(raw))) {
		return { accepted: false, reason: "transient", fact, topicKey: "" };
	}
	if (
		source !== "verified_durable_fact" &&
		source !== "direct_user_request" &&
		(lowerRaw.startsWith("requirement:") ||
			lowerRaw.startsWith("functional requirement:") ||
			lowerFact.startsWith("requirement "))
	) {
		return { accepted: false, reason: "unverified_requirement", fact, topicKey: "" };
	}
	const topicKey = topicKeyForFact(fact);
	if (!topicKey) return { accepted: false, reason: "no_topic_key", fact, topicKey };
	return { accepted: true, reason: "accepted", fact, topicKey };
}

export function extractFacts(reply) {
	return String(reply ?? "")
		.split("\n")
		.map(normalizeFact)
		.filter((line) => line && line.toUpperCase() !== "NONE")
		.slice(0, 5);
}

function readFactSet(scope = memoryScope()) {
	return new Set(
		loadCanonicalIndex(scope).facts.map((fact) => normalizeForCompare(fact.text)),
	);
}

function readCanonicalIndex(scope = memoryScope()) {
	const { canonicalJson } = scopePaths(scope);
	if (!existsSync(canonicalJson)) return { version: 1, facts: [] };
	try {
		const parsed = JSON.parse(readFileSync(canonicalJson, "utf-8"));
		return {
			version: 1,
			facts: Array.isArray(parsed.facts) ? parsed.facts : [],
		};
	} catch {
		return { version: 1, facts: [] };
	}
}

function migrateMarkdownMemory(scope = memoryScope()) {
	const facts = extractFacts(readMd(scope));
	if (!facts.length) return { version: 1, facts: [] };
	const index = { version: 1, facts: [] };
	for (const fact of facts) {
		const classification = classifyMemoryText(fact, { source: "manual" });
		if (!classification.accepted) continue;
		const existing = findSimilarFact(index, classification.fact, classification.topicKey);
		const record = {
			id: canonicalId(existing?.topicKey ?? classification.topicKey),
			topicKey: existing?.topicKey ?? classification.topicKey,
			text: chooseCanonicalText(existing, classification.fact, "manual"),
			source: "manual",
			createdAt: existing?.createdAt ?? new Date(0).toISOString(),
			updatedAt: new Date().toISOString(),
			provenance: [
				...(existing?.provenance ?? []),
				{ source: "manual", text: classification.fact, ts: Date.now(), migrated: true },
			].slice(-20),
		};
		index.facts = existing
			? index.facts.map((item) => (item.id === existing.id ? record : item))
			: [...index.facts, record];
	}
	index.facts.sort((a, b) => a.topicKey.localeCompare(b.topicKey));
	writeCanonicalIndex(scope, index);
	renderMemoryMd(scope, index);
	return index;
}

function loadCanonicalIndex(scope = memoryScope()) {
	const { canonicalJson } = scopePaths(scope);
	if (!existsSync(canonicalJson) && readMd(scope).trim()) return migrateMarkdownMemory(scope);
	return readCanonicalIndex(scope);
}

function writeCanonicalIndex(scope, index) {
	const { dir, canonicalJson } = scopePaths(scope);
	ensureDir(dir);
	writeFileSync(canonicalJson, `${JSON.stringify({ version: 1, facts: index.facts }, null, 2)}\n`);
}

function renderMemoryMd(scope, index) {
	const { dir, memoryMd } = scopePaths(scope);
	ensureDir(dir);
	const lines = ["# MEMORY", "", ...index.facts.map((fact) => `- ${fact.text}`), ""];
	writeFileSync(memoryMd, lines.join("\n"));
}

function canonicalId(topicKey) {
	return `canonical:${topicKey}`;
}

function findSimilarFact(index, fact, topicKey) {
	const normalized = normalizeForCompare(fact);
	for (const existing of index.facts) {
		const existingNormalized = normalizeForCompare(existing.text);
		if (existing.topicKey === topicKey) return existing;
		if (existingNormalized === normalized) return existing;
		if (existingNormalized.includes(normalized) || normalized.includes(existingNormalized)) return existing;
		if (tokenOverlap(existing.text, fact) >= 0.82) return existing;
	}
	return undefined;
}

function chooseCanonicalText(existing, incoming, source) {
	if (!existing) return incoming;
	const incomingPriority = SOURCE_PRIORITY[source] ?? SOURCE_PRIORITY.manual;
	const existingPriority = SOURCE_PRIORITY[existing.source] ?? SOURCE_PRIORITY.manual;
	if (incomingPriority > existingPriority) return incoming;
	if (incomingPriority === existingPriority && incoming.length > existing.text.length) return incoming;
	return existing.text;
}

async function upsertCanonicalFact(text, options = {}) {
	const source = options.source ?? "manual";
	const scope = memoryScope(options);
	const classification = classifyMemoryText(text, { source });
	if (!classification.accepted) {
		return {
			action: "rejected",
			reason: classification.reason,
			added: 0,
			items: [],
			skipped: classification.fact ? [classification.fact] : [],
		};
	}

	const now = Date.now();
	const index = loadCanonicalIndex(scope);
	const existing = findSimilarFact(index, classification.fact, classification.topicKey);
	const canonicalText = chooseCanonicalText(existing, classification.fact, source);
	const topicKey = existing?.topicKey ?? classification.topicKey;
	const id = canonicalId(topicKey);
	const provenance = [
		...(existing?.provenance ?? []),
		{
			source,
			sessionId: options.sessionId,
			text: classification.fact,
			ts: now,
		},
	].slice(-20);
	const record = {
		id,
		topicKey,
		text: canonicalText,
		source: (SOURCE_PRIORITY[source] ?? 0) >= (SOURCE_PRIORITY[existing?.source] ?? 0) ? source : existing?.source ?? source,
		createdAt: existing?.createdAt ?? new Date(now).toISOString(),
		updatedAt: new Date(now).toISOString(),
		provenance,
	};
	const nextFacts = existing
		? index.facts.map((fact) => (fact.id === existing.id ? record : fact))
		: [...index.facts, record];
	const nextIndex = { version: 1, facts: nextFacts.sort((a, b) => a.topicKey.localeCompare(b.topicKey)) };
	writeCanonicalIndex(scope, nextIndex);
	renderMemoryMd(scope, nextIndex);
	const [vector] = await embed(record.text);
	const scopedStore = storeForScope(scope);
	scopedStore.remove((item) => item.id === id || item.meta?.topicKey === topicKey);
	scopedStore.add(id, record.text, vector, { source: record.source, sessionId: options.sessionId, topicKey, memoryScope: scope.kind, scopeKey: scope.key });
	const item = { id, text: record.text, source: record.source, ts: now, meta: { source: record.source, sessionId: options.sessionId, topicKey, memoryScope: scope.kind, scopeKey: scope.key } };
	return {
		action: existing ? "updated" : "added",
		reason: existing ? "canonical_upsert" : "new_canonical_fact",
		added: existing ? 0 : 1,
		items: [item],
		topicKey,
		context: formatMemoryContext([item], { title: existing ? "Updated memory" : "Stored memory" }),
	};
}

function clipText(text, maxChars = MEMORY_LIMITS.maxItemChars) {
	const normalized = String(text ?? "").replace(/\s+/g, " ").trim();
	if (normalized.length <= maxChars) return normalized;
	return `${normalized.slice(0, Math.max(0, maxChars - 1)).trimEnd()}…`;
}

function normalizeTopK(topK) {
	const n = Number(topK);
	if (!Number.isFinite(n)) return MEMORY_LIMITS.defaultTopK;
	return Math.min(MEMORY_LIMITS.maxTopK, Math.max(1, Math.floor(n)));
}

function toMemoryItem(hit, source) {
	return {
		id: hit.id,
		text: clipText(hit.text),
		score: typeof hit.score === "number" ? Number(hit.score.toFixed(4)) : undefined,
		source,
		ts: hit.ts,
		meta: hit.meta,
	};
}

function dedupeItems(items) {
	const seen = new Set();
	const out = [];
	for (const item of items) {
		const key = item.meta?.topicKey ?? topicKeyForFact(item.text) ?? item.id ?? item.text.toLowerCase();
		if (seen.has(key)) continue;
		seen.add(key);
		out.push(item);
	}
	return out;
}

function recentItems(scope, limit, excludeIds = new Set()) {
	return storeForScope(scope)
		.all()
		.filter((item) => !excludeIds.has(item.id))
		.sort((a, b) => (b.ts ?? 0) - (a.ts ?? 0))
		.slice(0, limit)
		.map((item) => toMemoryItem(item, "recent"));
}

export function formatMemoryContext(items, options = {}) {
	const title = options.title ?? "Retrieved memory";
	const maxChars = options.maxChars ?? MEMORY_LIMITS.maxContextChars;
	const lines = [`## ${title}`];
	let usedChars = lines[0].length;

	for (const item of items) {
		const line = `- ${item.text}`;
		if (usedChars + line.length + 1 > maxChars) break;
		lines.push(line);
		usedChars += line.length + 1;
	}

	return lines.length > 1 ? lines.join("\n") : "";
}

const EXTRACT_PROMPT = `You maintain an agent's long-term memory. Given the latest exchange, output durable facts worth remembering across sessions (preferences, decisions, stable project facts, identities). Output 0-5 short bullet lines, each a standalone fact. If nothing is durable, output exactly NONE.`;

export async function syncTurn(userContent, assistantContent, options = {}) {
	return { ok: true, added: 0, items: [], skipped: [], reason: "auto_sync_disabled" };

	const reply = await chat(
		[
			{ role: "system", content: EXTRACT_PROMPT },
			{ role: "user", content: `User: ${userContent}\nAssistant: ${assistantContent}` },
		],
		{ maxTokens: 256 },
	).catch(() => "NONE");

	const existingFacts = readFactSet(memoryScope(options));
	const facts = extractFacts(reply).filter((fact) => !existingFacts.has(normalizeForCompare(fact)));
	if (!facts.length) return { ok: true, added: 0, items: [] };

	const items = [];
	let added = 0;
	const skipped = [];
	for (const fact of facts) {
		const result = await upsertCanonicalFact(fact, { ...options, source: "sync" });
		added += result.added;
		items.push(...result.items);
		if (result.skipped) skipped.push(...result.skipped);
	}
	return { ok: true, added, items, skipped, context: formatMemoryContext(items, { title: "Stored memory" }) };
}

export async function prefetch(query, options = {}) {
	const scopes = recallScopes(options);
	const scopedStores = scopes.map((scope) => ({ scope, store: storeForScope(scope) }));
	const all = scopedStores.flatMap(({ store: scopedStore }) => scopedStore.all());
	const topK = normalizeTopK(options.topK ?? options.top_k);
	if (!all.length) return { ok: true, query, context: "", items: [], totalCandidates: 0 };

	const semanticItems = [];
	const trimmedQuery = String(query ?? "").trim();
	if (trimmedQuery) {
		const [qv] = await embed(trimmedQuery);
		semanticItems.push(
			...scopedStores.flatMap(({ store: scopedStore }) => scopedStore
				.search(qv, topK)
				.filter((hit) => hit.score > (options.scoreThreshold ?? MEMORY_LIMITS.scoreThreshold))
				.map((hit) => toMemoryItem(hit, "semantic"))),
		);
	}

	const usedIds = new Set(semanticItems.map((item) => item.id).filter(Boolean));
	const remainingSlots = Math.max(0, topK - semanticItems.length);
	const fallbackCount = trimmedQuery ? Math.min(MEMORY_LIMITS.recentFallback, remainingSlots) : topK;
	const canonicalByTopic = new Map(scopes.flatMap((scope) => loadCanonicalIndex(scope).facts.map((fact) => [fact.topicKey, fact])));
	const fallbackItems = scopes.flatMap((scope) => recentItems(scope, fallbackCount, usedIds));
	const items = dedupeItems([...semanticItems, ...fallbackItems])
		.map((item) => {
			const topicKey = item.meta?.topicKey ?? topicKeyForFact(item.text);
			const canonical = canonicalByTopic.get(topicKey);
			return canonical ? { ...item, id: canonical.id, text: canonical.text, meta: { ...(item.meta ?? {}), topicKey } } : item;
		})
		.slice(0, topK);
	const context = formatMemoryContext(items);
	return {
		ok: true,
		query: trimmedQuery,
		context,
		items,
		totalCandidates: all.length,
		semanticCount: semanticItems.length,
		recentCount: items.filter((item) => item.source === "recent").length,
	};
}

export function systemPromptBlock() {
	if (!loadCanonicalIndex(memoryScope()).facts.length) return "";
	return [
		"## Persistent memory",
		"Durable memory is available through Xenonite. The runtime retrieves a bounded working context for each prompt and the `mem_recall`/`mem_search` tools can fetch focused semantic matches. Do not assume the full memory file is in context.",
	].join("\n");
}

export async function manualStore(text, options = {}) {
	const result = await upsertCanonicalFact(text, { ...options, source: options.source ?? "manual" });
	if (result.action === "rejected") {
		return { ok: true, added: 0, items: [], skipped: result.skipped, reason: result.reason };
	}
	return { ok: true, added: result.added, items: result.items, skipped: result.added ? [] : result.items.map((item) => item.text), action: result.action, reason: result.reason, topicKey: result.topicKey, context: result.context };
}
