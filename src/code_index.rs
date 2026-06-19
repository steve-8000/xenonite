use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    config::Config,
    rocky,
    storage::{code_collection, VectorRecord, WorkScope, ZvecStore},
};

const CHUNK_SIZE: usize = 100;
const CHUNK_OVERLAP: usize = 10;
const MAX_FILE_BYTES: u64 = 1_000_000;
const SEARCH_DEFAULT_LIMIT: usize = 10;
const SEARCH_MAX_LIMIT: usize = 50;

const DEFAULT_IGNORE_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    "__pycache__",
    ".venv",
    "venv",
    "target",
    ".gradle",
    ".idea",
    ".vscode",
    "coverage",
    ".cache",
    ".parcel-cache",
];

const DEFAULT_IGNORE_FILES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "Gemfile.lock",
    "poetry.lock",
    ".DS_Store",
    "Thumbs.db",
];

const DEFAULT_IGNORE_SUFFIXES: &[&str] = &[
    ".pyc", ".min.js", ".min.css", ".map", ".lock", ".log", ".tmp", ".swp", ".swo",
];

const SPECIAL_FILES: &[&str] = &["Dockerfile", "Makefile", "Rakefile", "Gemfile", "Procfile"];

const SUPPORTED_EXTENSIONS: &[&str] = &[
    ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json", ".py", ".rs", ".go", ".java", ".kt",
    ".kts", ".swift", ".c", ".h", ".cpp", ".hpp", ".cc", ".cs", ".php", ".rb", ".lua", ".scala",
    ".sc", ".sol", ".sh", ".bash", ".zsh", ".fish", ".html", ".css", ".scss", ".md", ".mdx",
    ".yml", ".yaml", ".toml", ".nix", ".sql", ".graphql", ".proto",
];

#[derive(Debug, Deserialize)]
pub struct CodeRequest {
    pub op: Option<String>,
    pub args: Option<serde_json::Value>,
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    #[serde(rename = "fileFilter")]
    pub file_filter: Option<String>,
    #[serde(rename = "languageFilter")]
    pub language_filter: Option<String>,
    #[serde(rename = "minScore")]
    pub min_score: Option<f64>,
    #[serde(rename = "includeLinked")]
    pub include_linked: Option<bool>,
    #[serde(rename = "extraExtensions")]
    pub extra_extensions: Option<String>,
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

#[derive(Debug, Serialize)]
pub struct IndexResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "filesIndexed")]
    pub files_indexed: usize,
    #[serde(rename = "chunksCreated")]
    pub chunks_created: usize,
    pub cancelled: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub status: String,
    #[serde(rename = "indexedFiles")]
    pub indexed_files: usize,
    #[serde(rename = "indexedChunks")]
    pub indexed_chunks: usize,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub ok: bool,
    pub query: String,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
pub struct UpdateResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    #[serde(rename = "chunksCreated")]
    pub chunks_created: usize,
    pub cancelled: bool,
}

#[derive(Debug, Serialize)]
pub struct RemoveResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub removed: bool,
}

#[derive(Debug, Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ProjectListResponse {
    pub ok: bool,
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Serialize)]
pub struct ProjectSummary {
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "indexedFiles")]
    pub indexed_files: usize,
    #[serde(rename = "indexedChunks")]
    pub indexed_chunks: usize,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileChunk {
    pub id: String,
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
    #[serde(rename = "type")]
    pub chunk_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedFile {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
    pub chunks: Vec<FileChunk>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectIndex {
    pub version: u8,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    pub files: Vec<IndexedFile>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
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
}

pub async fn index_project(config: &Config, request: CodeRequest) -> Result<IndexResponse> {
    let project_path = resolve_project_path(request.project_path.clone())?;
    let extra_extensions = parse_extra_extensions(request.extra_extensions.as_deref());
    let mut files = scan_project(&project_path, &extra_extensions)?;
    attach_embeddings(config, &mut files).await;
    let chunks_created = files.iter().map(|file| file.chunks.len()).sum();
    let index = ProjectIndex {
        version: 1,
        project_path: project_path.display().to_string(),
        updated_at: unix_timestamp(),
        files,
    };
    write_project_index(config, &project_path, &index)?;
    let records = index
        .files
        .iter()
        .flat_map(|file| {
            file.chunks
                .iter()
                .map(|chunk| code_record(&project_path, &request, file, chunk))
        })
        .collect::<Vec<_>>();
    let store = ZvecStore::new(config);
    store.delete_collection(&code_collection(&project_path))?;
    store.upsert_records(&code_collection(&project_path), &records)?;

    Ok(IndexResponse {
        ok: true,
        project_path: project_path.display().to_string(),
        files_indexed: index.files.len(),
        chunks_created,
        cancelled: false,
    })
}

pub async fn update_project(config: &Config, request: CodeRequest) -> Result<UpdateResponse> {
    let project_path = resolve_project_path(request.project_path.clone())?;
    let before = read_project_index(config, &project_path)?;
    let indexed = index_project(config, request).await?;
    let after = read_project_index(config, &project_path)?.unwrap_or(ProjectIndex {
        version: 1,
        project_path: project_path.display().to_string(),
        updated_at: unix_timestamp(),
        files: vec![],
    });

    let before_paths = before
        .as_ref()
        .map(|index| {
            index
                .files
                .iter()
                .map(|file| file.relative_path.clone())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let after_paths = after
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<HashSet<_>>();

    let added = after_paths.difference(&before_paths).count();
    let removed = before_paths.difference(&after_paths).count();
    let updated = after
        .files
        .iter()
        .filter(|file| {
            before
                .as_ref()
                .and_then(|index| {
                    index
                        .files
                        .iter()
                        .find(|old| old.relative_path == file.relative_path)
                })
                .is_some_and(|old| old.content_hash != file.content_hash)
        })
        .count();

    Ok(UpdateResponse {
        ok: true,
        project_path: indexed.project_path,
        added,
        updated,
        removed,
        chunks_created: indexed.chunks_created,
        cancelled: false,
    })
}

pub fn remove_project(config: &Config, request: CodeRequest) -> Result<RemoveResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let path = index_path(config, &project_path);
    let removed = if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        let _ = ZvecStore::new(config).delete_collection(&code_collection(&project_path));
        true
    } else {
        false
    };

    Ok(RemoveResponse {
        ok: true,
        project_path: project_path.display().to_string(),
        removed,
    })
}

pub fn list_projects(config: &Config) -> Result<ProjectListResponse> {
    let dir = index_dir(config);
    if !dir.exists() {
        return Ok(ProjectListResponse {
            ok: true,
            projects: vec![],
        });
    }

    let mut projects = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let raw = fs::read_to_string(entry.path())?;
        if let Ok(index) = serde_json::from_str::<ProjectIndex>(&raw) {
            projects.push(ProjectSummary {
                project_path: index.project_path,
                indexed_files: index.files.len(),
                indexed_chunks: index.files.iter().map(|file| file.chunks.len()).sum(),
                updated_at: index.updated_at,
            });
        }
    }
    projects.sort_by(|a, b| a.project_path.cmp(&b.project_path));
    Ok(ProjectListResponse { ok: true, projects })
}

pub fn status(config: &Config, request: CodeRequest) -> Result<StatusResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let Some(index) = read_project_index(config, &project_path)? else {
        return Ok(StatusResponse {
            ok: true,
            project_path: project_path.display().to_string(),
            status: "not_indexed".to_string(),
            indexed_files: 0,
            indexed_chunks: 0,
            updated_at: None,
        });
    };

    Ok(StatusResponse {
        ok: true,
        project_path: index.project_path,
        status: "completed".to_string(),
        indexed_files: index.files.len(),
        indexed_chunks: index.files.iter().map(|file| file.chunks.len()).sum(),
        updated_at: Some(index.updated_at),
    })
}

pub async fn search(config: &Config, request: CodeRequest) -> Result<SearchResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let query = request.query.unwrap_or_default();
    let limit = request
        .limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);
    let file_filter = request.file_filter.clone();
    let language_filter = request.language_filter.clone();
    let min_score = request.min_score.unwrap_or(0.0).clamp(0.0, 1.0);
    let Some(index) = read_project_index(config, &project_path)? else {
        bail!("No index found for project: {}", project_path.display());
    };
    let mut indexes = vec![index];
    if request.include_linked.unwrap_or(false) {
        for linked_path in linked_projects(&project_path)? {
            if let Some(linked_index) = read_project_index(config, &linked_path)? {
                indexes.push(linked_index);
            }
        }
    }

    let query_tokens = tokens(&query);
    let query_embedding = rocky::embed(&config.rocky, std::slice::from_ref(&query))
        .await
        .ok()
        .and_then(|mut vectors| vectors.pop());
    let mut results = ZvecStore::new(config)
        .search(
            &code_collection(&project_path),
            query_embedding.as_deref().unwrap_or(&[]),
            limit.max(50),
        )
        .unwrap_or_default()
        .into_iter()
        .filter(|hit| {
            file_filter
                .as_ref()
                .is_none_or(|filter| &hit.record.relative_path == filter)
                && language_filter
                    .as_ref()
                    .is_none_or(|filter| &hit.record.language == filter)
        })
        .filter_map(|hit| {
            let chunk = record_as_chunk(&hit.record);
            let lexical = lexical_score(&query_tokens, &chunk);
            let score = (lexical * 0.35) + (hit.score * 0.65);
            (score > 0.0 && score >= min_score).then(|| SearchResult {
                file_path: hit.record.file_path,
                relative_path: hit.record.relative_path,
                content: hit.record.text,
                start_line: hit.record.start_line,
                end_line: hit.record.end_line,
                language: hit.record.language,
                score,
            })
        })
        .collect::<Vec<_>>();

    if results.is_empty() {
        results = indexes
            .iter()
            .flat_map(|index| index.files.iter())
            .flat_map(|file| file.chunks.iter())
            .filter(|chunk| {
                matches_search_filters(chunk, file_filter.as_deref(), language_filter.as_deref())
            })
            .filter_map(|chunk| {
                let lexical = lexical_score(&query_tokens, chunk);
                let vector = query_embedding
                    .as_ref()
                    .zip(chunk.embedding.as_ref())
                    .map(|(query, chunk)| cosine(query, chunk))
                    .unwrap_or(0.0);
                let score = if vector > 0.0 {
                    (lexical * 0.35) + (vector * 0.65)
                } else {
                    lexical
                };
                (score > 0.0 && score >= min_score).then(|| SearchResult {
                    file_path: chunk.file_path.clone(),
                    relative_path: chunk.relative_path.clone(),
                    content: chunk.content.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    language: chunk.language.clone(),
                    score,
                })
            })
            .collect::<Vec<_>>();
    }

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    results.truncate(limit);

    Ok(SearchResponse {
        ok: true,
        query,
        results,
    })
}

fn scan_project(
    project_path: &Path,
    extra_extensions: &HashSet<String>,
) -> Result<Vec<IndexedFile>> {
    if !project_path.exists() {
        bail!("project path does not exist: {}", project_path.display());
    }

    let mut files = Vec::new();
    let ignore_rules = IgnoreRules::load(project_path)?;
    scan_dir(
        project_path,
        project_path,
        extra_extensions,
        &ignore_rules,
        &mut files,
    )?;
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

fn linked_projects(project_path: &Path) -> Result<Vec<PathBuf>> {
    let mut linked = Vec::new();
    let config_path = project_path.join(".xenonite.json");
    if config_path.exists() {
        let raw = fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?;
        if let Ok(config) = serde_json::from_str::<XenoniteConfig>(&raw) {
            for path in config.linked_projects.unwrap_or_default() {
                linked.push(resolve_linked_path(project_path, &path));
            }
        }
    }

    if let Ok(value) = env::var("XENONITE_LINKED_PROJECTS") {
        for path in value
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            linked.push(resolve_linked_path(project_path, path));
        }
    }

    let mut seen = HashSet::new();
    linked.retain(|path| seen.insert(path.display().to_string()));
    Ok(linked)
}

fn resolve_linked_path(project_path: &Path, linked_path: &str) -> PathBuf {
    let path = PathBuf::from(linked_path);
    let resolved = if path.is_absolute() {
        path
    } else {
        project_path.join(path)
    };
    resolved.canonicalize().unwrap_or(resolved)
}

#[derive(Debug, Deserialize)]
struct XenoniteConfig {
    #[serde(rename = "linkedProjects")]
    linked_projects: Option<Vec<String>>,
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    extra_extensions: &HashSet<String>,
    ignore_rules: &IgnoreRules,
    files: &mut Vec<IndexedFile>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let relative_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        if entry.file_type()?.is_dir() {
            if should_ignore_dir(&name)
                || relative_path.split('/').any(should_ignore_dir)
                || ignore_rules.is_ignored(&relative_path, true)
            {
                continue;
            }
            scan_dir(root, &path, extra_extensions, ignore_rules, files)?;
            continue;
        }

        if !entry.file_type()?.is_file()
            || ignore_rules.is_ignored(&relative_path, false)
            || should_ignore_file(&name)
            || !is_indexable_file(&path, extra_extensions)
        {
            continue;
        }
        if entry.metadata()?.len() > MAX_FILE_BYTES {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if content.contains('\0') {
            continue;
        }
        let chunks = chunk_file(&path, &relative_path, &content);
        if chunks.is_empty() {
            continue;
        }

        files.push(IndexedFile {
            file_path: path.display().to_string(),
            relative_path,
            content_hash: content_hash(&content),
            chunks,
        });
    }

    Ok(())
}

fn chunk_file(path: &Path, relative_path: &str, content: &str) -> Vec<FileChunk> {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return vec![];
    }

    let language = language_for_path(path);
    let mut chunks = Vec::new();
    let step = CHUNK_SIZE.saturating_sub(CHUNK_OVERLAP).max(1);
    let mut start = 0;
    while start < lines.len() {
        let end = (start + CHUNK_SIZE).min(lines.len());
        chunks.push(FileChunk {
            id: chunk_id(relative_path, start + 1),
            file_path: path.display().to_string(),
            relative_path: relative_path.to_string(),
            content: lines[start..end].join("\n"),
            start_line: start + 1,
            end_line: end,
            language: language.clone(),
            chunk_type: "code".to_string(),
            embedding: None,
        });
        if end >= lines.len() {
            break;
        }
        start += step;
    }
    chunks
}

async fn attach_embeddings(config: &Config, files: &mut [IndexedFile]) {
    let texts = files
        .iter()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk| format!("{}:\n{}", chunk.relative_path, chunk.content))
        .collect::<Vec<_>>();
    if texts.is_empty() {
        return;
    }
    let Ok(vectors) = rocky::embed(&config.rocky, &texts).await else {
        return;
    };
    let mut vectors = vectors.into_iter();
    for chunk in files.iter_mut().flat_map(|file| file.chunks.iter_mut()) {
        chunk.embedding = vectors.next();
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

fn matches_search_filters(
    chunk: &FileChunk,
    file_filter: Option<&str>,
    language_filter: Option<&str>,
) -> bool {
    if let Some(filter) = file_filter.filter(|filter| !filter.trim().is_empty()) {
        let filter = filter.replace('\\', "/");
        if !chunk.relative_path.contains(&filter)
            && !chunk.file_path.replace('\\', "/").contains(&filter)
        {
            return false;
        }
    }
    if let Some(filter) = language_filter.filter(|filter| !filter.trim().is_empty()) {
        if chunk.language.to_lowercase() != filter.to_lowercase() {
            return false;
        }
    }
    true
}

fn write_project_index(config: &Config, project_path: &Path, index: &ProjectIndex) -> Result<()> {
    let dir = index_dir(config);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = index_path(config, project_path);
    fs::write(&path, serde_json::to_vec(index)?)
        .with_context(|| format!("write {}", path.display()))
}

pub(crate) fn read_project_index(
    config: &Config,
    project_path: &Path,
) -> Result<Option<ProjectIndex>> {
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
    config.data_dir.join("code").join("indexes")
}

fn index_path(config: &Config, project_path: &Path) -> PathBuf {
    index_dir(config).join(format!("{}.json", project_id(project_path)))
}

pub(crate) fn resolve_project_path(project_path: Option<String>) -> Result<PathBuf> {
    let path = project_path
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let path = if let Ok(stripped) = path.strip_prefix("/host") {
        let local_path = PathBuf::from("/").join(stripped);
        if local_path.exists() {
            local_path
        } else {
            path
        }
    } else {
        path
    };
    Ok(path.canonicalize().unwrap_or(path))
}

fn project_id(project_path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project_path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn chunk_id(relative_path: &str, start_line: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{relative_path}:{start_line}").as_bytes());
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

fn code_record(
    project_path: &Path,
    request: &CodeRequest,
    file: &IndexedFile,
    chunk: &FileChunk,
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
        kind: "code".to_string(),
        scope,
        text: chunk.content.clone(),
        source: "code_index".to_string(),
        file_path: chunk.file_path.clone(),
        relative_path: chunk.relative_path.clone(),
        language: chunk.language.clone(),
        artifact_name: String::new(),
        content_hash: file.content_hash.clone(),
        topic_key: chunk.relative_path.clone(),
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        created_at: unix_timestamp(),
        updated_at: unix_timestamp(),
        embedding: chunk.embedding.clone().unwrap_or_default(),
    }
}

fn record_as_chunk(record: &VectorRecord) -> FileChunk {
    FileChunk {
        id: record.id.clone(),
        file_path: record.file_path.clone(),
        relative_path: record.relative_path.clone(),
        content: record.text.clone(),
        start_line: record.start_line,
        end_line: record.end_line,
        language: record.language.clone(),
        chunk_type: "mixed".to_string(),
        embedding: Some(record.embedding.clone()),
    }
}

fn first_present<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

fn is_indexable_file(path: &Path, extra_extensions: &HashSet<String>) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if SPECIAL_FILES.contains(&file_name) {
        return true;
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_lowercase()));
    extension.as_ref().is_some_and(|ext| {
        SUPPORTED_EXTENSIONS.contains(&ext.as_str()) || extra_extensions.contains(ext)
    })
}

fn should_ignore_dir(name: &str) -> bool {
    DEFAULT_IGNORE_DIRS.contains(&name)
}

fn should_ignore_file(name: &str) -> bool {
    DEFAULT_IGNORE_FILES.contains(&name)
        || DEFAULT_IGNORE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

#[derive(Debug, Default)]
struct IgnoreRules {
    patterns: Vec<String>,
}

impl IgnoreRules {
    fn load(root: &Path) -> Result<Self> {
        let mut rules = Self::default();
        rules.load_gitignores(root, root)?;
        Ok(rules)
    }

    fn load_gitignores(&mut self, root: &Path, dir: &Path) -> Result<()> {
        let gitignore = dir.join(".gitignore");
        if gitignore.exists() {
            let prefix = dir
                .strip_prefix(root)
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .replace('\\', "/");
            let raw = fs::read_to_string(&gitignore)
                .with_context(|| format!("read {}", gitignore.display()))?;
            for line in raw.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                    continue;
                }
                let pattern = trimmed.trim_start_matches('/');
                let pattern = if prefix.is_empty() {
                    pattern.to_string()
                } else {
                    format!("{prefix}/{pattern}")
                };
                self.patterns.push(pattern);
            }
        }

        for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !should_ignore_dir(&name) {
                    self.load_gitignores(root, &entry.path())?;
                }
            }
        }

        Ok(())
    }

    fn is_ignored(&self, relative_path: &str, is_dir: bool) -> bool {
        let path = relative_path.replace('\\', "/");
        self.patterns
            .iter()
            .any(|pattern| matches_ignore_pattern(pattern, &path, is_dir))
    }
}

fn matches_ignore_pattern(pattern: &str, path: &str, is_dir: bool) -> bool {
    let pattern = pattern.trim_end_matches('/');
    if pattern.is_empty() {
        return false;
    }
    if pattern.contains('*') {
        return wildcard_match(pattern, path)
            || path
                .rsplit('/')
                .next()
                .is_some_and(|name| wildcard_match(pattern, name));
    }
    if path == pattern || path.starts_with(&format!("{pattern}/")) {
        return true;
    }
    if !pattern.contains('/') {
        return path.split('/').any(|segment| segment == pattern)
            || (is_dir && path.ends_with(pattern));
    }
    false
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let mut remainder = value;
    let anchored = !pattern.starts_with('*');
    let mut parts = pattern.split('*').filter(|part| !part.is_empty());
    if let Some(first) = parts.next() {
        if anchored {
            let Some(stripped) = remainder.strip_prefix(first) else {
                return false;
            };
            remainder = stripped;
        } else if let Some(index) = remainder.find(first) {
            remainder = &remainder[index + first.len()..];
        } else {
            return false;
        }
    }
    for part in parts {
        if let Some(index) = remainder.find(part) {
            remainder = &remainder[index + part.len()..];
        } else {
            return false;
        }
    }
    pattern.ends_with('*') || remainder.is_empty()
}

fn language_for_path(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "rb" => "ruby",
        "lua" => "lua",
        "scala" | "sc" => "scala",
        "sol" => "solidity",
        "sh" | "bash" | "zsh" | "fish" => "bash",
        "html" => "html",
        "css" | "scss" => "css",
        "json" => "json",
        "md" | "mdx" => "markdown",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "nix" => "nix",
        _ => "text",
    }
    .to_string()
}

fn lexical_score(query_tokens: &[String], chunk: &FileChunk) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }

    let content_tokens = tokens(&format!("{} {}", chunk.relative_path, chunk.content));
    let content_set = content_tokens.iter().collect::<HashSet<_>>();
    let matched = query_tokens
        .iter()
        .filter(|token| content_set.contains(token))
        .count();
    if matched == 0 {
        return 0.0;
    }

    let density = matched as f64 / query_tokens.len() as f64;
    let repeats = query_tokens
        .iter()
        .map(|token| {
            content_tokens
                .iter()
                .filter(|candidate| *candidate == token)
                .count()
        })
        .sum::<usize>() as f64;
    density + repeats.log2() / 10.0
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

fn parse_extra_extensions(value: Option<&str>) -> HashSet<String> {
    value
        .unwrap_or("")
        .split(',')
        .filter_map(|ext| {
            let trimmed = ext.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.starts_with('.') {
                Some(trimmed.to_lowercase())
            } else {
                Some(format!(".{}", trimmed.to_lowercase()))
            }
        })
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
        chunk_file, is_indexable_file, matches_ignore_pattern, matches_search_filters,
        parse_extra_extensions, resolve_linked_path, should_ignore_file,
    };
    use std::path::Path;

    #[test]
    fn chunk_file_creates_stable_line_chunk() {
        let chunks = chunk_file(
            Path::new("/tmp/example.rs"),
            "example.rs",
            "fn main() {}\nlet x = 1;",
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 2);
        assert_eq!(chunks[0].language, "rust");
    }

    #[test]
    fn indexable_file_honors_extra_extensions() {
        let extras = parse_extra_extensions(Some("txt,.prompt"));

        assert!(is_indexable_file(Path::new("notes.prompt"), &extras));
        assert!(is_indexable_file(Path::new("src/main.rs"), &extras));
        assert!(!is_indexable_file(Path::new("image.png"), &extras));
    }

    #[test]
    fn default_ignore_files_skip_locks_logs_and_minified_outputs() {
        assert!(should_ignore_file("package-lock.json"));
        assert!(should_ignore_file("debug.log"));
        assert!(should_ignore_file("bundle.min.js"));
        assert!(!should_ignore_file("source.ts"));
    }

    #[test]
    fn gitignore_matcher_handles_paths_dirs_and_wildcards() {
        assert!(matches_ignore_pattern("generated/", "src/generated", true));
        assert!(matches_ignore_pattern(
            "secrets/*.ts",
            "secrets/key.ts",
            false
        ));
        assert!(matches_ignore_pattern(
            "ignored.ts",
            "src/ignored.ts",
            false
        ));
        assert!(!matches_ignore_pattern("ignored.ts", "src/kept.ts", false));
    }

    #[test]
    fn search_filters_match_relative_path_and_language() {
        let chunks = chunk_file(
            Path::new("/tmp/project/src/main.rs"),
            "src/main.rs",
            "fn main() {}",
        );
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];

        assert!(matches_search_filters(
            chunk,
            Some("src/main"),
            Some("rust")
        ));
        assert!(matches_search_filters(chunk, Some("main.rs"), None));
        assert!(!matches_search_filters(chunk, Some("other.rs"), None));
        assert!(!matches_search_filters(chunk, None, Some("typescript")));
    }

    #[test]
    fn linked_paths_resolve_relative_to_project_root() {
        let linked = resolve_linked_path(Path::new("/tmp/project"), "../linked");

        assert!(linked.ends_with("linked"));
    }
}
