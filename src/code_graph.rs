use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    code_index::{read_project_index, resolve_project_path, ProjectIndex},
    config::Config,
};

#[derive(Debug, Deserialize)]
pub struct GraphRequest {
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    #[serde(rename = "filePath")]
    pub file_path: Option<String>,
    pub file: Option<String>,
    pub target: Option<String>,
    pub entrypoint: Option<String>,
    pub name: Option<String>,
    pub query: Option<String>,
    pub depth: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CodeGraph {
    pub version: u8,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphNode {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    pub imports: Vec<String>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Serialize)]
pub struct GraphBuildResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub nodes: usize,
    pub edges: usize,
    pub symbols: usize,
}

#[derive(Debug, Serialize)]
pub struct GraphQueryResponse {
    pub ok: bool,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub imports: Vec<String>,
    #[serde(rename = "importedBy")]
    pub imported_by: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GraphStatsResponse {
    pub ok: bool,
    pub nodes: usize,
    pub edges: usize,
    pub symbols: usize,
}

#[derive(Debug, Serialize)]
pub struct GraphStatusResponse {
    pub ok: bool,
    pub status: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<u64>,
    pub nodes: usize,
    pub edges: usize,
}

#[derive(Debug, Serialize)]
pub struct SymbolsResponse {
    pub ok: bool,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize)]
pub struct CircularResponse {
    pub ok: bool,
    pub cycles: Vec<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct ImpactResponse {
    pub ok: bool,
    pub target: String,
    pub depth: usize,
    #[serde(rename = "filesByDepth")]
    pub files_by_depth: BTreeMap<usize, Vec<String>>,
    #[serde(rename = "totalFiles")]
    pub total_files: usize,
}

#[derive(Debug, Serialize)]
pub struct FlowResponse {
    pub ok: bool,
    pub target: String,
    pub depth: usize,
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entrypoints: Vec<EntryPoint>,
}

#[derive(Debug, Serialize)]
pub struct EntryPoint {
    pub name: String,
    pub file: String,
    pub line: Option<usize>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct VisualizeResponse {
    pub ok: bool,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub nodes: usize,
    pub edges: usize,
    pub mermaid: String,
}

pub fn build(config: &Config, request: GraphRequest) -> Result<GraphBuildResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let Some(index) = read_project_index(config, &project_path)? else {
        bail!(
            "No code index found for project: {}",
            project_path.display()
        );
    };
    let graph = build_from_index(&index);
    write_graph(config, &project_path, &graph)?;
    Ok(GraphBuildResponse {
        ok: true,
        project_path: project_path.display().to_string(),
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
        symbols: graph.symbols.len(),
    })
}

pub fn query(config: &Config, request: GraphRequest) -> Result<GraphQueryResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    let file_path = request.file_path.or(request.file).unwrap_or_else(|| {
        graph
            .nodes
            .first()
            .map(|n| n.relative_path.clone())
            .unwrap_or_default()
    });
    let normalized = normalize_path(&file_path);
    let node = graph
        .nodes
        .iter()
        .find(|node| normalize_path(&node.relative_path) == normalized);

    Ok(GraphQueryResponse {
        ok: true,
        file_path,
        imports: node
            .map(|node| node.dependencies.clone())
            .unwrap_or_default(),
        imported_by: node.map(|node| node.dependents.clone()).unwrap_or_default(),
    })
}

pub fn stats(config: &Config, request: GraphRequest) -> Result<GraphStatsResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    Ok(GraphStatsResponse {
        ok: true,
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
        symbols: graph.symbols.len(),
    })
}

pub fn status(config: &Config, request: GraphRequest) -> Result<GraphStatusResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let Some(graph) = read_graph(config, &project_path)? else {
        return Ok(GraphStatusResponse {
            ok: true,
            status: "not_built".to_string(),
            updated_at: None,
            nodes: 0,
            edges: 0,
        });
    };
    Ok(GraphStatusResponse {
        ok: true,
        status: "completed".to_string(),
        updated_at: Some(graph.updated_at),
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
    })
}

pub fn symbols(config: &Config, request: GraphRequest) -> Result<SymbolsResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    let limit = request.limit.unwrap_or(200).clamp(1, 1000);
    let query = request
        .query
        .or(request.name)
        .unwrap_or_default()
        .to_lowercase();
    let file = request.file.as_deref().map(normalize_path);
    let mut symbols = graph
        .symbols
        .into_iter()
        .filter(|symbol| query.is_empty() || symbol.name.to_lowercase().contains(&query))
        .filter(|symbol| {
            file.as_ref()
                .is_none_or(|file| normalize_path(&symbol.file) == *file)
        })
        .collect::<Vec<_>>();
    symbols.truncate(limit);
    Ok(SymbolsResponse { ok: true, symbols })
}

pub fn symbol(config: &Config, request: GraphRequest) -> Result<SymbolsResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    let name = request.name.unwrap_or_default();
    let file = request.file.as_deref().map(normalize_path);
    let symbols = graph
        .symbols
        .into_iter()
        .filter(|symbol| symbol.name == name)
        .filter(|symbol| {
            file.as_ref()
                .is_none_or(|file| normalize_path(&symbol.file) == *file)
        })
        .collect();
    Ok(SymbolsResponse { ok: true, symbols })
}

pub fn circular(config: &Config, request: GraphRequest) -> Result<CircularResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    Ok(CircularResponse {
        ok: true,
        cycles: find_cycles(&graph),
    })
}

pub fn impact(config: &Config, request: GraphRequest) -> Result<ImpactResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    let target = request
        .target
        .or(request.file_path)
        .or(request.file)
        .unwrap_or_default();
    let depth = request.depth.unwrap_or(3).clamp(1, 10);
    let files_by_depth = traverse_by_depth(&graph, &target, depth, TraversalDirection::Dependents);
    let total_files = files_by_depth.values().map(Vec::len).sum();

    Ok(ImpactResponse {
        ok: true,
        target,
        depth,
        files_by_depth,
        total_files,
    })
}

pub fn flow(config: &Config, request: GraphRequest) -> Result<FlowResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    let target = request
        .entrypoint
        .or(request.target)
        .or(request.file_path)
        .or(request.file)
        .unwrap_or_default();
    if target.trim().is_empty() {
        return Ok(FlowResponse {
            ok: true,
            target,
            depth: 0,
            files: vec![],
            entrypoints: detect_entrypoints(&graph),
        });
    }
    let traversal_target = resolve_flow_target(&graph, &target);
    let depth = request.depth.unwrap_or(5).clamp(1, 10);
    let files_by_depth = traverse_by_depth(
        &graph,
        &traversal_target,
        depth,
        TraversalDirection::Dependencies,
    );
    let files = files_by_depth
        .values()
        .flat_map(|files| files.iter().cloned())
        .collect();

    Ok(FlowResponse {
        ok: true,
        target,
        depth,
        files,
        entrypoints: vec![],
    })
}

pub fn visualize(config: &Config, request: GraphRequest) -> Result<VisualizeResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let graph = read_or_build_graph(config, &project_path)?;
    Ok(VisualizeResponse {
        ok: true,
        project_path: project_path.display().to_string(),
        nodes: graph.nodes.len(),
        edges: graph.edges.len(),
        mermaid: generate_mermaid(&graph),
    })
}

pub fn remove(config: &Config, request: GraphRequest) -> Result<GraphStatusResponse> {
    let project_path = resolve_project_path(request.project_path)?;
    let path = graph_path(config, &project_path);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(GraphStatusResponse {
        ok: true,
        status: "removed".to_string(),
        updated_at: None,
        nodes: 0,
        edges: 0,
    })
}

fn build_from_index(index: &ProjectIndex) -> CodeGraph {
    let files = index
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let mut dependencies_by_file = BTreeMap::<String, BTreeSet<String>>::new();
    let mut dependents_by_file = BTreeMap::<String, BTreeSet<String>>::new();
    let mut edges = Vec::new();
    let mut symbols = Vec::new();

    for file in &index.files {
        let content = file
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let imports = extract_imports(&content);
        for import in imports {
            if let Some(target) = resolve_import(&file.relative_path, &import, &files) {
                dependencies_by_file
                    .entry(file.relative_path.clone())
                    .or_default()
                    .insert(target.clone());
                dependents_by_file
                    .entry(target.clone())
                    .or_default()
                    .insert(file.relative_path.clone());
                edges.push(GraphEdge {
                    source: file.relative_path.clone(),
                    target,
                    edge_type: "import".to_string(),
                });
            }
        }
        symbols.extend(extract_symbols(&file.relative_path, &content));
    }

    let nodes = index
        .files
        .iter()
        .map(|file| GraphNode {
            file_path: file.file_path.clone(),
            relative_path: file.relative_path.clone(),
            imports: dependencies_by_file
                .get(&file.relative_path)
                .map(set_to_vec)
                .unwrap_or_default(),
            dependencies: dependencies_by_file
                .get(&file.relative_path)
                .map(set_to_vec)
                .unwrap_or_default(),
            dependents: dependents_by_file
                .get(&file.relative_path)
                .map(set_to_vec)
                .unwrap_or_default(),
        })
        .collect();

    CodeGraph {
        version: 1,
        project_path: index.project_path.clone(),
        updated_at: unix_timestamp(),
        nodes,
        edges,
        symbols,
    }
}

fn extract_imports(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                None
            } else if trimmed.starts_with("#include ") {
                bracket_or_quoted_segment(trimmed)
            } else if trimmed.starts_with('#') {
                None
            } else if let Some(rest) = trimmed.strip_prefix("from ") {
                rest.split_whitespace().next().map(str::to_string)
            } else if trimmed.starts_with("import ") {
                quoted_segment(trimmed)
                    .or_else(|| trimmed.split_whitespace().nth(1).map(str::to_string))
            } else if trimmed.starts_with("package ") {
                None
            } else if trimmed.starts_with("export ") && trimmed.contains(" from ") {
                quoted_segment(trimmed)
            } else if let Some(rest) = trimmed.strip_prefix("require ") {
                Some(rest.trim_matches('"').trim_matches('\'').to_string())
            } else if trimmed.starts_with("require(") || trimmed.starts_with("require ") {
                quoted_segment(trimmed)
            } else if let Some(rest) = trimmed.strip_prefix("using ") {
                if rest.contains('=') {
                    None
                } else {
                    Some(rest.trim_end_matches(';').trim().to_string())
                }
            } else if trimmed.starts_with("use ") {
                Some(
                    trimmed
                        .trim_start_matches("use ")
                        .trim_end_matches(';')
                        .to_string(),
                )
            } else if trimmed.starts_with("mod ") {
                Some(
                    trimmed
                        .trim_start_matches("mod ")
                        .trim_end_matches(';')
                        .to_string(),
                )
            } else if let Some(rest) = trimmed.strip_prefix("open ") {
                Some(rest.trim().to_string())
            } else {
                None
            }
        })
        .collect()
}

fn bracket_or_quoted_segment(value: &str) -> Option<String> {
    if let Some(quoted) = quoted_segment(value) {
        return Some(quoted);
    }
    let start = value.find('<')?;
    let rest = &value[start + 1..];
    let end = rest.find('>')?;
    Some(rest[..end].to_string())
}

fn quoted_segment(value: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let Some(start) = value.find(quote) else {
            continue;
        };
        let rest = &value[start + 1..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        return Some(rest[..end].to_string());
    }
    None
}

fn resolve_import(from: &str, import: &str, files: &BTreeSet<String>) -> Option<String> {
    let from_dir = Path::new(from).parent().unwrap_or_else(|| Path::new(""));
    let module_path = import.replace("::", "/").replace('.', "/");
    let base = if import.starts_with('.') {
        normalize_path(&from_dir.join(import).to_string_lossy())
    } else {
        normalize_path(&from_dir.join(&module_path).to_string_lossy())
    };
    let candidates = [
        base.clone(),
        format!("{base}.ts"),
        format!("{base}.tsx"),
        format!("{base}.js"),
        format!("{base}.jsx"),
        format!("{base}.mjs"),
        format!("{base}.rs"),
        format!("{base}.py"),
        format!("{base}.go"),
        format!("{base}.java"),
        format!("{base}.kt"),
        format!("{base}.cs"),
        format!("{base}.rb"),
        format!("{base}.php"),
        format!("{base}.swift"),
        format!("{base}.sh"),
        format!("{base}/index.ts"),
        format!("{base}/index.js"),
        format!("{base}/mod.rs"),
        format!("{base}/__init__.py"),
    ];
    candidates
        .into_iter()
        .find(|candidate| files.contains(candidate))
}

fn extract_symbols(file: &str, content: &str) -> Vec<Symbol> {
    content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let trimmed = line.trim();
            let (kind, name) = if let Some(rest) = trimmed.strip_prefix("export function ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("function ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("export const ") {
                ("constant", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("const ") {
                ("constant", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("export class ") {
                ("class", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("class ") {
                ("class", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("pub fn ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("fn ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("pub struct ") {
                ("struct", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("struct ") {
                ("struct", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("def ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("async def ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("func ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("type ") {
                ("type", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("interface ") {
                ("interface", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("enum ") {
                ("enum", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("public class ") {
                ("class", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("private class ") {
                ("class", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("internal class ") {
                ("class", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("public interface ") {
                ("interface", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("public enum ") {
                ("enum", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("fun ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("object ") {
                ("object", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("trait ") {
                ("trait", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("protocol ") {
                ("protocol", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("extension ") {
                ("extension", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("module ") {
                ("module", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("<?php function ") {
                ("function", word(rest))
            } else if let Some(rest) = trimmed.strip_prefix("function ") {
                ("function", word(rest))
            } else if trimmed.ends_with("() {") {
                ("function", word(trimmed))
            } else {
                return None;
            };
            Some(Symbol {
                name,
                kind: kind.to_string(),
                file: file.to_string(),
                line: idx + 1,
            })
        })
        .collect()
}

fn word(rest: &str) -> String {
    rest.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

fn read_or_build_graph(config: &Config, project_path: &Path) -> Result<CodeGraph> {
    if let Some(graph) = read_graph(config, project_path)? {
        return Ok(graph);
    }
    let Some(index) = read_project_index(config, project_path)? else {
        bail!(
            "No code index found for project: {}",
            project_path.display()
        );
    };
    let graph = build_from_index(&index);
    write_graph(config, project_path, &graph)?;
    Ok(graph)
}

fn read_graph(config: &Config, project_path: &Path) -> Result<Option<CodeGraph>> {
    let path = graph_path(config, project_path);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("parse {}", path.display()))
}

fn write_graph(config: &Config, project_path: &Path, graph: &CodeGraph) -> Result<()> {
    let dir = graph_dir(config);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = graph_path(config, project_path);
    fs::write(&path, serde_json::to_vec(graph)?)
        .with_context(|| format!("write {}", path.display()))
}

fn graph_dir(config: &Config) -> PathBuf {
    config.data_dir.join("code").join("graphs")
}

fn graph_path(config: &Config, project_path: &Path) -> PathBuf {
    graph_dir(config).join(format!("{}.json", project_id(project_path)))
}

fn project_id(project_path: &Path) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(project_path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn find_cycles(graph: &CodeGraph) -> Vec<Vec<String>> {
    let mut adjacency = BTreeMap::<String, Vec<String>>::new();
    for edge in &graph.edges {
        adjacency
            .entry(edge.source.clone())
            .or_default()
            .push(edge.target.clone());
    }

    let mut cycles = BTreeSet::<Vec<String>>::new();
    for start in adjacency.keys() {
        let mut stack = Vec::new();
        find_cycles_from(start, start, &adjacency, &mut stack, &mut cycles);
    }

    cycles.into_iter().collect()
}

fn detect_entrypoints(graph: &CodeGraph) -> Vec<EntryPoint> {
    let mut entries = Vec::new();
    for symbol in &graph.symbols {
        let lower = symbol.name.to_lowercase();
        if matches!(lower.as_str(), "main" | "handler" | "app" | "server")
            || symbol.name.ends_with("Entry")
            || symbol.name.ends_with("entry")
        {
            entries.push(EntryPoint {
                name: symbol.name.clone(),
                file: symbol.file.clone(),
                line: Some(symbol.line),
                reason: "conventional entrypoint symbol".to_string(),
            });
        }
    }

    for node in &graph.nodes {
        if node.dependents.is_empty() {
            let already_listed = entries.iter().any(|entry| entry.file == node.relative_path);
            if !already_listed {
                entries.push(EntryPoint {
                    name: node.relative_path.clone(),
                    file: node.relative_path.clone(),
                    line: None,
                    reason: "file has no known dependents".to_string(),
                });
            }
        }
    }
    entries.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.name.cmp(&b.name)));
    entries.truncate(50);
    entries
}

fn resolve_flow_target(graph: &CodeGraph, target: &str) -> String {
    let normalized = normalize_path(target);
    if graph
        .nodes
        .iter()
        .any(|node| normalize_path(&node.relative_path) == normalized)
    {
        return normalized;
    }
    graph
        .symbols
        .iter()
        .find(|symbol| symbol.name == target)
        .map(|symbol| symbol.file.clone())
        .unwrap_or(normalized)
}

fn find_cycles_from(
    start: &str,
    current: &str,
    adjacency: &BTreeMap<String, Vec<String>>,
    stack: &mut Vec<String>,
    cycles: &mut BTreeSet<Vec<String>>,
) {
    if stack.iter().any(|item| item == current) {
        return;
    }
    stack.push(current.to_string());

    if let Some(neighbors) = adjacency.get(current) {
        for neighbor in neighbors {
            if neighbor == start && stack.len() > 1 {
                cycles.insert(canonical_cycle(stack));
            } else {
                find_cycles_from(start, neighbor, adjacency, stack, cycles);
            }
        }
    }

    stack.pop();
}

fn canonical_cycle(cycle: &[String]) -> Vec<String> {
    let mut best = cycle.to_vec();
    for i in 1..cycle.len() {
        let rotated = cycle[i..]
            .iter()
            .chain(cycle[..i].iter())
            .cloned()
            .collect::<Vec<_>>();
        if rotated < best {
            best = rotated;
        }
    }
    best
}

fn generate_mermaid(graph: &CodeGraph) -> String {
    let mut lines = vec!["graph TD".to_string()];
    if graph.edges.is_empty() {
        for node in &graph.nodes {
            lines.push(format!("  {}", mermaid_node(&node.relative_path)));
        }
    } else {
        for edge in &graph.edges {
            lines.push(format!(
                "  {} --> {}",
                mermaid_node(&edge.source),
                mermaid_node(&edge.target)
            ));
        }
    }
    lines.join("\n")
}

fn mermaid_node(path: &str) -> String {
    let id = path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>();
    format!("{id}[\"{path}\"]")
}

enum TraversalDirection {
    Dependencies,
    Dependents,
}

fn traverse_by_depth(
    graph: &CodeGraph,
    target: &str,
    max_depth: usize,
    direction: TraversalDirection,
) -> BTreeMap<usize, Vec<String>> {
    let start = normalize_path(target);
    let mut result = BTreeMap::<usize, Vec<String>>::new();
    let mut visited = BTreeSet::<String>::from([start.clone()]);
    let mut frontier = BTreeSet::<String>::from([start]);

    for depth in 1..=max_depth {
        let mut next = BTreeSet::new();
        for file in &frontier {
            if let Some(node) = graph
                .nodes
                .iter()
                .find(|node| normalize_path(&node.relative_path) == *file)
            {
                let neighbors = match direction {
                    TraversalDirection::Dependencies => &node.dependencies,
                    TraversalDirection::Dependents => &node.dependents,
                };
                for neighbor in neighbors {
                    let normalized = normalize_path(neighbor);
                    if visited.insert(normalized.clone()) {
                        next.insert(normalized);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        result.insert(depth, next.iter().cloned().collect());
        frontier = next;
    }

    result
}

fn set_to_vec(set: &BTreeSet<String>) -> Vec<String> {
    set.iter().cloned().collect()
}

fn normalize_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_start_matches("./")
        .replace("/./", "/")
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{extract_imports, extract_symbols, find_cycles, CodeGraph, GraphEdge};

    #[test]
    fn extracts_ts_imports_and_symbols() {
        let content = "import { b } from './b';\nexport function alpha() {}\nconst beta = 1;";

        assert_eq!(extract_imports(content), vec!["./b"]);
        let symbols = extract_symbols("a.ts", content);
        assert_eq!(symbols[0].name, "alpha");
        assert_eq!(symbols[1].name, "beta");
    }

    #[test]
    fn extracts_multi_language_imports() {
        let content = [
            "from pkg.sub import thing",
            "import os",
            "#include <stdio.h>",
            "using System.Text;",
            "require 'json'",
            "use crate::module;",
            "open Foundation",
        ]
        .join("\n");

        assert_eq!(
            extract_imports(&content),
            vec![
                "pkg.sub",
                "os",
                "stdio.h",
                "System.Text",
                "json",
                "crate::module",
                "Foundation"
            ]
        );
    }

    #[test]
    fn extracts_multi_language_symbols() {
        let content = [
            "def py_func():",
            "class PyClass:",
            "func goFunc() {}",
            "type GoType struct {}",
            "public class JavaClass {}",
            "interface GoIface {}",
            "protocol SwiftProtocol {}",
            "module RubyModule",
            "shell_func() {",
        ]
        .join("\n");

        let names = extract_symbols("mixed.txt", &content)
            .into_iter()
            .map(|symbol| symbol.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "py_func",
                "PyClass",
                "goFunc",
                "GoType",
                "JavaClass",
                "GoIface",
                "SwiftProtocol",
                "RubyModule",
                "shell_func"
            ]
        );
    }

    #[test]
    fn detects_three_node_cycles() {
        let graph = CodeGraph {
            version: 1,
            project_path: "/tmp/project".to_string(),
            updated_at: 0,
            nodes: vec![],
            edges: vec![
                GraphEdge {
                    source: "a.ts".to_string(),
                    target: "b.ts".to_string(),
                    edge_type: "import".to_string(),
                },
                GraphEdge {
                    source: "b.ts".to_string(),
                    target: "c.ts".to_string(),
                    edge_type: "import".to_string(),
                },
                GraphEdge {
                    source: "c.ts".to_string(),
                    target: "a.ts".to_string(),
                    edge_type: "import".to_string(),
                },
            ],
            symbols: vec![],
        };

        assert_eq!(
            find_cycles(&graph),
            vec![vec![
                "a.ts".to_string(),
                "b.ts".to_string(),
                "c.ts".to_string()
            ]]
        );
    }
}
