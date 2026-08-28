use crate::{
    account::save_index,
    config::{Config, RecommendStrategy, save_config},
    error::{AppError, Result},
    integration::CodexIntegration,
    paths::Paths,
    proxy::{ProxyRuntime, ProxyServer, ProxyState},
    storage::MetadataStore,
    types::AccountIndex,
};
use futures_util::stream;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Frame, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use parking_lot::RwLock;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    convert::Infallible,
    fs,
    net::SocketAddr,
    path::Path,
    sync::{Arc, atomic::Ordering},
};
use tokio::{net::TcpListener, sync::watch};
use uuid::Uuid;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
type ResponseBody = http_body_util::combinators::UnsyncBoxBody<Bytes, Infallible>;

fn full(body: impl Into<Bytes>) -> ResponseBody {
    Full::new(body.into()).boxed_unsync()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub level: String,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxySnapshot {
    pub running: bool,
    pub runtime_state: crate::proxy::RuntimeState,
    pub last_error: Option<String>,
    pub listen_addr: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub control_plane: String,
    pub database: String,
    pub database_counts: Option<(u64, u64)>,
    pub config_drift: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlSnapshot {
    pub protocol_version: u32,
    pub tenant_id: String,
    pub proxy: ProxySnapshot,
    pub stats: crate::proxy::ProxyStats,
    pub current_account: Option<Uuid>,
    pub accounts: Vec<crate::types::Account>,
    pub account_runtime: Vec<AccountRuntimeSnapshot>,
    pub eligible_accounts: usize,
    pub routing_paused: bool,
    pub active_requests: Vec<crate::proxy::InFlightRequest>,
    pub instances: Vec<InstanceSummary>,
    /// Legacy v1 field retained for clients that display the debug label.
    pub integration: String,
    pub integration_state: crate::integration::IntegrationStatus,
    pub health: HealthSnapshot,
    pub alerts: Vec<Alert>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountRuntimeSnapshot {
    pub account_id: Uuid,
    pub circuit_reason: Option<String>,
    pub bound_instances: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceSummary {
    pub client_instance_id: String,
    pub device_id: String,
    pub pid: Option<u32>,
    pub parent_pid: Option<u32>,
    pub executable: Option<std::path::PathBuf>,
    pub working_directory: Option<std::path::PathBuf>,
    pub active_requests: usize,
    pub oldest_request_ms: u64,
    pub current_account: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeDescriptor {
    pub protocol_version: u32,
    pub address: SocketAddr,
    pub pid: u32,
    pub bearer_token: String,
}

#[derive(Clone)]
pub struct ControlContext {
    pub config: Arc<RwLock<Config>>,
    pub accounts: Arc<RwLock<AccountIndex>>,
    pub current_account: Arc<RwLock<Option<Uuid>>>,
    pub proxy_server: Arc<ProxyServer>,
    pub proxy_state: ProxyState,
    pub proxy_runtime: ProxyRuntime,
    pub paths: Paths,
    pub shutdown: watch::Sender<bool>,
    pub metadata_store: Arc<MetadataStore>,
}

pub struct ControlServer {
    listener: TcpListener,
    token: String,
    context: ControlContext,
    shutdown_rx: watch::Receiver<bool>,
    runtime_path: std::path::PathBuf,
}

impl ControlServer {
    pub async fn bind(context: ControlContext) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let mut token_bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut token_bytes);
        let token: String = token_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let descriptor = RuntimeDescriptor {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            address,
            pid: std::process::id(),
            bearer_token: token.clone(),
        };
        atomic_write_private(
            &context.paths.runtime_file,
            &serde_json::to_vec_pretty(&descriptor)?,
        )?;
        let shutdown_rx = context.shutdown.subscribe();
        let runtime_path = context.paths.runtime_file.clone();
        Ok(Self {
            listener,
            token,
            context,
            shutdown_rx,
            runtime_path,
        })
    }

    pub async fn serve(mut self) -> Result<()> {
        loop {
            tokio::select! {
                changed = self.shutdown_rx.changed() => {
                    if changed.is_err() || *self.shutdown_rx.borrow() { break; }
                }
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted?;
                    let token = self.token.clone();
                    let context = self.context.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |request| {
                            let token = token.clone();
                            let context = context.clone();
                            async move { handle(request, &token, context).await }
                        });
                        let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
                    });
                }
            }
        }
        let _ = fs::remove_file(&self.runtime_path);
        Ok(())
    }
}

async fn handle(
    request: Request<Incoming>,
    token: &str,
    context: ControlContext,
) -> std::result::Result<Response<ResponseBody>, hyper::Error> {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {token}"));
    if !authorized {
        return Ok(json_response(
            StatusCode::UNAUTHORIZED,
            json!({"error":"unauthorized"}),
        ));
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    let response = match (method, path.as_str()) {
        (Method::GET, "/v1/snapshot") => {
            let config = context.config.read();
            let database_counts = context.metadata_store.counts().ok();
            let accounts = context.accounts.read().accounts.clone();
            let eligible_accounts = accounts
                .iter()
                .filter(|account| {
                    account.proxy_enabled
                        && account.status.kind == crate::types::StatusKind::Live
                        && account.status.checked_at.is_some_and(|checked| {
                            chrono::Utc::now()
                                .signed_duration_since(checked)
                                .num_seconds()
                                <= 90
                        })
                        && account
                            .status
                            .primary
                            .as_ref()
                            .is_some_and(|quota| quota.used_percent < config.proxy.threshold)
                })
                .count();
            let integration_status = CodexIntegration::new(&config.codex_home)
                .status()
                .unwrap_or(crate::integration::IntegrationStatus::Disabled);
            let config_drift = matches!(
                &integration_status,
                crate::integration::IntegrationStatus::Drifted(_)
            );
            let mut alerts = Vec::new();
            if eligible_accounts == 0 {
                alerts.push(Alert {
                    level: "error".into(),
                    title: "没有可路由账户".into(),
                    detail: "检测账户并按 Space 加入代理池".into(),
                });
            }
            if config_drift {
                alerts.push(Alert {
                    level: "error".into(),
                    title: "Codex 配置发生漂移".into(),
                    detail: "打开接入详情处理冲突，工具不会覆盖外部修改".into(),
                });
            }
            if let Some(error) = context.proxy_runtime.last_error()
                && (eligible_accounts > 0
                    || context.proxy_runtime.state() == crate::proxy::RuntimeState::Error)
            {
                alerts.push(Alert {
                    level: "error".into(),
                    title: "代理运行异常".into(),
                    detail: error,
                });
            }
            let active_requests = context.proxy_server.connection_tracker().in_flight();
            let instances = summarize_instances(&active_requests, context.proxy_server.router());
            let binding_counts = context.proxy_server.router().binding_counts();
            let account_runtime = accounts
                .iter()
                .map(|account| AccountRuntimeSnapshot {
                    account_id: account.id,
                    circuit_reason: context
                        .proxy_server
                        .router()
                        .circuit_reason(account.id)
                        .map(|reason| format!("{reason:?}")),
                    bound_instances: binding_counts.get(&account.id).copied().unwrap_or(0),
                })
                .collect();
            json_response(
                StatusCode::OK,
                serde_json::to_value(ControlSnapshot {
                    protocol_version: CONTROL_PROTOCOL_VERSION,
                    tenant_id: "local".into(),
                    proxy: ProxySnapshot {
                        running: context.proxy_state.running.load(Ordering::Relaxed),
                        runtime_state: context.proxy_runtime.state(),
                        last_error: context.proxy_runtime.last_error(),
                        listen_addr: config.proxy.listen_addr.clone(),
                    },
                    stats: context.proxy_state.stats.read().clone(),
                    current_account: *context.current_account.read(),
                    accounts,
                    account_runtime,
                    eligible_accounts,
                    routing_paused: context.proxy_server.router().is_paused(),
                    active_requests,
                    instances,
                    integration: format!("{integration_status:?}"),
                    integration_state: integration_status,
                    health: HealthSnapshot {
                        control_plane: "connected".into(),
                        database: if database_counts.is_some() {
                            "ok"
                        } else {
                            "error"
                        }
                        .into(),
                        database_counts,
                        config_drift,
                    },
                    alerts,
                })
                .unwrap(),
            )
        }
        (Method::GET, "/v1/metrics") => {
            if request.uri().query().is_none() {
                return Ok(json_response(
                    StatusCode::OK,
                    serde_json::to_value(context.proxy_state.stats.read().clone()).unwrap(),
                ));
            }
            let window = query_number(request.uri().query(), "window").unwrap_or(300);
            let bucket = query_number(request.uri().query(), "bucket").unwrap_or(10);
            match context.metadata_store.metrics(window, bucket) {
                Ok(metrics) => {
                    json_response(StatusCode::OK, serde_json::to_value(metrics).unwrap())
                }
                Err(error) => json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error":error.to_string()}),
                ),
            }
        }
        (Method::GET, "/v1/events") => {
            let query = request.uri().query();
            let offset = query_number(query, "cursor").unwrap_or(0) as usize;
            let account = query_value(query, "account").and_then(|id| Uuid::parse_str(id).ok());
            let instance = query_value(query, "instance");
            let kind = query_value(query, "kind");
            match context.metadata_store.recent_events(500, 0) {
                Ok(all_items) => {
                    let mut items = all_items
                        .into_iter()
                        .filter(|event| account.is_none_or(|id| event.account_id == Some(id)))
                        .filter(|event| {
                            instance
                                .is_none_or(|id| event.client_instance_id.as_deref() == Some(id))
                        })
                        .filter(|event| kind.is_none_or(|kind| event.kind == kind))
                        .skip(offset)
                        .take(101)
                        .collect::<Vec<_>>();
                    let has_more = items.len() > 100;
                    items.truncate(100);
                    let next = has_more.then(|| (offset + items.len()).to_string());
                    json_response(StatusCode::OK, json!({"items":items,"next_cursor":next}))
                }
                Err(error) => json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error":error.to_string()}),
                ),
            }
        }
        (Method::GET, "/v1/requests") => {
            let query = request.uri().query();
            let offset = query_number(query, "cursor").unwrap_or(0) as usize;
            let status = query_number(query, "status").map(|status| status as u16);
            let account_id = query_value(query, "account").and_then(|id| Uuid::parse_str(id).ok());
            let instance = query_value(query, "instance");
            match context
                .metadata_store
                .recent_requests(101, offset, account_id, instance, status)
            {
                Ok(mut items) => {
                    let has_more = items.len() > 100;
                    items.truncate(100);
                    let next = has_more.then(|| (offset + items.len()).to_string());
                    json_response(StatusCode::OK, json!({"items":items,"next_cursor":next}))
                }
                Err(error) => json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error":error.to_string()}),
                ),
            }
        }
        (Method::GET, "/v1/stream") => {
            let events = stream::unfold(context.clone(), |context| async move {
                let snapshot = json!({
                    "runtime_state": context.proxy_runtime.state(),
                    "stats": context.proxy_state.stats.read().clone(),
                    "active_requests": context.proxy_server.connection_tracker().in_flight(),
                });
                let metrics = context.metadata_store.metrics(300, 10).unwrap_or_default();
                let requests = context
                    .metadata_store
                    .recent_requests(10, 0, None, None, None)
                    .unwrap_or_default();
                let runtime_events = context
                    .metadata_store
                    .recent_events(10, 0)
                    .unwrap_or_default();
                let payload = format!(
                    "event: snapshot\ndata: {snapshot}\n\nevent: metrics\ndata: {}\n\nevent: requests\ndata: {}\n\nevent: events\ndata: {}\n\n",
                    serde_json::to_value(metrics).unwrap(),
                    serde_json::to_value(requests).unwrap(),
                    serde_json::to_value(runtime_events).unwrap(),
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                Some((
                    Ok::<_, Infallible>(Frame::data(Bytes::from(payload))),
                    context,
                ))
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .body(StreamBody::new(events).boxed_unsync())
                .unwrap()
        }
        (Method::PATCH, "/v1/config") => match collect_json::<ConfigPatch>(request).await {
            Ok(patch) => {
                let mut config = context.config.write().clone();
                if let Some(value) = patch.auto_switch {
                    config.proxy.auto_switch = value;
                }
                if let Some(value) = patch.threshold {
                    if !(1.0..=100.0).contains(&value) {
                        return Ok(json_response(
                            StatusCode::BAD_REQUEST,
                            json!({"error":"threshold must be 1..=100"}),
                        ));
                    }
                    config.proxy.threshold = value;
                }
                if let Some(value) = patch.strategy {
                    config.proxy.strategy = value;
                }
                context
                    .proxy_server
                    .router()
                    .update_config(config.proxy.clone());
                let result = save_config(&context.paths, &config);
                if result.is_ok() {
                    *context.config.write() = config;
                    record_control_event(&context, "config_updated", None, "代理热配置已更新");
                }
                operation_result(result)
            }
            Err(error) => {
                json_response(StatusCode::BAD_REQUEST, json!({"error":error.to_string()}))
            }
        },
        (Method::POST, "/v1/proxy/pause") => {
            context.proxy_runtime.pause();
            record_control_event(&context, "proxy_paused", None, "路由已暂停");
            json_response(StatusCode::OK, json!({"ok":true}))
        }
        (Method::POST, "/v1/proxy/resume") => {
            context.proxy_runtime.resume();
            record_control_event(&context, "proxy_resumed", None, "路由已恢复");
            json_response(StatusCode::OK, json!({"ok":true}))
        }
        (Method::POST, "/v1/proxy/start") => {
            let eligible = context.accounts.read().accounts.iter().any(|account| {
                account.proxy_enabled
                    && account.status.kind == crate::types::StatusKind::Live
                    && account.status.checked_at.is_some_and(|checked| {
                        chrono::Utc::now()
                            .signed_duration_since(checked)
                            .num_seconds()
                            <= 90
                    })
                    && account.status.primary.as_ref().is_some_and(|quota| {
                        quota.used_percent < context.config.read().proxy.threshold
                    })
            });
            if !eligible {
                context
                    .proxy_runtime
                    .mark_blocked("没有已入池且健康的账户，无法启动代理");
                json_response(
                    StatusCode::CONFLICT,
                    json!({"error":"请先将至少一个刚检测为可用的账户加入代理池"}),
                )
            } else {
                context.proxy_runtime.start();
                record_control_event(&context, "proxy_started", None, "数据代理已启动");
                let mut config = context.config.read().clone();
                config.proxy.enabled = true;
                if save_config(&context.paths, &config).is_ok() {
                    *context.config.write() = config;
                }
                json_response(StatusCode::OK, json!({"ok":true}))
            }
        }
        (Method::POST, "/v1/proxy/stop") => {
            let drained = context
                .proxy_runtime
                .stop(std::time::Duration::from_secs(30))
                .await;
            record_control_event(
                &context,
                "proxy_stopped",
                None,
                if drained {
                    "数据代理已排空并停止"
                } else {
                    "数据代理排空超时后停止"
                },
            );
            let mut config = context.config.read().clone();
            config.proxy.enabled = false;
            if save_config(&context.paths, &config).is_ok() {
                *context.config.write() = config;
            }
            json_response(StatusCode::OK, json!({"ok":true,"drained":drained}))
        }
        (Method::POST, "/v1/integration/enable") => {
            let result = CodexIntegration::new(&context.config.read().codex_home).enable();
            if result.is_ok() {
                record_control_event(&context, "integration_enabled", None, "已启用 Codex 接入");
            }
            operation_result(result)
        }
        (Method::POST, "/v1/integration/disable") => {
            let result = CodexIntegration::new(&context.config.read().codex_home).disable();
            if result.is_ok() {
                record_control_event(&context, "integration_disabled", None, "已停用 Codex 接入");
            }
            operation_result(result)
        }
        (Method::POST, "/v1/daemon/stop") => {
            let _ = context.shutdown.send(true);
            json_response(StatusCode::OK, json!({"ok":true}))
        }
        (Method::POST, "/v1/daemon/reload") => {
            match (
                crate::config::load_config(&context.paths),
                crate::account::load_index(&context.paths),
            ) {
                (Ok(config), Ok(index)) => {
                    context
                        .proxy_server
                        .router()
                        .update_config(config.proxy.clone());
                    *context.config.write() = config;
                    *context.accounts.write() = index;
                    json_response(StatusCode::OK, json!({"ok":true}))
                }
                (Err(error), _) | (_, Err(error)) => {
                    json_response(StatusCode::CONFLICT, json!({"error":error.to_string()}))
                }
            }
        }
        (Method::POST, _) if path.starts_with("/v1/accounts/") && path.ends_with("/pool") => {
            let id = path
                .trim_start_matches("/v1/accounts/")
                .trim_end_matches("/pool")
                .trim_end_matches('/');
            match (
                Uuid::parse_str(id),
                collect_json::<PoolPatch>(request).await,
            ) {
                (Ok(id), Ok(patch)) => {
                    let mut index = context.accounts.read().clone();
                    match index.accounts.iter_mut().find(|account| account.id == id) {
                        Some(account) => {
                            account.proxy_enabled = patch.enabled;
                            if patch.enabled {
                                context.proxy_server.router().close_circuit(id);
                            }
                            let result = save_index(&context.paths, &index);
                            if result.is_ok() {
                                *context.accounts.write() = index;
                                record_control_event(
                                    &context,
                                    if patch.enabled {
                                        "pool_joined"
                                    } else {
                                        "pool_left"
                                    },
                                    Some(id),
                                    if patch.enabled {
                                        "账户加入代理池"
                                    } else {
                                        "账户移出代理池"
                                    },
                                );
                            }
                            operation_result(result)
                        }
                        None => json_response(
                            StatusCode::NOT_FOUND,
                            json!({"error":"account not found"}),
                        ),
                    }
                }
                _ => json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error":"invalid account or body"}),
                ),
            }
        }
        (Method::POST, _) if path.starts_with("/v1/accounts/") && path.ends_with("/switch") => {
            let id = path
                .trim_start_matches("/v1/accounts/")
                .trim_end_matches("/switch")
                .trim_end_matches('/');
            match Uuid::parse_str(id) {
                Ok(id)
                    if context
                        .accounts
                        .read()
                        .accounts
                        .iter()
                        .any(|account| account.id == id && account.proxy_enabled) =>
                {
                    *context.current_account.write() = Some(id);
                    context.proxy_state.stats.write().current_account = Some(id);
                    context.proxy_server.router().prefer(id);
                    record_control_event(
                        &context,
                        "manual_switch",
                        Some(id),
                        "请求在下一安全边界切换账户",
                    );
                    json_response(StatusCode::OK, json!({"ok":true}))
                }
                _ => json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error":"account unavailable or not in pool"}),
                ),
            }
        }
        _ => json_response(StatusCode::NOT_FOUND, json!({"error":"not found"})),
    };
    Ok(response)
}

#[derive(Deserialize)]
struct ConfigPatch {
    auto_switch: Option<bool>,
    threshold: Option<f64>,
    strategy: Option<RecommendStrategy>,
}
#[derive(Deserialize)]
struct PoolPatch {
    enabled: bool,
}

async fn collect_json<T: for<'de> Deserialize<'de>>(request: Request<Incoming>) -> Result<T> {
    let bytes = request
        .into_body()
        .collect()
        .await
        .map_err(|_| AppError::Message("invalid request body".into()))?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(AppError::from)
}

fn operation_result(result: Result<()>) -> Response<ResponseBody> {
    match result {
        Ok(()) => json_response(
            StatusCode::OK,
            json!({"ok":true,"restart_codex_required":true}),
        ),
        Err(error) => json_response(
            StatusCode::CONFLICT,
            json!({"error":crate::storage::sanitize(&error.to_string())}),
        ),
    }
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(full(value.to_string()))
        .unwrap()
}

fn query_value<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn query_number(query: Option<&str>, key: &str) -> Option<u64> {
    query_value(query, key)?.parse().ok()
}

fn record_control_event(
    context: &ControlContext,
    kind: &str,
    account_id: Option<Uuid>,
    detail: &str,
) {
    let _ = context
        .metadata_store
        .record_event(&crate::storage::RuntimeEvent {
            id: Uuid::new_v4().to_string(),
            occurred_at: chrono::Utc::now(),
            tenant_id: "local".into(),
            device_id: std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "local-device".into()),
            client_instance_id: None,
            kind: kind.into(),
            account_id,
            detail: detail.into(),
        });
}

fn summarize_instances(
    requests: &[crate::proxy::InFlightRequest],
    router: &crate::proxy::Router,
) -> Vec<InstanceSummary> {
    let mut instances: HashMap<String, InstanceSummary> = HashMap::new();
    for request in requests {
        let identity = &request.identity;
        let entry = instances
            .entry(identity.client_instance_id.0.clone())
            .or_insert_with(|| InstanceSummary {
                client_instance_id: identity.client_instance_id.0.clone(),
                device_id: identity.device_id.0.clone(),
                pid: identity.process.pid,
                parent_pid: identity.process.parent_pid,
                executable: identity.process.executable.clone(),
                working_directory: identity.process.working_directory.clone(),
                active_requests: 0,
                oldest_request_ms: 0,
                current_account: router.binding_for(&identity.sticky_key()),
            });
        entry.active_requests += 1;
        entry.oldest_request_ms = entry.oldest_request_ms.max(request.elapsed_ms);
    }
    let mut instances = instances.into_values().collect::<Vec<_>>();
    instances.sort_by(|left, right| {
        right
            .active_requests
            .cmp(&left.active_requests)
            .then_with(|| left.client_instance_id.cmp(&right.client_instance_id))
    });
    instances
}

pub fn read_runtime(paths: &Paths) -> Result<RuntimeDescriptor> {
    let raw = fs::read(&paths.runtime_file)?;
    let descriptor: RuntimeDescriptor = serde_json::from_slice(&raw)?;
    if descriptor.protocol_version != CONTROL_PROTOCOL_VERSION {
        return Err(AppError::Message(format!(
            "控制协议版本不兼容：{}",
            descriptor.protocol_version
        )));
    }
    if !descriptor.address.ip().is_loopback() {
        return Err(AppError::Message("拒绝非回环控制地址".into()));
    }
    Ok(descriptor)
}

pub async fn control_request(
    paths: &Paths,
    method: Method,
    endpoint: &str,
) -> Result<serde_json::Value> {
    control_request_json(paths, method, endpoint, None).await
}

pub async fn control_request_json(
    paths: &Paths,
    method: Method,
    endpoint: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value> {
    let descriptor = read_runtime(paths)?;
    let request = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?
        .request(
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
            format!("http://{}{}", descriptor.address, endpoint),
        )
        .bearer_auth(descriptor.bearer_token);
    let response = if let Some(body) = body {
        request.json(body)
    } else {
        request
    }
    .send()
    .await?;
    let status = response.status();
    let value = response
        .json()
        .await
        .unwrap_or_else(|_| json!({"error":"invalid control response"}));
    if status.is_success() {
        Ok(value)
    } else {
        Err(AppError::Message(format!(
            "控制请求失败 HTTP {}：{}",
            status.as_u16(),
            value
        )))
    }
}

pub async fn control_stream(paths: &Paths) -> Result<reqwest::Response> {
    let descriptor = read_runtime(paths)?;
    let response = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()?
        .get(format!("http://{}/v1/stream", descriptor.address))
        .bearer_auth(descriptor.bearer_token)
        .send()
        .await?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(AppError::Message(format!(
            "控制事件流失败 HTTP {}",
            response.status().as_u16()
        )))
    }
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&temp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    crate::filesystem::atomic_replace(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::ProxyState;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn control_api_requires_runtime_bearer_token() {
        let root = std::env::temp_dir().join(format!("codex-switcher-control-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let paths = Paths {
            config_file: root.join("config.toml"),
            index_file: root.join("accounts.toml"),
            config_dir: root.clone(),
            pid_file: root.join("daemon.pid"),
            runtime_file: root.join("runtime.json"),
            database_file: root.join("runtime.sqlite3"),
        };
        let mut config = Config::defaults();
        config.accounts_dir = root.join("accounts");
        let accounts = Arc::new(RwLock::new(AccountIndex::default()));
        let current = Arc::new(RwLock::new(None));
        let proxy_state = ProxyState::new();
        let proxy = Arc::new(ProxyServer::new(
            config.clone(),
            config.proxy.clone(),
            accounts.clone(),
            current.clone(),
            proxy_state.stats.clone(),
        ));
        let proxy_runtime = ProxyRuntime::new(proxy.clone(), proxy_state.clone());
        let (shutdown, _) = watch::channel(false);
        let metadata_store = Arc::new(
            crate::storage::MetadataStore::open(
                &paths.database_file,
                crate::storage::RetentionPolicy::default(),
            )
            .unwrap(),
        );
        let server = ControlServer::bind(ControlContext {
            config: Arc::new(RwLock::new(config)),
            accounts,
            current_account: current,
            proxy_server: proxy,
            proxy_state,
            proxy_runtime,
            paths: paths.clone(),
            shutdown: shutdown.clone(),
            metadata_store,
        })
        .await
        .unwrap();
        let descriptor = read_runtime(&paths).unwrap();
        let task = tokio::spawn(server.serve());
        let client = reqwest::Client::new();
        let url = format!("http://{}/v1/snapshot", descriptor.address);
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&url)
                .bearer_auth(&descriptor.bearer_token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let snapshot_text = client
            .get(&url)
            .bearer_auth(&descriptor.bearer_token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .to_ascii_lowercase();
        for forbidden in [
            "authorization",
            "access_token",
            "refresh_token",
            "request_body",
            "response_body",
        ] {
            assert!(
                !snapshot_text.contains(forbidden),
                "leaked field {forbidden}"
            );
        }
        assert_eq!(
            client
                .post(format!("http://{}/v1/proxy/start", descriptor.address))
                .bearer_auth(&descriptor.bearer_token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            client
                .get(format!(
                    "http://{}/v1/metrics?window=300&bucket=10",
                    descriptor.address
                ))
                .bearer_auth(&descriptor.bearer_token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let stream_response = client
            .get(format!("http://{}/v1/stream", descriptor.address))
            .bearer_auth(&descriptor.bearer_token)
            .send()
            .await
            .unwrap();
        assert_eq!(stream_response.status(), StatusCode::OK);
        let mut stream = stream_response.bytes_stream();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("event: snapshot"));
        let _ = shutdown.send(true);
        task.await.unwrap().unwrap();
        assert!(!paths.runtime_file.exists());
    }
}
