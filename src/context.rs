use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    code_index::resolve_project_path,
    config::Config,
    rocky,
    storage::{context_collection, VectorRecord, WorkScope, ZvecStore},
};

const CONFIG_FILENAME: &str = ".xenonitecontextartifacts.json";
const CHUNK_SIZE: usize = 100;
const CHUNK_OVERLAP: usize = 10;
const MAX_CHUNK_CHARS: usize = 32_000;

#[derive(Debug, Deserialize)]
pub struct ContextRequest {
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    #[serde(rename = "minScore")]
    pub min_score: Option<f64>,
    #[serde(rename = "artifactName")]
    pub artifact_name: Option<String>,
    pub work_group_id: Option<String>,
    #[serde(rename = "workGroupId")]
    pub work_group_id_camel: Option<String>,
    pub work_unit_id: Option<String>,
    #[serde(rename = "workUnitId")]
    pub work_unit_id_camel: Option<String>,
    pub agent_id: Option<String>,
    #[serde(rename = "agentId")]
    pub agent_id_camel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContextConfig {
    artifacts: Option<Vec<ContextArtifact>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ContextArtifact {
    name: String,
    path: String,
    description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArtifactChunk {
    id: String,
    content: String,
    #[serde(rename = "startLine")]
    start_line: usize,
    #[serde(rename = "endLine")]
    end_line: usize,
    #[serde(rename = "artifactName")]
    artifact_name: String,
    #[serde(rename = "artifactPath")]
    artifact_path: String,
    description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ContextIndex {
    version: u8,
    #[serde(rename = "projectPath")]
    project_path: String,
    #[serde(rename = "updatedAt")]
    updated_at: u64,
    artifacts: Vec<ContextArtifactState>,
    chunks: Vec<ArtifactChunk>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextArtifactState {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "resolvedPath")]
    pub resolved_path: String,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
    #[serde(rename = "lastIndexedAt")]
    pub last_indexed_at: u64,
    #[serde(rename = "chunksIndexed")]
    pub chunks_indexed: usize,
}

#[derive(Debug, Serialize)]
pub struct ContextIndexResponse {
    pub ok: bool,
    pub indexed: Vec<ContextArtifactState>,
    pub errors: Vec<ContextError>,
}

#[derive(Debug, Serialize)]
pub struct ContextError {
    pub name: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ContextSearchResponse {
    pub ok: bool,
    pub query: String,
    pub results: Vec<ContextSearchResult>,
    pub context: String,
}

#[derive(Debug, Serialize)]
pub struct ContextSearchResult {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    pub content: String,
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "endLine")]
    pub end_line: usize,
    pub language: String,
    pub score: f64,
    #[serde(rename = "artifactName")]
    pub artifact_name: String,
}

#[derive(Debug, Serialize)]
pub struct ContextRemoveResponse {
    pub ok: bool,
    pub removed: bool,
}

pub async fn index(config: &Config, request: ContextRequest) -> Result<ContextIndexResponse> {
    let project_path = resolve_project_path(request.project_path.clone())?;
    let context_config = load_context_config(&project_path)?;
    let Some(artifacts) = context_config.and_then(|config| config.artifacts) else {
        let index = ContextIndex {
            version: 1,
            project_path: project_path.display().to_string(),
            updated_at: unix_timestamp(),
            artifacts: vec![],
            chunks: vec![],
        };
        write_index(config, &project_path, &index)?;
        return Ok(ContextIndexResponse {
            ok: true,
            indexed: vec![],
            errors: vec![],
        });
    };

    let mut indexed = Vec::new();
    let mut errors = Vec::new();
    let mut chunks = Vec::new();
    for artifact in artifacts {
        match index_artifact(&project_path, &artifact) {
            Ok((state, mut artifact_chunks)) => {
                indexed.push(state);
                chunks.append(&mut artifact_chunks);
            }
            Err(error) => errors.push(ContextError {
                name: artifact.name,
                error: error.to_string(),
            }),
        }
    }
    let index = ContextIndex {
        version: 1,
        project_path: project_path.display().to_string(),
        updated_at: unix_timestamp(),
        artifacts: indexed,
        chunks,
    };
    write_index(config, &project_path, &index)?;
    let texts = index
        .chunks
        .iter()
        .map(|chunk| chunk.content.clone())
        .collect::<Vec<_>>();
    let embeddings = rocky::embed(&config.rocky, &texts)
        .await
        .unwrap_or_default();
    let records = index
        .chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            context_record(
                &project_path,
                &request,
                chunk,
                embeddings.get(i).cloned().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    let store = ZvecStore::new(config);
    store.delete_collection(&context_collection(&project_path))?;
    store.upsert_records(&context_collection(&project_path), &records)?;
    let Some(index) = read_index(config, &project_path)? else {
        bail!("context index was written but could not be read back");
    };
    Ok(ContextIndexResponse {
        ok: true,
        indexed: index.artifacts,
        errors,
    })
}

pub async fn search(config: &Config, request: ContextRequest) -> Result<ContextSearchResponse> {
    let project_path = resolve_project_path(request.project_path.clone())?;
    let query = request.query.clone().unwrap_or_default();
    let limit = request.limit.unwrap_or(10).clamp(1, 50);
    let min_score = request.min_score.unwrap_or(0.0).clamp(0.0, 1.0);
    let artifact_filter = request.artifact_name.clone();
    if read_index(config, &project_path)?.is_none() {
        let _ = index(config, request).await?;
    }
    let index = read_index(config, &project_path)?.unwrap_or(ContextIndex {
        version: 1,
        project_path: project_path.display().to_string(),
        updated_at: unix_timestamp(),
        artifacts: vec![],
        chunks: vec![],
    });
    let query_tokens = tokens(&query);
    let query_embedding = rocky::embed(&config.rocky, std::slice::from_ref(&query))
        .await
        .ok()
        .and_then(|mut vectors| vectors.pop());
    let mut results = ZvecStore::new(config)
        .search(
            &context_collection(&project_path),
            query_embedding.as_deref().unwrap_or(&[]),
            limit.max(50),
        )
        .unwrap_or_default()
        .into_iter()
        .filter(|hit| {
            artifact_filter
                .as_ref()
                .is_none_or(|name| &hit.record.artifact_name == name)
        })
        .filter_map(|hit| {
            let chunk = record_as_artifact_chunk(&hit.record);
            let lexical = lexical_score(&query_tokens, &chunk);
            let score = (lexical * 0.35) + (hit.score * 0.65);
            (score > 0.0 && score >= min_score).then(|| ContextSearchResult {
                file_path: hit.record.file_path,
                relative_path: hit.record.relative_path,
                content: hit.record.text,
                start_line: hit.record.start_line,
                end_line: hit.record.end_line,
                language: "context".to_string(),
                score,
                artifact_name: hit.record.artifact_name,
            })
        })
        .collect::<Vec<_>>();

    if results.is_empty() {
        results = index
            .chunks
            .iter()
            .filter(|chunk| {
                artifact_filter
                    .as_ref()
                    .is_none_or(|name| &chunk.artifact_name == name)
            })
            .filter_map(|chunk| {
                let score = lexical_score(&query_tokens, chunk);
                (score > 0.0 && score >= min_score).then(|| ContextSearchResult {
                    file_path: chunk.artifact_path.clone(),
                    relative_path: chunk.artifact_path.clone(),
                    content: chunk.content.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    language: "context".to_string(),
                    score,
                    artifact_name: chunk.artifact_name.clone(),
                })
            })
            .collect::<Vec<_>>();
    }
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    results.truncate(limit);

    let context = format_context_results(&results);

    Ok(ContextSearchResponse {
        ok: true,
        query,
        results,
        context,
    })
}

pub async fn bundle(config: &Config, request: ContextRequest) -> Result<ContextSearchResponse> {
    search(config, request).await
}

pub fn remove(config: &Config, request: ContextRequest) -> Result<ContextRemoveResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let path = index_path(config, &project_path);
    let removed = if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        let _ = ZvecStore::new(config).delete_collection(&context_collection(&project_path));
        true
    } else {
        false
    };
    Ok(ContextRemoveResponse { ok: true, removed })
}

fn load_context_config(project_path: &Path) -> Result<Option<ContextConfig>> {
    let path = project_path.join(CONFIG_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("parse {}", path.display()))
}

fn index_artifact(
    project_path: &Path,
    artifact: &ContextArtifact,
) -> Result<(ContextArtifactState, Vec<ArtifactChunk>)> {
    let resolved_path = if Path::new(&artifact.path).is_absolute() {
        PathBuf::from(&artifact.path)
    } else {
        project_path.join(&artifact.path)
    };
    let content = read_artifact_content(&resolved_path)?;
    let content_hash = hash(&content);
    let chunks = chunk_artifact_content(&content, artifact, &resolved_path);
    let state = ContextArtifactState {
        name: artifact.name.clone(),
        description: artifact.description.clone(),
        resolved_path: resolved_path.display().to_string(),
        content_hash,
        last_indexed_at: unix_timestamp(),
        chunks_indexed: chunks.len(),
    };
    Ok((state, chunks))
}

fn read_artifact_content(path: &Path) -> Result<String> {
    if path.is_file() {
        return fs::read_to_string(path).with_context(|| format!("read {}", path.display()));
    }
    if path.is_dir() {
        let mut content = String::new();
        read_artifact_dir(path, path, &mut content)?;
        return Ok(content);
    }
    bail!("artifact path does not exist: {}", path.display())
}

fn read_artifact_dir(root: &Path, dir: &Path, content: &mut String) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            read_artifact_dir(root, &path, content)?;
        } else if entry.file_type()?.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path).display();
            if let Ok(file_content) = fs::read_to_string(&path) {
                content.push_str(&format!("\n--- {rel} ---\n{file_content}\n"));
            }
        }
    }
    Ok(())
}

fn chunk_artifact_content(
    content: &str,
    artifact: &ContextArtifact,
    resolved_path: &Path,
) -> Vec<ArtifactChunk> {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return vec![];
    }
    let step = CHUNK_SIZE.saturating_sub(CHUNK_OVERLAP).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let end = (start + CHUNK_SIZE).min(lines.len());
        let mut chunk_content = lines[start..end].join("\n");
        if chunk_content.len() > MAX_CHUNK_CHARS {
            chunk_content.truncate(MAX_CHUNK_CHARS);
        }
        chunks.push(ArtifactChunk {
            id: chunk_id(&artifact.path, &artifact.name, start + 1),
            content: chunk_content,
            start_line: start + 1,
            end_line: end,
            artifact_name: artifact.name.clone(),
            artifact_path: resolved_path.display().to_string(),
            description: artifact.description.clone(),
        });
        if end >= lines.len() {
            break;
        }
        start += step;
    }
    chunks
}

fn write_index(config: &Config, project_path: &Path, index: &ContextIndex) -> Result<()> {
    let dir = index_dir(config);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = index_path(config, project_path);
    fs::write(&path, serde_json::to_vec(index)?)
        .with_context(|| format!("write {}", path.display()))
}

fn read_index(config: &Config, project_path: &Path) -> Result<Option<ContextIndex>> {
    let path = index_path(config, project_path);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("parse {}", path.display()))
}

fn index_dir(config: &Config) -> PathBuf {
    config.data_dir.join("code").join("context")
}

fn index_path(config: &Config, project_path: &Path) -> PathBuf {
    index_dir(config).join(format!("{}.json", project_id(project_path)))
}

fn project_id(project_path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project_path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn context_record(
    project_path: &Path,
    request: &ContextRequest,
    chunk: &ArtifactChunk,
    embedding: Vec<f32>,
) -> VectorRecord {
    let mut scope = WorkScope::project(project_path);
    scope.work_group_id = first_present([
        request.work_group_id.as_deref(),
        request.work_group_id_camel.as_deref(),
    ])
    .map(str::to_string);
    scope.work_unit_id = first_present([
        request.work_unit_id.as_deref(),
        request.work_unit_id_camel.as_deref(),
    ])
    .map(str::to_string);
    scope.agent_id = first_present([
        request.agent_id.as_deref(),
        request.agent_id_camel.as_deref(),
    ])
    .map(str::to_string);

    VectorRecord {
        id: chunk.id.clone(),
        kind: "context".to_string(),
        scope,
        text: chunk.content.clone(),
        source: "context_artifact".to_string(),
        file_path: chunk.artifact_path.clone(),
        relative_path: chunk.artifact_path.clone(),
        language: "context".to_string(),
        artifact_name: chunk.artifact_name.clone(),
        content_hash: hash(&chunk.content),
        topic_key: chunk.artifact_name.clone(),
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        created_at: unix_timestamp(),
        updated_at: unix_timestamp(),
        embedding,
    }
}

fn record_as_artifact_chunk(record: &VectorRecord) -> ArtifactChunk {
    ArtifactChunk {
        id: record.id.clone(),
        content: record.text.clone(),
        start_line: record.start_line,
        end_line: record.end_line,
        artifact_name: record.artifact_name.clone(),
        artifact_path: record.file_path.clone(),
        description: None,
    }
}

fn first_present<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

fn chunk_id(artifact_path: &str, artifact_name: &str, start_line: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("context:{artifact_path}:{artifact_name}:{start_line}").as_bytes());
    let hash = hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

fn hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn lexical_score(query_tokens: &[String], chunk: &ArtifactChunk) -> f64 {
    let chunk_tokens = tokens(&format!(
        "{} {} {}",
        chunk.artifact_name,
        chunk.description.clone().unwrap_or_default(),
        chunk.content
    ));
    if query_tokens.is_empty() || chunk_tokens.is_empty() {
        return 0.0;
    }
    let set = chunk_tokens.iter().collect::<HashSet<_>>();
    let matched = query_tokens
        .iter()
        .filter(|token| set.contains(token))
        .count();
    if matched == 0 {
        0.0
    } else {
        matched as f64 / query_tokens.len() as f64
    }
}

fn format_context_results(results: &[ContextSearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Retrieved context artifacts\n");
    for result in results.iter().take(8) {
        out.push_str(&format!(
            "- {}:{}-{} ({})\n{}\n",
            result.relative_path,
            result.start_line,
            result.end_line,
            result.artifact_name,
            result.content
        ));
        if out.len() > 4000 {
            out.truncate(4000);
            break;
        }
    }
    out
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|token| token.len() > 1)
        .map(str::to_string)
        .collect()
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
        chunk_artifact_content, format_context_results, ContextArtifact, ContextSearchResult,
    };
    use std::path::Path;

    #[test]
    fn chunks_context_artifact() {
        let artifact = ContextArtifact {
            name: "schema".to_string(),
            path: "schema.sql".to_string(),
            description: Some("database schema".to_string()),
        };
        let chunks = chunk_artifact_content(
            "create table users(id int);",
            &artifact,
            Path::new("/tmp/schema.sql"),
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].artifact_name, "schema");
        assert_eq!(chunks[0].start_line, 1);
    }

    #[test]
    fn formats_context_results_as_bounded_context_block() {
        let context = format_context_results(&[ContextSearchResult {
            file_path: "/tmp/schema.sql".to_string(),
            relative_path: "schema.sql".to_string(),
            content: "create table users(id int);".to_string(),
            start_line: 1,
            end_line: 1,
            language: "context".to_string(),
            score: 1.0,
            artifact_name: "schema".to_string(),
        }]);

        assert!(context.starts_with("## Retrieved context artifacts\n"));
        assert!(context.contains("schema.sql:1-1"));
    }
}
