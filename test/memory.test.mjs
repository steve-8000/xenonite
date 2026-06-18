import assert from "node:assert/strict";
import test from "node:test";
import { classifyMemoryText, extractFacts, formatMemoryContext, memoryScope, recallScopes, topicKeyForFact } from "../src/memory.mjs";

test("extractFacts keeps only durable bullet-like facts and drops NONE", () => {
	assert.deepEqual(
		extractFacts(`
			- Steve prefers Korean final reports.
			NONE
			2. Memory search must stay bounded.
		`),
		["Steve prefers Korean final reports.", "Memory search must stay bounded."],
	);
});

test("formatMemoryContext returns a bounded retrieved-memory block instead of all items", () => {
	const items = Array.from({ length: 20 }, (_, i) => ({
		text: `memory-${i} ${"x".repeat(80)}`,
	}));

	const context = formatMemoryContext(items, { maxChars: 360 });

	assert.match(context, /^## Retrieved memory\n/);
	assert.ok(context.length <= 360);
	assert.match(context, /memory-0/);
	assert.doesNotMatch(context, /memory-19/);
});

test("classifyMemoryText rejects transient and unverified requirement memories", () => {
	assert.equal(
		classifyMemoryText("Thinking... currently investigating the memory pipeline").accepted,
		false,
	);
	assert.equal(
		classifyMemoryText("Requirement: amaze-search must auto-index cwd", { source: "sync" }).reason,
		"unverified_requirement",
	);
	assert.equal(
		classifyMemoryText("Requirement: amaze-search must auto-index cwd", { source: "verified_durable_fact" }).accepted,
		true,
	);
});

test("topicKeyForFact normalizes prefixes and stable topic tokens", () => {
	assert.equal(
		topicKeyForFact("Verified project fact: amaze-search stores git-state snapshots for code engine sync"),
		topicKeyForFact("amaze-search stores git state snapshots for code engine sync"),
	);
});

test("memoryScope maps path namespace and recallScopes stays strict", () => {
	assert.deepEqual(memoryScope(), { kind: "global", key: "global" });
	assert.deepEqual(
		memoryScope({ memory_scope: "path", namespace: "agent-scope", path_id: "abc", memory_path: "/tmp/project" }),
		{ kind: "project", key: "agent-scope", memoryPath: "/tmp/project" },
	);
	assert.deepEqual(recallScopes({ memory_scope: "path", namespace: "agent-scope" }), [
		{ kind: "project", key: "agent-scope", memoryPath: undefined },
	]);
});
