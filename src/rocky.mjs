import { config } from "./config.mjs";

// Thin OpenAI-compatible client for the rocky LLM + embedding servers.

export async function chat(messages, { maxTokens = 512, temperature = 0.2 } = {}) {
	const res = await fetch(`${config.rocky.llmBaseUrl}/chat/completions`, {
		method: "POST",
		headers: { "content-type": "application/json", authorization: `Bearer ${config.rocky.llmApiKey}` },
		body: JSON.stringify({ model: config.rocky.llmModel, messages, max_tokens: maxTokens, temperature }),
	});
	if (!res.ok) throw new Error(`rocky LLM ${res.status}: ${await res.text()}`);
	const data = await res.json();
	return data.choices?.[0]?.message?.content ?? "";
}

export async function embed(input) {
	const inputs = Array.isArray(input) ? input : [input];
	const res = await fetch(`${config.rocky.embedBaseUrl}/embeddings`, {
		method: "POST",
		headers: { "content-type": "application/json", authorization: `Bearer ${config.rocky.embedApiKey}` },
		body: JSON.stringify({ model: config.rocky.embedModel, input: inputs }),
	});
	if (!res.ok) throw new Error(`rocky embed ${res.status}: ${await res.text()}`);
	const data = await res.json();
	return data.data.map((d) => d.embedding);
}

export async function rockyHealth() {
	try {
		const r = await fetch(`${config.rocky.llmBaseUrl}/models`, { signal: AbortSignal.timeout(4000) });
		return r.ok;
	} catch {
		return false;
	}
}
