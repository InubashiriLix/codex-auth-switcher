use super::{
    connection_tracker::{ConnectionTracker, RequestGuard, RequestMetadata},
    SharedProxyStats,
};
use crate::{
    account::{auth_tokens, snapshot_path},
    config::{Config, ProxyConfig},
    error::*,
    types::AccountIndex,
};
use http_body_util::{BodyExt, Empty, Full};
use hyper::{
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
    Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use parking_lot::RwLock;
use std::{
    fs,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::net::TcpListener;
use tracing::{error, info};
use uuid::Uuid;

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}

fn _empty() -> BoxBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

pub struct ProxyServer {
    config: Config,
    proxy_config: ProxyConfig,
    #[allow(dead_code)]
    accounts: Arc<RwLock<AccountIndex>>,
    current_account: Arc<RwLock<Option<Uuid>>>,
    connection_tracker: ConnectionTracker,
    stats: SharedProxyStats,
    accepting: Arc<AtomicBool>,
}

impl ProxyServer {
    pub fn new(
        config: Config,
        proxy_config: ProxyConfig,
        accounts: Arc<RwLock<AccountIndex>>,
        current_account: Arc<RwLock<Option<Uuid>>>,
        stats: SharedProxyStats,
    ) -> Self {
        Self {
            config,
            proxy_config,
            accounts,
            current_account,
            connection_tracker: ConnectionTracker::new(),
            stats,
            accepting: Arc::new(AtomicBool::new(true)),
        }
    }

    pub async fn serve(self: Arc<Self>) -> Result<()> {
        let addr: SocketAddr = self
            .proxy_config
            .listen_addr
            .parse()
            .map_err(|e| AppError::Message(format!("Invalid listen address: {}", e)))?;

        let listener = TcpListener::bind(addr).await?;
        info!("Proxy server listening on {}", addr);

        loop {
            if !self.accepting.load(Ordering::Relaxed) {
                info!("Proxy server stopped accepting new connections");
                break;
            }

            let (stream, remote_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                    continue;
                }
            };

            let io = TokioIo::new(stream);
            let server = self.clone();

            tokio::task::spawn(async move {
                let service = service_fn(move |req| {
                    let server = server.clone();
                    async move { server.handle_request(req).await }
                });

                if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                    error!("Error serving connection from {}: {:?}", remote_addr, err);
                }
            });
        }

        Ok(())
    }

    async fn handle_request(
        &self,
        req: Request<Incoming>,
    ) -> std::result::Result<Response<BoxBody>, hyper::Error> {
        let req_id = Uuid::new_v4().to_string();
        let method = req.method().to_string();
        let path = req.uri().path().to_string();

        // 跟踪请求
        self.connection_tracker.track_request(
            req_id.clone(),
            RequestMetadata {
                started_at: std::time::Instant::now(),
                method: method.clone(),
                path: path.clone(),
            },
        );

        let _guard = RequestGuard::new(req_id.clone(), self.connection_tracker.clone());

        // 增加请求计数
        self.stats.write().total_requests += 1;

        // 处理请求并转发
        match self.proxy_request(req).await {
            Ok(response) => Ok(response),
            Err(e) => {
                self.stats.write().failed_requests += 1;
                error!("Request {} failed: {}", req_id, e);
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(full(format!("Proxy error: {}", e)))
                    .unwrap())
            }
        }
    }

    async fn proxy_request(&self, req: Request<Incoming>) -> Result<Response<BoxBody>> {
        // 获取当前账户的token
        let current_id = self.current_account.read().clone();
        let access_token = if let Some(id) = current_id {
            let snapshot = snapshot_path(&self.config, id);
            if !snapshot.exists() {
                return Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(full("No active account snapshot found"))
                    .unwrap());
            }

            let raw = fs::read_to_string(&snapshot)?;
            let value: serde_json::Value = serde_json::from_str(&raw)?;
            let (_, access, _, _, _) = auth_tokens(&value)?;
            access
        } else {
            return Ok(Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(full("No active account configured"))
                .unwrap());
        };

        // 构建目标URL
        let target_base = &self.proxy_config.target_base;
        let path = req.uri().path();
        let query = req
            .uri()
            .query()
            .map(|q| format!("?{}", q))
            .unwrap_or_default();
        let target_url = format!("{}{}{}", target_base, path, query);

        // 构建转发请求
        let method = req.method().clone();
        let headers = req.headers().clone();

        // 收集body
        let body_bytes = req
            .into_body()
            .collect()
            .await
            .map_err(|e| AppError::Message(format!("Failed to read request body: {}", e)))?
            .to_bytes();

        // 使用reqwest发送请求
        let client = reqwest::Client::new();
        let mut builder = client
            .request(method.clone().try_into().unwrap(), &target_url)
            .bearer_auth(&access_token);

        // 复制必要的header
        for (name, value) in headers.iter() {
            if name != "host"
                && name != "authorization"
                && name != "content-length"
                && name != "transfer-encoding"
            {
                if let Ok(val_str) = value.to_str() {
                    builder = builder.header(name.as_str(), val_str);
                }
            }
        }

        if !body_bytes.is_empty() {
            builder = builder.body(body_bytes.to_vec());
        }

        let resp = builder.send().await?;

        // 构建响应
        let mut response_builder = Response::builder().status(resp.status().as_u16());

        // 复制响应头
        for (name, value) in resp.headers().iter() {
            if name != "content-length" && name != "transfer-encoding" {
                response_builder = response_builder.header(name, value);
            }
        }

        let body_bytes = resp.bytes().await?;
        Ok(response_builder.body(full(body_bytes)).unwrap())
    }

    pub async fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Relaxed);
    }

    pub fn connection_tracker(&self) -> &ConnectionTracker {
        &self.connection_tracker
    }
}
