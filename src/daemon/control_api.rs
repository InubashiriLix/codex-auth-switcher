use crate::{
    account::save_index,
    config::{Config, RecommendStrategy, save_config},
    error::{AppError, Result},
    integration::CodexIntegration,
    paths::Paths,
    proxy::{ProxyServer, ProxyState},
    storage::MetadataStore,
    types::AccountIndex,
};
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
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
    fs,
    net::SocketAddr,
    path::Path,
    sync::{Arc, atomic::Ordering},
};
use tokio::{net::TcpListener, sync::watch};
use uuid::Uuid;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
type ResponseBody = Full<Bytes>;

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
            json_response(
                StatusCode::OK,
                json!({
                    "protocol_version": CONTROL_PROTOCOL_VERSION,
                    "tenant_id": "local",
                    "proxy": {"running": context.proxy_state.running.load(Ordering::Relaxed), "listen_addr": config.proxy.listen_addr},
                    "stats": context.proxy_state.stats.read().clone(),
                    "current_account": *context.current_account.read(),
                    "accounts": context.accounts.read().accounts,
                    "routing_paused": context.proxy_server.router().is_paused(),
                    "active_requests": context.proxy_server.connection_tracker().in_flight(),
                    "integration": format!("{:?}", CodexIntegration::new(&config.codex_home).status().unwrap_or(crate::integration::IntegrationStatus::Disabled)),
                    "health": {
                        "control_plane": "connected",
                        "database": if database_counts.is_some() { "ok" } else { "error" },
                        "database_counts": database_counts,
                        "config_drift": matches!(CodexIntegration::new(&config.codex_home).status(), Ok(crate::integration::IntegrationStatus::Drifted(_))),
                    },
                }),
            )
        }
        (Method::GET, "/v1/metrics") => json_response(
            StatusCode::OK,
            serde_json::to_value(context.proxy_state.stats.read().clone()).unwrap(),
        ),
        (Method::GET, "/v1/events") => {
            let offset = request
                .uri()
                .query()
                .and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("cursor="))
                })
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            match context.metadata_store.recent_events(100, offset) {
                Ok(items) => {
                    let next = (items.len() == 100).then(|| (offset + items.len()).to_string());
                    json_response(StatusCode::OK, json!({"items":items,"next_cursor":next}))
                }
                Err(error) => json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error":error.to_string()}),
                ),
            }
        }
        (Method::GET, "/v1/stream") => {
            let payload = format!(
                "event: snapshot\ndata: {}\n\n",
                json!({"stats":context.proxy_state.stats.read().clone()})
            );
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Full::new(Bytes::from(payload)))
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
                }
                operation_result(result)
            }
            Err(error) => {
                json_response(StatusCode::BAD_REQUEST, json!({"error":error.to_string()}))
            }
        },
        (Method::POST, "/v1/proxy/pause") => {
            context.proxy_server.router().pause();
            json_response(StatusCode::OK, json!({"ok":true}))
        }
        (Method::POST, "/v1/proxy/resume") | (Method::POST, "/v1/proxy/start") => {
            context.proxy_server.router().resume();
            json_response(StatusCode::OK, json!({"ok":true}))
        }
        (Method::POST, "/v1/proxy/stop") => {
            context.proxy_server.stop_accepting().await;
            json_response(StatusCode::OK, json!({"ok":true}))
        }
        (Method::POST, "/v1/integration/enable") => {
            operation_result(CodexIntegration::new(&context.config.read().codex_home).enable())
        }
        (Method::POST, "/v1/integration/disable") => {
            operation_result(CodexIntegration::new(&context.config.read().codex_home).disable())
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
        .body(Full::new(Bytes::from(value.to_string())))
        .unwrap()
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
    let descriptor = read_runtime(paths)?;
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?
        .request(
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
            format!("http://{}{}", descriptor.address, endpoint),
        )
        .bearer_auth(descriptor.bearer_token)
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
        let _ = shutdown.send(true);
        task.await.unwrap().unwrap();
        assert!(!paths.runtime_file.exists());
    }
}
