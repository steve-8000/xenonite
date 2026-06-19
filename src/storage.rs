use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use zvec::{
    Collection, CollectionSchema, DataType, Doc, DocRef, FieldSchema, IndexParams, IndexType,
    MetricType, VectorQuery,
};

use crate::config::Config;

const EMBEDDING_FIELD: &str = "embedding";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkScope {
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "workGroupId", skip_serializing_if = "Option::is_none")]
    pub work_group_id: Option<String>,
    #[serde(rename = "workUnitId", skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

impl WorkScope {
    pub fn project(project_path: &Path) -> Self {
        Self {
            project_id: stable_id(project_path.to_string_lossy().as_ref()),
            work_group_id: None,
            work_unit_id: None,
            agent_id: None,
        }
    }

    pub fn global() -> Self {
        Self {
            project_id: "global".to_string(),
            work_group_id: None,
            work_unit_id: None,
            agent_id: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorRecord {
    pub id: String,
    pub kind: String,
    pub scope: WorkScope,
    pub text: String,
    pub source: String,
    pub file_path: String,
    pub relative_path: String,
    pub language: String,
    pub artifact_name: String,
    pub content_hash: String,
    pub topic_key: String,
    pub start_line: usize,
    pub end_line: usize,
    pub created_at: u64,
    pub updated_at: u64,
    pub embedding: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct VectorHit {
    pub record: VectorRecord,
    pub score: f64,
}

pub struct ZvecStore {
    root: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct StorageStatus {
    pub backend: &'static str,
    #[serde(rename = "dataDir")]
    pub data_dir: String,
    #[serde(rename = "sidecarRecovery")]
    pub sidecar_recovery: bool,
    #[serde(rename = "directQueryFallback")]
    pub direct_query_fallback: &'static str,
}

impl ZvecStore {
    pub fn new(config: &Config) -> Self {
        Self {
            root: config.data_dir.join("zvec"),
        }
    }

    pub fn status(&self) -> StorageStatus {
        StorageStatus {
            backend: "zvec",
            data_dir: self.root.display().to_string(),
            sidecar_recovery: true,
            direct_query_fallback: "sidecar_cosine_when_zvec_query_returns_no_hits",
        }
    }

    pub fn upsert_records(&self, collection: &str, records: &[VectorRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let normalized_records = records
            .iter()
            .cloned()
            .map(|mut record| {
                if record.embedding.is_empty() {
                    record.embedding = vec![1.0];
                }
                record
            })
            .collect::<Vec<_>>();
        let dimension = normalized_records
            .iter()
            .find(|record| !record.embedding.is_empty())
            .map(|record| record.embedding.len())
            .unwrap_or(1);
        let collection_name = collection.to_string();
        let collection = self.open_or_create(&collection_name, dimension)?;
        let docs = normalized_records
            .iter()
            .map(|record| record_to_doc(record, dimension))
            .collect::<Result<Vec<_>>>()?;
        let pks = normalized_records
            .iter()
            .map(|record| zvec_pk(&record.id))
            .collect::<Vec<_>>();
        let pk_refs = pks.iter().map(String::as_str).collect::<Vec<_>>();
        let _ = collection.delete(&pk_refs);
        for batch in docs.chunks(128) {
            let refs = batch.iter().collect::<Vec<_>>();
            let summary = collection
                .insert(&refs)
                .map_err(|err| anyhow!("upsert zvec collection {collection_name}: {err}"))?;
            if summary.error > 0 {
                bail!(
                    "zvec insert into {} reported {} successes and {} errors",
                    collection_name,
                    summary.success,
                    summary.error
                );
            }
        }
        collection.flush()?;
        collection.optimize()?;
        self.write_sidecar(&collection_name, &normalized_records)?;
        Ok(())
    }

    pub fn search(
        &self,
        collection: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorHit>> {
        let dimension = query_vector.len().max(1);
        let collection_name = collection.to_string();
        let collection = self.open_or_create(&collection_name, dimension)?;
        let mut query = VectorQuery::new()?;
        query.set_field_name(EMBEDDING_FIELD)?;
        let fallback;
        let vector = if query_vector.is_empty() {
            fallback = vec![0.0_f32; dimension];
            fallback.as_slice()
        } else {
            query_vector
        };
        query.set_query_vector_fp32(vector)?;
        query.set_topk(limit.max(1) as i32)?;
        query.set_include_vector(true)?;
        query.set_include_doc_id(true)?;
        let docs = collection.query(&query)?;
        let hits = docs
            .iter()
            .map(|doc| {
                let score = doc.score() as f64;
                Ok(VectorHit {
                    record: doc_to_record(doc, dimension)?,
                    score,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if hits.is_empty() {
            return self.sidecar_search(&collection_name, query_vector, limit);
        }
        Ok(hits)
    }

    pub fn delete_collection(&self, collection: &str) -> Result<bool> {
        let path = self.collection_path(collection);
        let _ = fs::remove_file(self.sidecar_path(collection));
        if path.exists() {
            let metadata =
                fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?;
            if metadata.is_dir() {
                fs::remove_dir_all(&path).or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })?;
            } else {
                fs::remove_file(&path).or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })?;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn open_or_create(&self, collection: &str, dimension: usize) -> Result<Collection> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        let path = self.collection_path(collection);
        let path_string = path.to_string_lossy().to_string();
        if !path.exists() {
            fs::create_dir_all(path.parent().unwrap_or(&self.root))
                .with_context(|| format!("create {}", self.root.display()))?;
            let schema = collection_schema(collection, dimension as u32)?;
            return Collection::create_and_open(&path_string, &schema, None)
                .with_context(|| format!("create zvec collection {}", collection));
        }
        match Collection::open(&path_string, None) {
            Ok(collection) => Ok(collection),
            Err(open_error) => {
                fs::create_dir_all(path.parent().unwrap_or(&self.root))
                    .with_context(|| format!("create {}", self.root.display()))?;
                let schema = collection_schema(collection, dimension as u32)?;
                Collection::create_and_open(&path_string, &schema, None).with_context(|| {
                    format!(
                        "open/create zvec collection {} after open failed: {}",
                        collection, open_error
                    )
                })
            }
        }
    }

    fn collection_path(&self, collection: &str) -> PathBuf {
        self.root.join(safe_name(collection))
    }

    fn sidecar_path(&self, collection: &str) -> PathBuf {
        self.root
            .join(format!("{}.records.json", safe_name(collection)))
    }

    fn write_sidecar(&self, collection: &str, records: &[VectorRecord]) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        let path = self.sidecar_path(collection);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(records)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }

    fn read_sidecar(&self, collection: &str) -> Result<Vec<VectorRecord>> {
        let path = self.sidecar_path(collection);
        if !path.exists() {
            return Ok(vec![]);
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
    }

    fn sidecar_search(
        &self,
        collection: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorHit>> {
        let mut hits = self
            .read_sidecar(collection)?
            .into_iter()
            .map(|record| {
                let score = cosine(query_vector, &record.embedding);
                VectorHit { record, score }
            })
            .collect::<Vec<_>>();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

pub fn code_collection(project_path: &Path) -> String {
    format!(
        "code_project_{}",
        stable_id(project_path.to_string_lossy().as_ref())
    )
}

pub fn context_collection(project_path: &Path) -> String {
    format!(
        "context_project_{}",
        stable_id(project_path.to_string_lossy().as_ref())
    )
}

pub fn memory_collection(scope_key: &str) -> String {
    format!("memory_{}", safe_name(scope_key))
}

pub fn stable_id(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn collection_schema(name: &str, dimension: u32) -> Result<CollectionSchema> {
    let mut schema = CollectionSchema::new(name)?;
    let mut invert = IndexParams::new(IndexType::Invert)?;
    invert.set_invert_params(true, false)?;
    for field in ["id"] {
        let mut schema_field = FieldSchema::new(field, DataType::String, true, 0)?;
        schema_field.set_index_params(&invert)?;
        schema.add_field(&schema_field)?;
    }
    let text_field = FieldSchema::new("text", DataType::String, true, 0)?;
    schema.add_field(&text_field)?;
    let payload_field = FieldSchema::new("payload", DataType::String, true, 0)?;
    schema.add_field(&payload_field)?;
    let mut hnsw = IndexParams::new(IndexType::Hnsw)?;
    hnsw.set_metric_type(MetricType::Cosine)?;
    hnsw.set_hnsw_params(16, 200)?;
    let mut embedding = FieldSchema::new(EMBEDDING_FIELD, DataType::VectorFp32, false, dimension)?;
    embedding.set_index_params(&hnsw)?;
    schema.add_field(&embedding)?;
    Ok(schema)
}

fn record_to_doc(record: &VectorRecord, dimension: usize) -> Result<Doc> {
    let mut doc = Doc::new()?;
    doc.set_pk(&zvec_pk(&record.id))?;
    add_string(&mut doc, "id", &record.id)?;
    add_string(&mut doc, "text", &record.text)?;
    add_string(&mut doc, "payload", &serde_json::to_string(record)?)?;
    let mut embedding = record.embedding.clone();
    if embedding.is_empty() {
        embedding.resize(dimension, 0.0);
    }
    doc.add_vector_fp32(EMBEDDING_FIELD, &embedding)?;
    Ok(doc)
}

fn doc_to_record(doc: DocRef<'_>, dimension: usize) -> Result<VectorRecord> {
    let mut record = string_field(doc, "payload")?
        .and_then(|payload| serde_json::from_str::<VectorRecord>(&payload).ok())
        .unwrap_or_else(|| VectorRecord {
            id: string_field(doc, "id")
                .ok()
                .flatten()
                .unwrap_or_else(|| doc.pk_copy().unwrap_or_default()),
            kind: String::new(),
            scope: WorkScope::global(),
            text: string_field(doc, "text").ok().flatten().unwrap_or_default(),
            source: String::new(),
            file_path: String::new(),
            relative_path: String::new(),
            language: String::new(),
            artifact_name: String::new(),
            content_hash: String::new(),
            topic_key: String::new(),
            start_line: 0,
            end_line: 0,
            created_at: 0,
            updated_at: 0,
            embedding: vec![0.0; dimension],
        });
    record.embedding = doc
        .get_vector_fp32(EMBEDDING_FIELD)
        .unwrap_or_else(|_| record.embedding.clone());
    Ok(record)
}

fn add_string(doc: &mut Doc, field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        doc.set_field_null(field)?;
    } else {
        doc.add_string(field, value)?;
    }
    Ok(())
}

fn string_field(doc: DocRef<'_>, field: &str) -> Result<Option<String>> {
    if doc.is_field_null(field) {
        Ok(None)
    } else {
        Ok(doc.get_string(field)?)
    }
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn zvec_pk(id: &str) -> String {
    format!("pk_{}", stable_id(id))
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (a, b) in a.iter().zip(b.iter()) {
        let a = *a as f64;
        let b = *b as f64;
        dot += a * b;
        na += a * a;
        nb += b * b;
    }
    if na > 0.0 && nb > 0.0 {
        dot / (na.sqrt() * nb.sqrt())
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, RockyConfig};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_config() -> Config {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Config {
            port: 0,
            data_dir: std::env::temp_dir().join(format!("xenonite-zvec-test-{nonce}")),
            rocky: RockyConfig {
                llm_base_url: "http://127.0.0.1:1/v1".to_string(),
                llm_model: "test".to_string(),
                llm_api_key: String::new(),
                embed_base_url: "http://127.0.0.1:1/v1".to_string(),
                embed_model: "test".to_string(),
                embed_api_key: String::new(),
            },
        }
    }

    #[test]
    fn zvec_store_round_trips_work_scoped_vector_records() {
        let config = test_config();
        let store = ZvecStore::new(&config);
        let record = VectorRecord {
            id: "doc-1".to_string(),
            kind: "code".to_string(),
            scope: WorkScope {
                project_id: "project-1".to_string(),
                work_group_id: Some("group-1".to_string()),
                work_unit_id: Some("unit-1".to_string()),
                agent_id: Some("agent-1".to_string()),
            },
            text: "fn main() {}".to_string(),
            source: "unit-test".to_string(),
            file_path: "/tmp/main.rs".to_string(),
            relative_path: "main.rs".to_string(),
            language: "rust".to_string(),
            artifact_name: String::new(),
            content_hash: "hash".to_string(),
            topic_key: "main".to_string(),
            start_line: 1,
            end_line: 1,
            created_at: 1,
            updated_at: 2,
            embedding: vec![1.0, 0.0, 0.0],
        };

        let mut second = record.clone();
        second.id = "doc-2".to_string();
        second.text = "fn helper() {}".to_string();
        second.embedding = vec![0.0, 1.0, 0.0];
        store
            .upsert_records("test_collection", &[record, second])
            .unwrap();
        let hits = store
            .search("test_collection", &[1.0, 0.0, 0.0], 5)
            .unwrap();

        assert!(!hits.is_empty());
        assert_eq!(hits[0].record.id, "doc-1");
        assert_eq!(
            hits[0].record.scope.work_group_id.as_deref(),
            Some("group-1")
        );
        assert_eq!(hits[0].record.scope.work_unit_id.as_deref(), Some("unit-1"));
        assert_eq!(hits[0].record.scope.agent_id.as_deref(), Some("agent-1"));
        assert_eq!(hits[0].record.relative_path, "main.rs");
    }
}
