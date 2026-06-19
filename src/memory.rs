use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    config::Config,
    rocky,
    storage::{memory_collection, stable_id, VectorRecord, WorkScope, ZvecStore},
};

const DEFAULT_TOP_K: usize = 6;
const MAX_TOP_K: usize = 20;
const MAX_CONTEXT_CHARS: usize = 2400;
const MAX_ITEM_CHARS: usize = 360;

#[derive(Debug, Default, Deserialize)]
pub struct MemoryRequest {
    pub query: Option<String>,
    pub text: Option<String>,
    pub source: Option<String>,
    pub top_k: Option<usize>,
    pub memory_scope: Option<String>,
    #[serde(rename = "memoryScope")]
    pub memory_scope_camel: Option<String>,
    pub scope: Option<String>,
    pub namespace: Option<String>,
    pub work_group_id: Option<String>,
    #[serde(rename = "workGroupId")]
    pub work_group_id_camel: Option<String>,
    pub work_unit_id: Option<String>,
    #[serde(rename = "workUnitId")]
    pub work_unit_id_camel: Option<String>,
    pub agent_id: Option<String>,
    #[serde(rename = "agentId")]
    pub agent_id_camel: Option<String>,
    pub path_id: Option<String>,
    #[serde(rename = "pathId")]
    pub path_id_camel: Option<String>,
    pub memory_path: Option<String>,
    #[serde(rename = "memoryPath")]
    pub memory_path_camel: Option<String>,
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    pub path: Option<String>,
    #[serde(rename = "folderPath")]
    pub folder_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StoreResponse {
    pub ok: bool,
    pub added: usize,
    pub items: Vec<MemoryFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecallResponse {
    pub ok: bool,
    pub query: String,
    pub items: Vec<RecallItem>,
    pub context: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct OptimizeMemoryRequest {
    #[serde(rename = "dryRun")]
    pub dry_run: Option<bool>,
    pub apply: Option<bool>,
    #[serde(rename = "maxFacts")]
    pub max_facts: Option<usize>,
    #[serde(rename = "batchSize")]
    pub batch_size: Option<usize>,
    #[serde(rename = "useLlm")]
    pub use_llm: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct OptimizeMemoryResponse {
    pub ok: bool,
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    pub applied: bool,
    pub scanned: usize,
    pub kept: usize,
    pub removed: usize,
    pub moved: usize,
    pub scopes: Vec<OptimizedScope>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OptimizedScope {
    pub scope: String,
    pub before: usize,
    pub after: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    #[serde(rename = "topicKey")]
    pub topic_key: String,
    pub text: String,
    pub source: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Serialize)]
pub struct RecallItem {
    pub id: String,
    pub text: String,
    pub score: f64,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CanonicalIndex {
    version: u8,
    facts: Vec<MemoryFact>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MemoryScope {
    kind: ScopeKind,
    key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopeKind {
    Global,
    Project,
    Path,
}

impl MemoryScope {
    fn storage_key(&self) -> String {
        match self.kind {
            ScopeKind::Global => "global".to_string(),
            ScopeKind::Project => format!("project_{}", self.key),
            ScopeKind::Path => format!("path_{}", self.key),
        }
    }
}

pub async fn store(config: &Config, request: MemoryRequest) -> Result<StoreResponse> {
    let source = request
        .source
        .clone()
        .unwrap_or_else(|| "verified_durable_fact".to_string());
    let Some(classified) = classify_memory_text(request.text.as_deref().unwrap_or(""), &source)
    else {
        return Ok(StoreResponse {
            ok: true,
            added: 0,
            items: vec![],
            skipped: Some("rejected".to_string()),
            reason: Some("empty_or_transient".to_string()),
        });
    };

    let scope = memory_scope(&request);
    let mut index = read_index(config, &scope)?;
    let now = unix_timestamp();

    let existing_pos = index.facts.iter().position(|fact| {
        fact.topic_key == classified.topic_key || token_overlap(&fact.text, &classified.text) >= 0.8
    });

    let id = existing_pos
        .and_then(|pos| index.facts.get(pos).map(|fact| fact.id.clone()))
        .unwrap_or_else(|| format!("canonical:{}", classified.topic_key));
    let created_at = existing_pos
        .and_then(|pos| index.facts.get(pos).map(|fact| fact.created_at))
        .unwrap_or(now);
    let embedding = rocky::embed(&config.rocky, std::slice::from_ref(&classified.text))
        .await
        .ok()
        .and_then(|mut vectors| vectors.pop());
    let fact = MemoryFact {
        id,
        topic_key: classified.topic_key,
        text: classified.text,
        source,
        created_at,
        updated_at: now,
        embedding,
    };

    if let Some(pos) = existing_pos {
        index.facts[pos] = fact.clone();
    } else {
        index.facts.push(fact.clone());
    }
    index.facts.sort_by(|a, b| a.topic_key.cmp(&b.topic_key));
    write_index(config, &scope, &index)?;
    ZvecStore::new(config).upsert_records(
        &memory_collection(&scope.storage_key()),
        &[memory_record(&scope, &request, &fact)],
    )?;

    Ok(StoreResponse {
        ok: true,
        added: 1,
        items: vec![fact],
        skipped: None,
        reason: None,
    })
}

pub async fn recall(config: &Config, request: MemoryRequest) -> Result<RecallResponse> {
    let query = request.query.clone().unwrap_or_default();
    let scope = memory_scope(&request);
    let top_k = request.top_k.unwrap_or(DEFAULT_TOP_K).min(MAX_TOP_K);

    let query_embedding = rocky::embed(&config.rocky, std::slice::from_ref(&query))
        .await
        .ok()
        .and_then(|mut vectors| vectors.pop());
    let mut items: Vec<RecallItem> = ZvecStore::new(config)
        .search(
            &memory_collection(&scope.storage_key()),
            query_embedding.as_deref().unwrap_or(&[]),
            top_k.max(20),
        )
        .unwrap_or_default()
        .into_iter()
        .map(|hit| {
            let fact = MemoryFact {
                id: hit.record.id,
                topic_key: hit.record.topic_key,
                text: hit.record.text,
                source: hit.record.source,
                created_at: hit.record.created_at,
                updated_at: hit.record.updated_at,
                embedding: Some(hit.record.embedding),
            };
            RecallItem {
                id: fact.id.clone(),
                text: fact.text.clone(),
                score: memory_score(&query, &query_embedding, &fact).max(hit.score),
                source: fact.source.clone(),
            }
        })
        .collect();
    if items.is_empty() {
        let index = read_index(config, &scope)?;
        items = index
            .facts
            .iter()
            .map(|fact| RecallItem {
                id: fact.id.clone(),
                text: fact.text.clone(),
                score: memory_score(&query, &query_embedding, fact),
                source: fact.source.clone(),
            })
            .collect();
    }

    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.text.cmp(&b.text))
    });
    items.truncate(top_k);
    let context = format_memory_context(&items, MAX_CONTEXT_CHARS);

    Ok(RecallResponse {
        ok: true,
        query,
        items,
        context,
    })
}

pub async fn optimize(
    config: &Config,
    request: OptimizeMemoryRequest,
) -> Result<OptimizeMemoryResponse> {
    let dry_run = !request.apply.unwrap_or(false) || request.dry_run.unwrap_or(false);
    let max_facts = request.max_facts.unwrap_or(200).max(1);
    let batch_size = request.batch_size.unwrap_or(8).clamp(1, 20);
    let use_llm = request.use_llm.unwrap_or(true);
    let original = load_all_indexes(config)?;
    let scanned = original
        .iter()
        .map(|(_, index)| index.facts.len())
        .sum::<usize>()
        .min(max_facts);
    let mut plans = Vec::new();
    let mut consumed = 0usize;

    for (scope, index) in &original {
        for batch in index.facts.chunks(batch_size) {
            if consumed >= max_facts {
                break;
            }
            let limited = batch
                .iter()
                .take(max_facts - consumed)
                .cloned()
                .collect::<Vec<_>>();
            consumed += limited.len();
            let llm = if use_llm {
                llm_optimize_batch(config, scope, &limited).await.ok()
            } else {
                None
            };
            plans.extend(plan_batch(scope, &limited, llm.as_ref()));
        }
    }

    let mut by_scope: BTreeMap<String, (MemoryScope, Vec<MemoryFact>)> = BTreeMap::new();
    let mut removed = 0usize;
    let mut moved = 0usize;
    for plan in &plans {
        if plan.action == "remove" {
            removed += 1;
            continue;
        }
        if plan.from.storage_key() != plan.to.storage_key() {
            moved += 1;
        }
        by_scope
            .entry(plan.to.storage_key())
            .or_insert_with(|| (plan.to.clone(), Vec::new()))
            .1
            .push(plan.fact.clone());
    }

    let mut scopes = Vec::new();
    for (scope, index) in &original {
        by_scope
            .entry(scope.storage_key())
            .or_insert_with(|| (scope.clone(), Vec::new()));
        scopes.push(OptimizedScope {
            scope: scope.storage_key(),
            before: index.facts.len(),
            after: 0,
        });
    }

    for (scope_key, (_, facts)) in by_scope.iter_mut() {
        dedupe_facts(facts);
        if let Some(scope_summary) = scopes.iter_mut().find(|item| item.scope == *scope_key) {
            scope_summary.after = facts.len();
        } else {
            scopes.push(OptimizedScope {
                scope: scope_key.clone(),
                before: 0,
                after: facts.len(),
            });
        }
    }

    let kept = by_scope.values().map(|(_, facts)| facts.len()).sum();
    if !dry_run {
        rewrite_indexes(config, &original, &by_scope).await?;
    }

    Ok(OptimizeMemoryResponse {
        ok: true,
        dry_run,
        applied: !dry_run,
        scanned,
        kept,
        removed,
        moved,
        scopes,
    })
}

fn read_index(config: &Config, scope: &MemoryScope) -> Result<CanonicalIndex> {
    let path = canonical_path(config, scope);
    if !path.exists() {
        return Ok(CanonicalIndex {
            version: 1,
            facts: vec![],
        });
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn write_index(config: &Config, scope: &MemoryScope, index: &CanonicalIndex) -> Result<()> {
    let dir = scope_dir(config, scope);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let canonical = dir.join("canonical.json");
    fs::write(&canonical, serde_json::to_vec(index)?)
        .with_context(|| format!("write {}", canonical.display()))?;
    fs::write(dir.join("MEMORY.md"), render_memory_md(index))?;
    Ok(())
}

#[derive(Clone, Debug)]
struct MemoryMovePlan {
    from: MemoryScope,
    to: MemoryScope,
    action: String,
    fact: MemoryFact,
}

#[derive(Debug, Deserialize)]
struct LlmOptimizeResponse {
    facts: Vec<LlmFactPlan>,
}

#[derive(Debug, Deserialize)]
struct LlmFactPlan {
    id: String,
    action: Option<String>,
    #[serde(rename = "memoryScope")]
    memory_scope: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    path: Option<String>,
    text: Option<String>,
}

fn load_all_indexes(config: &Config) -> Result<Vec<(MemoryScope, CanonicalIndex)>> {
    let mut indexes = Vec::new();
    let global = MemoryScope {
        kind: ScopeKind::Global,
        key: "global".to_string(),
    };
    indexes.push((global.clone(), read_index(config, &global)?));
    for kind in [ScopeKind::Project, ScopeKind::Path] {
        let dir = match kind {
            ScopeKind::Global => continue,
            ScopeKind::Project => config.data_dir.join("memory").join("projects"),
            ScopeKind::Path => config.data_dir.join("memory").join("paths"),
        };
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let key = entry.file_name().to_string_lossy().to_string();
            let scope = MemoryScope {
                kind: kind.clone(),
                key,
            };
            indexes.push((scope.clone(), read_index(config, &scope)?));
        }
    }
    Ok(indexes)
}

async fn llm_optimize_batch(
    config: &Config,
    scope: &MemoryScope,
    facts: &[MemoryFact],
) -> Result<LlmOptimizeResponse> {
    let system = "You classify durable memory facts. Return only JSON: {\"facts\":[{\"id\":\"...\",\"action\":\"keep|remove\",\"memoryScope\":\"global|project|path\",\"projectPath\":null|string,\"path\":null|string,\"text\":\"clean standalone fact\"}]}. global is only operator preference/style. Project/repo or folder facts must not be global. Remove transient test artifacts, duplicates, todos, temporary session notes.";
    let user = serde_json::json!({
        "currentScope": scope.storage_key(),
        "facts": facts.iter().map(|fact| serde_json::json!({
            "id": fact.id,
            "text": fact.text,
            "source": fact.source,
        })).collect::<Vec<_>>()
    })
    .to_string();
    let raw = rocky::chat_json(&config.rocky, system, &user).await?;
    serde_json::from_str(&raw).with_context(|| format!("parse LLM memory optimize JSON: {raw}"))
}

fn plan_batch(
    scope: &MemoryScope,
    facts: &[MemoryFact],
    llm: Option<&LlmOptimizeResponse>,
) -> Vec<MemoryMovePlan> {
    let llm_by_id = llm
        .map(|response| {
            response
                .facts
                .iter()
                .map(|plan| (plan.id.as_str(), plan))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    facts
        .iter()
        .map(|fact| {
            let llm_plan = llm_by_id.get(fact.id.as_str()).copied();
            let mut normalized = fact.clone();
            if let Some(text) = llm_plan.and_then(|plan| plan.text.as_deref()) {
                if let Some(classified) = classify_memory_text(text, &fact.source) {
                    normalized.text = classified.text;
                    normalized.topic_key = classified.topic_key;
                    normalized.id = format!("canonical:{}", normalized.topic_key);
                    normalized.embedding = None;
                    normalized.updated_at = unix_timestamp();
                }
            }
            let action = inferred_action(&normalized, llm_plan);
            let to = inferred_scope(scope, &normalized, llm_plan);
            MemoryMovePlan {
                from: scope.clone(),
                to,
                action,
                fact: normalized,
            }
        })
        .collect()
}

fn inferred_action(fact: &MemoryFact, llm: Option<&LlmFactPlan>) -> String {
    if let Some(action) = llm.and_then(|plan| plan.action.as_deref()) {
        if matches!(action, "keep" | "remove") {
            return action.to_string();
        }
    }
    let lower = fact.text.to_lowercase();
    if is_transient(&fact.text) || lower.contains("scope-check-") {
        "remove".to_string()
    } else {
        "keep".to_string()
    }
}

fn inferred_scope(
    current: &MemoryScope,
    fact: &MemoryFact,
    llm: Option<&LlmFactPlan>,
) -> MemoryScope {
    let requested = llm.and_then(|plan| plan.memory_scope.as_deref());
    let project_path = llm
        .and_then(|plan| plan.project_path.as_deref())
        .or_else(|| extract_user_path(&fact.text));
    let path = llm.and_then(|plan| plan.path.as_deref()).or(project_path);
    if matches!(requested, Some("path" | "folder")) {
        return MemoryScope {
            kind: ScopeKind::Path,
            key: safe_scope_name(&hash_scope(path.unwrap_or("default"))),
        };
    }
    if matches!(requested, Some("project" | "repo" | "repository"))
        || (current.kind == ScopeKind::Global && fact.source == "verified_durable_fact")
    {
        return MemoryScope {
            kind: ScopeKind::Project,
            key: safe_scope_name(&hash_scope(project_path.unwrap_or("default"))),
        };
    }
    if matches!(requested, Some("global" | "common" | "operator"))
        && fact.source == "direct_user_request"
    {
        return MemoryScope {
            kind: ScopeKind::Global,
            key: "global".to_string(),
        };
    }
    current.clone()
}

fn extract_user_path(text: &str) -> Option<&str> {
    text.split_whitespace()
        .find(|token| token.starts_with("/Users/") || token.starts_with("/host/"))
        .map(|token| token.trim_matches(|c: char| c == ',' || c == '.' || c == ':' || c == ';'))
}

fn dedupe_facts(facts: &mut Vec<MemoryFact>) {
    let mut by_topic = BTreeMap::<String, MemoryFact>::new();
    for fact in facts.drain(..) {
        by_topic
            .entry(fact.topic_key.clone())
            .and_modify(|existing| {
                if fact.updated_at >= existing.updated_at {
                    *existing = fact.clone();
                }
            })
            .or_insert(fact);
    }
    facts.extend(by_topic.into_values());
}

async fn rewrite_indexes(
    config: &Config,
    original: &[(MemoryScope, CanonicalIndex)],
    by_scope: &BTreeMap<String, (MemoryScope, Vec<MemoryFact>)>,
) -> Result<()> {
    let store = ZvecStore::new(config);
    let mut scopes = original
        .iter()
        .map(|(scope, _)| scope.clone())
        .collect::<Vec<_>>();
    scopes.extend(by_scope.values().map(|(scope, _)| scope.clone()));
    scopes.sort_by_key(|scope| scope.storage_key());
    scopes.dedup_by_key(|scope| scope.storage_key());

    for scope in scopes {
        let facts = by_scope
            .get(&scope.storage_key())
            .map(|(_, facts)| facts.clone())
            .unwrap_or_default();
        write_index(
            config,
            &scope,
            &CanonicalIndex {
                version: 1,
                facts: facts.clone(),
            },
        )?;
        store.delete_collection(&memory_collection(&scope.storage_key()))?;
        if facts.is_empty() {
            continue;
        }
        let embeddings = rocky::embed(
            &config.rocky,
            &facts
                .iter()
                .map(|fact| fact.text.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap_or_default();
        let records = facts
            .iter()
            .enumerate()
            .map(|(index, fact)| {
                let mut fact = fact.clone();
                fact.embedding = embeddings.get(index).cloned();
                memory_record(&scope, &MemoryRequest::default(), &fact)
            })
            .collect::<Vec<_>>();
        store.upsert_records(&memory_collection(&scope.storage_key()), &records)?;
    }
    Ok(())
}

fn scope_dir(config: &Config, scope: &MemoryScope) -> PathBuf {
    let memory_root = config.data_dir.join("memory");
    match scope.kind {
        ScopeKind::Global => memory_root,
        ScopeKind::Project => memory_root.join("projects").join(&scope.key),
        ScopeKind::Path => memory_root.join("paths").join(&scope.key),
    }
}

fn canonical_path(config: &Config, scope: &MemoryScope) -> PathBuf {
    scope_dir(config, scope).join("canonical.json")
}

fn memory_scope(request: &MemoryRequest) -> MemoryScope {
    let scope = first_present([
        request.memory_scope.as_deref(),
        request.memory_scope_camel.as_deref(),
        request.scope.as_deref(),
    ]);
    let namespace = first_present([
        request.namespace.as_deref(),
        request.path_id.as_deref(),
        request.path_id_camel.as_deref(),
    ]);
    let path = first_present([
        request.memory_path.as_deref(),
        request.memory_path_camel.as_deref(),
        request.path.as_deref(),
        request.folder_path.as_deref(),
    ]);
    let project = request.project_path.as_deref();

    if matches!(scope, Some("global" | "common" | "operator" | "user")) {
        MemoryScope {
            kind: ScopeKind::Global,
            key: "global".to_string(),
        }
    } else if matches!(scope, Some("path" | "folder")) || namespace.is_some() || path.is_some() {
        let key_source = namespace
            .map(str::to_string)
            .unwrap_or_else(|| hash_scope(path.unwrap_or("default")));
        MemoryScope {
            kind: ScopeKind::Path,
            key: safe_scope_name(&key_source),
        }
    } else if matches!(scope, Some("project" | "repo" | "repository")) || project.is_some() {
        MemoryScope {
            kind: ScopeKind::Project,
            key: safe_scope_name(&hash_scope(project.unwrap_or("default"))),
        }
    } else {
        MemoryScope {
            kind: ScopeKind::Global,
            key: "global".to_string(),
        }
    }
}

fn memory_record(scope: &MemoryScope, request: &MemoryRequest, fact: &MemoryFact) -> VectorRecord {
    let mut work_scope = match scope.kind {
        ScopeKind::Global => WorkScope::global(),
        ScopeKind::Project | ScopeKind::Path => WorkScope {
            project_id: scope.key.clone(),
            work_group_id: None,
            work_unit_id: None,
            agent_id: None,
        },
    };
    work_scope.work_group_id = first_present([
        request.work_group_id.as_deref(),
        request.work_group_id_camel.as_deref(),
    ])
    .map(str::to_string);
    work_scope.work_unit_id = first_present([
        request.work_unit_id.as_deref(),
        request.work_unit_id_camel.as_deref(),
    ])
    .map(str::to_string);
    work_scope.agent_id = first_present([
        request.agent_id.as_deref(),
        request.agent_id_camel.as_deref(),
    ])
    .map(str::to_string);

    VectorRecord {
        id: fact.id.clone(),
        kind: "memory".to_string(),
        scope: work_scope,
        text: fact.text.clone(),
        source: fact.source.clone(),
        file_path: String::new(),
        relative_path: String::new(),
        language: "memory".to_string(),
        artifact_name: String::new(),
        content_hash: stable_id(&fact.text),
        topic_key: fact.topic_key.clone(),
        start_line: 0,
        end_line: 0,
        created_at: fact.created_at,
        updated_at: fact.updated_at,
        embedding: fact.embedding.clone().unwrap_or_default(),
    }
}

fn first_present<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

struct ClassifiedFact {
    text: String,
    topic_key: String,
}

fn classify_memory_text(text: &str, source: &str) -> Option<ClassifiedFact> {
    let fact = normalize_fact(text);
    if fact.is_empty() || is_transient(text) {
        return None;
    }
    if source != "verified_durable_fact"
        && source != "direct_user_request"
        && fact.to_lowercase().starts_with("requirement:")
    {
        return None;
    }

    let topic_key = topic_key_for_fact(&fact);
    if topic_key.is_empty() {
        return None;
    }

    Some(ClassifiedFact {
        text: fact,
        topic_key,
    })
}

fn normalize_fact(text: &str) -> String {
    let without_prefix = text
        .trim()
        .trim_start_matches(|c: char| c == '-' || c == '*' || c.is_ascii_digit() || c == '.')
        .trim();
    for prefix in [
        "Verified project fact:",
        "Functional requirement:",
        "Requirement:",
        "Memory candidate:",
    ] {
        if without_prefix
            .to_lowercase()
            .starts_with(&prefix.to_lowercase())
        {
            return without_prefix[prefix.len()..].trim().to_string();
        }
    }
    without_prefix
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_transient(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "thinking",
        "investigating",
        "currently",
        "in progress",
        "todo",
        "next step",
        "test result",
        "temporary",
        "transient",
        "이번 세션",
        "진행 중",
        "조사 중",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn topic_key_for_fact(text: &str) -> String {
    let stop_words: HashSet<&str> = [
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
        "verified",
        "project",
        "fact",
        "requirement",
        "memory",
    ]
    .into_iter()
    .collect();

    tokens(text)
        .into_iter()
        .filter(|token| !stop_words.contains(token.as_str()))
        .take(8)
        .collect::<Vec<_>>()
        .join("-")
}

fn token_overlap(a: &str, b: &str) -> f64 {
    let a_tokens: HashSet<String> = tokens(a)
        .into_iter()
        .filter(|token| token.len() > 2)
        .collect();
    let b_tokens: HashSet<String> = tokens(b)
        .into_iter()
        .filter(|token| token.len() > 2)
        .collect();
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }

    let shared = a_tokens.intersection(&b_tokens).count();
    shared as f64 / a_tokens.len().min(b_tokens.len()) as f64
}

fn memory_score(query: &str, query_embedding: &Option<Vec<f32>>, fact: &MemoryFact) -> f64 {
    let lexical = token_overlap(query, &fact.text);
    let vector = query_embedding
        .as_ref()
        .zip(fact.embedding.as_ref())
        .map(|(query, fact)| cosine(query, fact))
        .unwrap_or(0.0);
    if vector > 0.0 {
        (lexical * 0.35) + (vector * 0.65)
    } else {
        lexical
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for i in 0..len {
        let aa = a[i] as f64;
        let bb = b[i] as f64;
        dot += aa * bb;
        na += aa * aa;
        nb += bb * bb;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn format_memory_context(items: &[RecallItem], max_chars: usize) -> String {
    if items.is_empty() {
        return String::new();
    }

    let mut context = "## Retrieved memory\n".to_string();
    for item in items {
        let text = truncate_chars(&item.text, MAX_ITEM_CHARS);
        let line = format!("- {}\n", text);
        if context.len() + line.len() > max_chars {
            break;
        }
        context.push_str(&line);
    }
    context.push_str(
        "\nUse these as bounded retrieved memory. Do not assume the full memory file is in context.",
    );
    truncate_chars(&context, max_chars)
}

fn render_memory_md(index: &CanonicalIndex) -> String {
    let mut lines = vec!["# Xenonite Memory".to_string(), "".to_string()];
    lines.extend(index.facts.iter().map(|fact| format!("- {}", fact.text)));
    lines.push(String::new());
    lines.join("\n")
}

fn safe_scope_name(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(96)
        .collect()
}

fn hash_scope(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())[..24].to_string()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        format_memory_context, inferred_action, inferred_scope, memory_scope, topic_key_for_fact,
        MemoryFact, MemoryRequest, MemoryScope, RecallItem, ScopeKind,
    };

    #[test]
    fn topic_key_normalizes_stable_tokens() {
        assert_eq!(
            topic_key_for_fact("Verified project fact: amaze-search stores git-state snapshots"),
            "amaze-search-stores-git-state-snapshots"
        );
    }

    #[test]
    fn context_is_bounded() {
        let items = (0..20)
            .map(|i| RecallItem {
                id: i.to_string(),
                text: format!("memory-{i} {}", "x".repeat(80)),
                score: 1.0,
                source: "manual".to_string(),
            })
            .collect::<Vec<_>>();

        let context = format_memory_context(&items, 360);

        assert!(context.starts_with("## Retrieved memory\n"));
        assert!(context.len() <= 360);
        assert!(context.contains("memory-0"));
        assert!(!context.contains("memory-19"));
    }

    #[test]
    fn memory_scope_separates_global_project_and_path() {
        let global = MemoryRequest {
            memory_scope: Some("global".to_string()),
            ..Default::default()
        };
        let project = MemoryRequest {
            memory_scope: Some("project".to_string()),
            project_path: Some("/host/repo".to_string()),
            ..Default::default()
        };
        let path = MemoryRequest {
            memory_scope: Some("path".to_string()),
            path: Some("/host/repo/src".to_string()),
            ..Default::default()
        };

        let global_scope = memory_scope(&global);
        let project_scope = memory_scope(&project);
        let path_scope = memory_scope(&path);

        assert_eq!(global_scope.kind, ScopeKind::Global);
        assert_eq!(project_scope.kind, ScopeKind::Project);
        assert_eq!(path_scope.kind, ScopeKind::Path);
        assert_eq!(global_scope.storage_key(), "global");
        assert!(project_scope.storage_key().starts_with("project_"));
        assert!(path_scope.storage_key().starts_with("path_"));
        assert_ne!(project_scope.storage_key(), path_scope.storage_key());
    }

    #[test]
    fn optimizer_moves_verified_global_project_fact_out_of_global() {
        let global = MemoryScope {
            kind: ScopeKind::Global,
            key: "global".to_string(),
        };
        let fact = MemoryFact {
            id: "canonical:repo-fact".to_string(),
            topic_key: "repo-fact".to_string(),
            text: "In /Users/steve/rocky/amaze, memory facts are project-scoped.".to_string(),
            source: "verified_durable_fact".to_string(),
            created_at: 1,
            updated_at: 1,
            embedding: None,
        };

        let target = inferred_scope(&global, &fact, None);

        assert_eq!(target.kind, ScopeKind::Project);
        assert_ne!(target.storage_key(), "global");
    }

    #[test]
    fn optimizer_removes_scope_check_artifacts() {
        let fact = MemoryFact {
            id: "canonical:scope-check".to_string(),
            topic_key: "scope-check".to_string(),
            text: "scope-check-123 global-only operator style".to_string(),
            source: "direct_user_request".to_string(),
            created_at: 1,
            updated_at: 1,
            embedding: None,
        };

        assert_eq!(inferred_action(&fact, None), "remove");
    }
}
