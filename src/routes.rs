use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::code_graph::{self, GraphRequest};
use crate::code_index::{self, CodeRequest};
use crate::context::{self, ContextRequest};
use crate::memory::{self, MemoryRequest, OptimizeMemoryRequest};
use crate::rocky;
use crate::storage::{StorageStatus, ZvecStore};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/config", get(service_config))
        .route("/v1/memory/recall", post(recall_memory))
        .route("/v1/memory/store", post(store_memory))
        .route("/v1/memory/optimize", post(optimize_memory))
        .route("/v1/code/status", get(code_status_get).post(code_status))
        .route("/v1/code/index", post(code_index))
        .route("/v1/code/update", post(code_update))
        .route("/v1/code/remove", post(code_remove))
        .route("/v1/code/stop", post(code_stop))
        .route("/v1/code/watch", post(code_watch))
        .route("/v1/code/search", post(code_search))
        .route("/v1/graph/build", post(graph_build))
        .route("/v1/graph/query", post(graph_query))
        .route("/v1/graph/stats", post(graph_stats))
        .route("/v1/graph/circular", post(graph_circular))
        .route("/v1/graph/visualize", post(graph_visualize))
        .route("/v1/graph/remove", post(graph_remove))
        .route("/v1/graph/status", post(graph_status))
        .route("/v1/graph/impact", post(graph_impact))
        .route("/v1/graph/flow", post(graph_flow))
        .route("/v1/graph/symbol", post(graph_symbol))
        .route("/v1/graph/symbols", post(graph_symbols))
        .route("/v1/context", post(context_bundle))
        .route("/v1/context/search", post(context_search))
        .route("/v1/context/index", post(context_index))
        .route("/v1/context/remove", post(context_remove))
        .route("/v1/code/op", post(code_op))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let rocky_health = rocky::health(&state.config.rocky).await;
    Json(json!({
        "ok": true,
        "service": "xenonite",
        "rocky": rocky_health,
        "storage": ZvecStore::new(&state.config).status()
    }))
}

async fn service_config(State(state): State<AppState>) -> Json<ServiceConfigResponse> {
    let rocky_health = rocky::health(&state.config.rocky).await;
    Json(ServiceConfigResponse {
        name: "xenonite-api",
        service: "xenonite",
        port: state.config.port,
        data_dir: state.config.data_dir.display().to_string(),
        rocky: RockyConfigResponse {
            llm_base_url: state.config.rocky.llm_base_url.clone(),
            embed_base_url: state.config.rocky.embed_base_url.clone(),
            health: json!(rocky_health),
        },
        storage: ZvecStore::new(&state.config).status(),
    })
}

async fn recall_memory(
    State(state): State<AppState>,
    Json(request): Json<MemoryRequest>,
) -> Response {
    match memory::recall(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn store_memory(
    State(state): State<AppState>,
    Json(request): Json<MemoryRequest>,
) -> Response {
    match memory::store(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn optimize_memory(
    State(state): State<AppState>,
    Json(request): Json<OptimizeMemoryRequest>,
) -> Response {
    match memory::optimize(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

fn error_response(error: anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "ok": false,
            "error": error.to_string()
        })),
    )
        .into_response()
}

async fn code_index(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    match code_index::index_project(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn code_search(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    match code_index::search(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn code_status(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    match code_index::status(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn code_status_get(
    State(state): State<AppState>,
    axum::extract::Query(request): axum::extract::Query<CodeRequest>,
) -> Response {
    match code_index::status(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn code_update(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    match code_index::update_project(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn code_remove(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    match code_index::remove_project(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn code_stop(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    stop_watcher(state, request).await
}

async fn code_watch(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    match request.action.as_deref().unwrap_or("start") {
        "start" => start_watcher(state, request).await,
        "stop" => stop_watcher(state, request).await,
        "status" => watcher_status(state, request).await,
        action => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": format!("unknown watch action: {action}")
            })),
        )
            .into_response(),
    }
}

async fn code_op(State(state): State<AppState>, Json(request): Json<CodeRequest>) -> Response {
    let op = request.op.clone().unwrap_or_default();
    let args = request.args.clone().unwrap_or_else(|| json!({}));
    let nested_request = serde_json::from_value::<CodeRequest>(args).unwrap_or(CodeRequest {
        op: None,
        args: None,
        action: request.action.clone(),
        project_path: request.project_path.clone(),
        query: request.query.clone(),
        limit: request.limit,
        file_filter: request.file_filter.clone(),
        language_filter: request.language_filter.clone(),
        min_score: request.min_score,
        include_linked: request.include_linked,
        extra_extensions: request.extra_extensions.clone(),
        work_group_id: request.work_group_id.clone(),
        work_group_id_camel: request.work_group_id_camel.clone(),
        work_unit_id: request.work_unit_id.clone(),
        work_unit_id_camel: request.work_unit_id_camel.clone(),
        agent_id: request.agent_id.clone(),
        agent_id_camel: request.agent_id_camel.clone(),
    });
    match op.as_str() {
        "codebase_index" => match code_index::index_project(&state.config, nested_request).await {
            Ok(response) => Json(response).into_response(),
            Err(error) => error_response(error),
        },
        "codebase_update" => {
            match code_index::update_project(&state.config, nested_request).await {
                Ok(response) => Json(response).into_response(),
                Err(error) => error_response(error),
            }
        }
        "codebase_remove" => match code_index::remove_project(&state.config, nested_request) {
            Ok(response) => Json(response).into_response(),
            Err(error) => error_response(error),
        },
        "codebase_stop" => stop_watcher(state, nested_request).await,
        "codebase_watch" => match nested_request.action.as_deref().unwrap_or("start") {
            "start" => start_watcher(state, nested_request).await,
            "stop" => stop_watcher(state, nested_request).await,
            "status" => watcher_status(state, nested_request).await,
            action => (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "error": format!("unknown watch action: {action}")
                })),
            )
                .into_response(),
        },
        "codebase_search" => match code_index::search(&state.config, nested_request).await {
            Ok(response) => Json(response).into_response(),
            Err(error) => error_response(error),
        },
        "codebase_status" => match code_index::status(&state.config, nested_request) {
            Ok(response) => Json(response).into_response(),
            Err(error) => error_response(error),
        },
        "codebase_graph_build" => graph_op(&state, code_graph::build, request.args),
        "codebase_graph_query" => graph_op(&state, code_graph::query, request.args),
        "codebase_graph_stats" => graph_op(&state, code_graph::stats, request.args),
        "codebase_graph_circular" => graph_op(&state, code_graph::circular, request.args),
        "codebase_graph_visualize" => graph_op(&state, code_graph::visualize, request.args),
        "codebase_graph_remove" => graph_op(&state, code_graph::remove, request.args),
        "codebase_graph_status" => graph_op(&state, code_graph::status, request.args),
        "codebase_symbol" => graph_op(&state, code_graph::symbol, request.args),
        "codebase_symbols" => graph_op(&state, code_graph::symbols, request.args),
        "codebase_impact" => graph_op(&state, code_graph::impact, request.args),
        "codebase_flow" => graph_op(&state, code_graph::flow, request.args),
        "codebase_context" => {
            match context::bundle(&state.config, context_request(request.args)).await {
                Ok(response) => Json(response).into_response(),
                Err(error) => error_response(error),
            }
        }
        "codebase_context_search" => {
            match context::search(&state.config, context_request(request.args)).await {
                Ok(response) => Json(response).into_response(),
                Err(error) => error_response(error),
            }
        }
        "codebase_context_index" => {
            match context::index(&state.config, context_request(request.args)).await {
                Ok(response) => Json(response).into_response(),
                Err(error) => error_response(error),
            }
        }
        "codebase_context_remove" => context_op(&state, context::remove, request.args),
        "codebase_health" => Json(json!({
            "ok": true,
            "service": "xenonite",
            "engine": "xenonite-rs",
            "dataDir": state.config.data_dir.display().to_string()
        }))
        .into_response(),
        "codebase_list_projects" => match code_index::list_projects(&state.config) {
            Ok(response) => Json(response).into_response(),
            Err(error) => error_response(error),
        },
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": format!("unknown op: {op}")
            })),
        )
            .into_response(),
    }
}

async fn start_watcher(state: AppState, request: CodeRequest) -> Response {
    let (project_key, already_active) =
        match ensure_watcher_for_project(state, request.project_path.clone()).await {
            Ok(result) => result,
            Err(error) => return error_response(error),
        };

    Json(code_index::ControlResponse {
        ok: true,
        project_path: project_key,
        status: "active".to_string(),
        message: if already_active {
            "Rust watcher is already active for this project.".to_string()
        } else {
            "Rust watcher started; project index refreshes in the background.".to_string()
        },
    })
    .into_response()
}

async fn ensure_watcher_for_project(
    state: AppState,
    project_path: Option<String>,
) -> anyhow::Result<(String, bool)> {
    let project_path = code_index::resolve_project_path(project_path)?;
    let project_key = project_path.display().to_string();
    let mut watchers = state.watchers.lock().await;
    let already_active = !watchers.insert(project_key.clone());
    drop(watchers);

    if !already_active {
        let config = state.config.clone();
        let watcher_registry = state.watchers.clone();
        let project_path_for_task = project_key.clone();
        tokio::spawn(async move {
            loop {
                let still_active = watcher_registry
                    .lock()
                    .await
                    .contains(&project_path_for_task);
                if !still_active {
                    break;
                }
                let _ = code_index::update_project(
                    &config,
                    CodeRequest {
                        op: None,
                        args: None,
                        action: None,
                        project_path: Some(project_path_for_task.clone()),
                        query: None,
                        limit: None,
                        file_filter: None,
                        language_filter: None,
                        min_score: None,
                        include_linked: None,
                        extra_extensions: None,
                        work_group_id: None,
                        work_group_id_camel: None,
                        work_unit_id: None,
                        work_unit_id_camel: None,
                        agent_id: None,
                        agent_id_camel: None,
                    },
                )
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }

    Ok((project_key, already_active))
}

async fn stop_watcher(state: AppState, request: CodeRequest) -> Response {
    let project_path = match code_index::resolve_project_path(request.project_path.clone()) {
        Ok(path) => path,
        Err(error) => return error_response(error),
    };
    let project_key = project_path.display().to_string();
    let removed = state.watchers.lock().await.remove(&project_key);

    Json(code_index::ControlResponse {
        ok: true,
        project_path: project_key,
        status: if removed { "stopped" } else { "idle" }.to_string(),
        message: if removed {
            "Rust watcher stopped for this project.".to_string()
        } else {
            "No Rust watcher was active for this project.".to_string()
        },
    })
    .into_response()
}

async fn watcher_status(state: AppState, request: CodeRequest) -> Response {
    let project_path = match code_index::resolve_project_path(request.project_path.clone()) {
        Ok(path) => path,
        Err(error) => return error_response(error),
    };
    let project_key = project_path.display().to_string();
    let active = state.watchers.lock().await.contains(&project_key);

    Json(code_index::ControlResponse {
        ok: true,
        project_path: project_key,
        status: if active { "active" } else { "idle" }.to_string(),
        message: if active {
            "Rust watcher is active for this project.".to_string()
        } else {
            "No Rust watcher is active for this project.".to_string()
        },
    })
    .into_response()
}

fn context_request(args: Option<Value>) -> ContextRequest {
    serde_json::from_value::<ContextRequest>(args.unwrap_or_else(|| json!({}))).unwrap_or(
        ContextRequest {
            project_path: None,
            query: None,
            limit: None,
            min_score: None,
            artifact_name: None,
            work_group_id: None,
            work_group_id_camel: None,
            work_unit_id: None,
            work_unit_id_camel: None,
            agent_id: None,
            agent_id_camel: None,
        },
    )
}

fn context_op<T: Serialize>(
    state: &AppState,
    handler: fn(&crate::config::Config, ContextRequest) -> anyhow::Result<T>,
    args: Option<Value>,
) -> Response {
    let request = context_request(args);
    match handler(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn context_bundle(
    State(state): State<AppState>,
    Json(request): Json<ContextRequest>,
) -> Response {
    match context::bundle(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn context_search(
    State(state): State<AppState>,
    Json(request): Json<ContextRequest>,
) -> Response {
    match context::search(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn context_index(
    State(state): State<AppState>,
    Json(request): Json<ContextRequest>,
) -> Response {
    match context::index(&state.config, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn context_remove(
    State(state): State<AppState>,
    Json(request): Json<ContextRequest>,
) -> Response {
    match context::remove(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

fn graph_op<T: Serialize>(
    state: &AppState,
    handler: fn(&crate::config::Config, GraphRequest) -> anyhow::Result<T>,
    args: Option<Value>,
) -> Response {
    let request = serde_json::from_value::<GraphRequest>(args.unwrap_or_else(|| json!({})))
        .unwrap_or(GraphRequest {
            project_path: None,
            file_path: None,
            file: None,
            target: None,
            entrypoint: None,
            name: None,
            query: None,
            depth: None,
            limit: None,
        });
    match handler(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_build(State(state): State<AppState>, Json(request): Json<GraphRequest>) -> Response {
    match code_graph::build(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_query(State(state): State<AppState>, Json(request): Json<GraphRequest>) -> Response {
    match code_graph::query(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_stats(State(state): State<AppState>, Json(request): Json<GraphRequest>) -> Response {
    match code_graph::stats(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_status(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::status(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_circular(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::circular(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_visualize(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::visualize(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_remove(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::remove(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_symbol(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::symbol(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_impact(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::impact(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_flow(State(state): State<AppState>, Json(request): Json<GraphRequest>) -> Response {
    match code_graph::flow(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn graph_symbols(
    State(state): State<AppState>,
    Json(request): Json<GraphRequest>,
) -> Response {
    match code_graph::symbols(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Serialize)]
struct ServiceConfigResponse {
    name: &'static str,
    service: &'static str,
    port: u16,
    #[serde(rename = "dataDir")]
    data_dir: String,
    rocky: RockyConfigResponse,
    storage: StorageStatus,
}

#[derive(Serialize)]
struct RockyConfigResponse {
    #[serde(rename = "llmBaseUrl")]
    llm_base_url: String,
    #[serde(rename = "embedBaseUrl")]
    embed_base_url: String,
    health: Value,
}
