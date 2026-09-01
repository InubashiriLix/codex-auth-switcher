use super::{
    CircuitReason, RefreshOutcome, Router, SharedProxyStats, TokenRefresher,
    connection_tracker::{ConnectionTracker, RequestGuard, RequestMetadata},
    fingerprint::DeviceFingerprint,
    upstream_error::{UpstreamFailure, UpstreamFailureKind, classify},
};
use crate::{
    account::{auth_tokens, snapshot_path},
    config::{Config, ProxyConfig},
    error::{AppError, Result},
    identity::{ProcessInspector, RequestIdentity, SystemProcessInspector},
    storage::{MetadataStore, RequestSummary},
    types::AccountIndex,
};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, stream};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::{
    Request, Response, StatusCode,
    body::{Bytes, Frame, Incoming},
    header::{
        CONNECTION, CONTENT_LENGTH, HOST, HeaderMap, HeaderName, HeaderValue, TRANSFER_ENCODING,
    },
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use parking_lot::{Mutex, RwLock};
use serde_json::json;
use std::{
    collections::HashSet,
    error::Error,
    fs,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use tokio::net::TcpListener;
use tracing::{error, info};
use uuid::Uuid;

const MAX_REPLAY_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: u64 = 1024 * 1024;
type BoxError = Box<dyn Error + Send + Sync>;
type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, BoxError>;

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody {
    Full::new(chunk.into())
        .map_err(|never| -> BoxError { match never {} })
        .boxed_unsync()
}

#[derive(Default)]
struct ReplayCapture {
    bytes: Vec<u8>,
    expected: Option<u64>,
    seen: u64,
    overflow: bool,
}

struct StreamCompletion {
    guard: Option<RequestGuard>,
    lease: Option<AccountLease>,
    store: Option<Arc<MetadataStore>>,
    summary: Option<RequestSummary>,
    response_bytes: u64,
    completed: bool,
    failed: bool,
    started: Instant,
}

struct AccountLease {
    router: Router,
    account_id: Option<Uuid>,
}

impl AccountLease {
    fn new(router: Router, account_id: Uuid) -> Self {
        router.acquire(account_id);
        Self {
            router,
            account_id: Some(account_id),
        }
    }
}

enum InspectedUpstream {
    Streaming {
        response: reqwest::Response,
        failure: Option<UpstreamFailure>,
    },
    Buffered {
        status: StatusCode,
        headers: HeaderMap,
        body: Bytes,
        failure: UpstreamFailure,
    },
}

impl InspectedUpstream {
    fn failure(&self) -> Option<&UpstreamFailure> {
        match self {
            Self::Streaming { failure, .. } => failure.as_ref(),
            Self::Buffered { failure, .. } => Some(failure),
        }
    }
}

impl Drop for AccountLease {
    fn drop(&mut self) {
        if let Some(account_id) = self.account_id.take() {
            self.router.release(account_id);
        }
    }
}

impl StreamCompletion {
    fn finish(mut self) {
        self.completed = true;
        drop(self);
    }
}

impl Drop for StreamCompletion {
    fn drop(&mut self) {
        self.guard.take();
        self.lease.take();
        if let (Some(store), Some(mut summary)) = (&self.store, self.summary.take()) {
            summary.duration_ms = Some(self.started.elapsed().as_millis() as u64);
            summary.response_bytes = self.response_bytes;
            summary.partial_failure = self.failed || !self.completed;
            summary.stage = if self.completed && !self.failed {
                "completed"
            } else {
                "partial_failure"
            }
            .into();
            let _ = store.record_request(&summary);
        }
    }
}

impl ReplayCapture {
    fn complete(bytes: &Bytes) -> Self {
        Self {
            bytes: bytes.to_vec(),
            expected: Some(bytes.len() as u64),
            seen: bytes.len() as u64,
            overflow: false,
        }
    }

    fn push(&mut self, bytes: &Bytes) {
        self.seen += bytes.len() as u64;
        if !self.overflow && self.bytes.len() + bytes.len() <= MAX_REPLAY_BYTES {
            self.bytes.extend_from_slice(bytes);
        } else {
            self.overflow = true;
            self.bytes.clear();
        }
    }

    fn replay(&self) -> Option<Bytes> {
        let complete = self.expected.is_some_and(|expected| expected == self.seen);
        (!self.overflow && complete).then(|| Bytes::copy_from_slice(&self.bytes))
    }
}

async fn collect_request_body(mut body: Incoming) -> Result<Bytes> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        if let Some(data) = frame.data_ref() {
            if bytes.len().saturating_add(data.len()) > MAX_REPLAY_BYTES {
                return Err(AppError::Message(format!(
                    "启用设备身份收敛时，请求体不能超过 {} MiB",
                    MAX_REPLAY_BYTES / (1024 * 1024)
                )));
            }
            bytes.extend_from_slice(data);
        }
    }
    Ok(Bytes::from(bytes))
}

pub struct ProxyServer {
    config: Config,
    proxy_config: ProxyConfig,
    accounts: Arc<RwLock<AccountIndex>>,
    current_account: Arc<RwLock<Option<Uuid>>>,
    router: Router,
    refresher: TokenRefresher,
    client: reqwest::Client,
    device_fingerprint: Option<DeviceFingerprint>,
    connection_tracker: ConnectionTracker,
    stats: SharedProxyStats,
    accepting: Arc<AtomicBool>,
    metadata_store: RwLock<Option<Arc<MetadataStore>>>,
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
            router: Router::new(proxy_config.clone()),
            refresher: TokenRefresher::new(config.clone()),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("static reqwest client configuration"),
            device_fingerprint: DeviceFingerprint::from_config(
                &proxy_config.device_identity,
                &config.codex_home,
            ),
            config,
            proxy_config,
            accounts,
            current_account,
            connection_tracker: ConnectionTracker::new(),
            stats,
            accepting: Arc::new(AtomicBool::new(true)),
            metadata_store: RwLock::new(None),
        }
    }

    pub async fn serve(self: Arc<Self>) -> Result<()> {
        let addr: SocketAddr = self
            .proxy_config
            .listen_addr
            .parse()
            .map_err(|e| AppError::Message(format!("代理监听地址无效：{e}")))?;
        if !addr.ip().is_loopback() {
            return Err(AppError::Message("首版代理拒绝非回环监听地址".into()));
        }
        let listener = TcpListener::bind(addr).await?;
        info!(%addr, "Proxy server listening");

        loop {
            let (stream, remote_addr) = tokio::select! {
                result = listener.accept() => result?,
                _ = async {
                    while self.accepting.load(Ordering::Relaxed) {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } => break,
            };
            let io = TokioIo::new(stream);
            let server = self.clone();
            let connection_id = format!("{}:{}", remote_addr, Uuid::new_v4());
            let mut identity = RequestIdentity::unknown_local(&connection_id);
            identity.process = SystemProcessInspector.inspect(remote_addr, listener.local_addr()?);
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let server = server.clone();
                    let identity = identity.clone();
                    async move { server.handle_request(request, &identity).await }
                });
                if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                    error!(%remote_addr, error = %err, "proxy connection failed");
                }
            });
        }
        Ok(())
    }

    pub fn start_accepting(&self) {
        self.accepting.store(true, Ordering::Relaxed);
    }

    fn record_event(&self, kind: &str, account_id: Option<Uuid>, detail: &str) {
        let Some(store) = self.metadata_store.read().clone() else {
            return;
        };
        let _ = store.record_account_event(account_id, kind, detail);
        let _ = store.record_event(&crate::storage::RuntimeEvent {
            id: Uuid::new_v4().to_string(),
            occurred_at: Utc::now(),
            tenant_id: "local".into(),
            device_id: std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "local-device".into()),
            client_instance_id: None,
            kind: kind.into(),
            account_id,
            detail: detail.into(),
            message: Some(crate::i18n::LocalizedMessage::new(format!(
                "event-{}",
                kind.replace('_', "-")
            ))),
        });
    }

    async fn handle_request(
        &self,
        request: Request<Incoming>,
        identity: &RequestIdentity,
    ) -> std::result::Result<Response<BoxBody>, hyper::Error> {
        let request_id = Uuid::new_v4().to_string();
        let method = request.method().to_string();
        let path = request.uri().path().to_string();
        self.connection_tracker.track_request(
            request_id.clone(),
            RequestMetadata {
                started_at: Instant::now(),
                method,
                path,
                identity: identity.clone(),
            },
        );
        let guard = RequestGuard::new(request_id.clone(), self.connection_tracker.clone());
        self.stats.write().total_requests += 1;

        match self
            .proxy_request(request, identity, &request_id, guard)
            .await
        {
            Ok(response) => Ok(response),
            Err(error) => {
                self.stats.write().failed_requests += 1;
                error!(request_id, error = %crate::storage::sanitize(&error.to_string()), "proxy request failed");
                Ok(responses_error(
                    StatusCode::BAD_GATEWAY,
                    "上游请求失败",
                    None,
                    None,
                ))
            }
        }
    }

    async fn proxy_request(
        &self,
        request: Request<Incoming>,
        identity: &RequestIdentity,
        request_id: &str,
        guard: RequestGuard,
    ) -> Result<Response<BoxBody>> {
        let started = Instant::now();
        let started_at = Utc::now();
        let sticky_key = identity.sticky_key();
        let (parts, incoming) = request.into_parts();
        let target_url = target_url(&self.proxy_config.target_base, &parts.uri)?;
        let mut request_headers = filtered_headers(&parts.headers);
        let expected = parts
            .headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let (capture, initial_body) = if let Some(fingerprint) = &self.device_fingerprint {
            fingerprint.normalize_headers(&mut request_headers)?;
            let raw = collect_request_body(incoming).await?;
            let normalized = fingerprint.normalize_body(&request_headers, raw)?;
            (
                Arc::new(Mutex::new(ReplayCapture::complete(&normalized))),
                reqwest::Body::from(normalized),
            )
        } else {
            let capture = Arc::new(Mutex::new(ReplayCapture {
                expected,
                ..Default::default()
            }));
            let capture_for_stream = capture.clone();
            let request_stream = incoming.into_data_stream().map(move |result| {
                if let Ok(bytes) = &result {
                    capture_for_stream.lock().push(bytes);
                }
                result
            });
            (capture, reqwest::Body::wrap_stream(request_stream))
        };

        let route = match self.router.route(&self.accounts.read(), &sticky_key) {
            Ok(route) => route,
            Err(unavailable) => {
                return Ok(responses_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &unavailable.reason,
                    unavailable.retry_after_seconds,
                    unavailable.earliest_recovery,
                ));
            }
        };
        let lease = AccountLease::new(self.router.clone(), route.account_id);
        *self.current_account.write() = Some(route.account_id);
        self.stats.write().current_account = Some(route.account_id);

        let (mut access_token, account_header) = self.credentials(route.account_id)?;
        if access_token.split('.').count() == 3 {
            match self.refresher.refresh(route.account_id, false).await {
                Ok(RefreshOutcome::Refreshed) => {
                    access_token = self.credentials(route.account_id)?.0
                }
                Ok(RefreshOutcome::ReauthRequired) => {
                    self.router
                        .open_circuit(route.account_id, CircuitReason::Reauth, None);
                    self.record_event(
                        "reauth_required",
                        Some(route.account_id),
                        "proactive OAuth refresh rejected",
                    );
                    return Ok(responses_error(
                        StatusCode::UNAUTHORIZED,
                        "账户需要重新登录",
                        None,
                        None,
                    ));
                }
                Ok(RefreshOutcome::StillValid) => {}
                Err(_) => {}
            }
        }

        let method = parts.method.clone();
        let mut retries = 0u64;
        let mut upstream = match self
            .send_upstream(
                &method,
                &target_url,
                &request_headers,
                &access_token,
                account_header.as_deref(),
                initial_body,
            )
            .await
        {
            Ok(response) => response,
            Err(first_error) => {
                let replay = capture.lock().replay();
                if let Some(replay) = replay {
                    retries += 1;
                    self.send_upstream(
                        &method,
                        &target_url,
                        &request_headers,
                        &access_token,
                        account_header.as_deref(),
                        reqwest::Body::from(replay),
                    )
                    .await?
                } else {
                    return Err(first_error);
                }
            }
        };

        if upstream.status().is_server_error() {
            let replay = capture.lock().replay();
            if let Some(replay) = replay {
                upstream = self
                    .send_upstream(
                        &method,
                        &target_url,
                        &request_headers,
                        &access_token,
                        account_header.as_deref(),
                        reqwest::Body::from(replay),
                    )
                    .await?;
                retries += 1;
            }
        }

        let mut inspected = inspect_upstream(upstream).await?;
        let refreshable_401 = self.proxy_config.auth_policy.refresh_once_on_401
            && inspected
                .failure()
                .is_some_and(|failure| failure.kind == UpstreamFailureKind::TokenExpired);
        if refreshable_401 {
            let replay = capture.lock().replay();
            if let Some(replay) = replay {
                match self
                    .refresher
                    .refresh_rejected(route.account_id, &access_token)
                    .await
                {
                    Ok(RefreshOutcome::Refreshed | RefreshOutcome::StillValid) => {
                        access_token = self.credentials(route.account_id)?.0;
                        let response = self
                            .send_upstream(
                                &method,
                                &target_url,
                                &request_headers,
                                &access_token,
                                account_header.as_deref(),
                                reqwest::Body::from(replay),
                            )
                            .await?;
                        inspected = inspect_upstream(response).await?;
                        retries += 1;
                    }
                    Ok(RefreshOutcome::ReauthRequired) | Err(_) => {
                        self.router
                            .open_circuit(route.account_id, CircuitReason::Reauth, None);
                    }
                }
            }
        }

        let failure = inspected.failure().cloned();
        let status = match &inspected {
            InspectedUpstream::Streaming { response, .. } => response.status(),
            InspectedUpstream::Buffered { status, .. } => *status,
        };
        if let Some(failure) = &failure {
            let (reason, until) = self.circuit_for_failure(route.account_id, failure);
            self.router.open_circuit(route.account_id, reason, until);
            let event_kind = if status == StatusCode::TOO_MANY_REQUESTS {
                "upstream_rate_limit"
            } else {
                "upstream_auth_failure"
            };
            self.record_event(event_kind, Some(route.account_id), &failure.event_detail());
        } else if status.is_success() {
            self.router.close_circuit(route.account_id);
        }
        {
            let mut stats = self.stats.write();
            stats.upstream_responses += 1;
            stats.retries += retries;
            stats.last_ttfb_ms = Some(started.elapsed().as_millis() as u64);
            match status.as_u16() {
                401 => stats.http_401 += 1,
                403 => stats.http_403 += 1,
                429 => stats.http_429 += 1,
                500..=599 => stats.http_5xx += 1,
                _ => {}
            }
        }

        let captured_request_bytes = capture.lock().seen;
        let summary = RequestSummary {
            id: request_id.into(),
            tenant_id: identity.tenant_id.0.clone(),
            device_id: identity.device_id.0.clone(),
            client_instance_id: sticky_key.clone(),
            session_key: identity.session_key.as_ref().map(|key| key.0.clone()),
            started_at,
            method: method.to_string(),
            path: parts.uri.path().into(),
            status: Some(status.as_u16()),
            stage: "streaming".into(),
            duration_ms: None,
            ttfb_ms: Some(started.elapsed().as_millis() as u64),
            request_bytes: captured_request_bytes,
            response_bytes: 0,
            account_id: Some(route.account_id),
            route_reason: route.reason.clone(),
            route_message: localized_route_message(&route.reason),
            retries: retries as u32,
            partial_failure: false,
        };

        match inspected {
            InspectedUpstream::Buffered {
                status,
                headers,
                body,
                ..
            } => {
                let mut builder = Response::builder().status(status.as_u16());
                for (name, value) in &filtered_headers(&headers) {
                    builder = builder.header(name, value);
                }
                self.stats.write().response_bytes += body.len() as u64;
                if let Some(store) = self.metadata_store.read().clone() {
                    let mut completed = summary;
                    completed.stage = "completed".into();
                    completed.duration_ms = Some(started.elapsed().as_millis() as u64);
                    completed.response_bytes = body.len() as u64;
                    let _ = store.record_request(&completed);
                }
                drop(guard);
                drop(lease);
                builder
                    .body(full(body))
                    .map_err(|error| AppError::Message(format!("构建代理响应失败：{error}")))
            }
            InspectedUpstream::Streaming { response, .. } => {
                let mut builder = Response::builder().status(status.as_u16());
                for (name, value) in &filtered_headers(response.headers()) {
                    builder = builder.header(name, value);
                }
                let completion = StreamCompletion {
                    guard: Some(guard),
                    lease: Some(lease),
                    store: self.metadata_store.read().clone(),
                    summary: Some(summary),
                    response_bytes: 0,
                    completed: false,
                    failed: false,
                    started,
                };
                let response_stream = response.bytes_stream();
                let stats = self.stats.clone();
                let guarded = stream::unfold(
                    (response_stream, completion, stats),
                    |(mut response_stream, mut completion, stats)| async move {
                        match response_stream.next().await {
                            Some(Ok(bytes)) => {
                                stats.write().response_bytes += bytes.len() as u64;
                                completion.response_bytes += bytes.len() as u64;
                                Some((
                                    Ok::<Frame<Bytes>, BoxError>(Frame::data(bytes)),
                                    (response_stream, completion, stats),
                                ))
                            }
                            Some(Err(error)) => {
                                stats.write().partial_failures += 1;
                                completion.failed = true;
                                Some((
                                    Err::<Frame<Bytes>, BoxError>(Box::new(error)),
                                    (response_stream, completion, stats),
                                ))
                            }
                            None => {
                                completion.finish();
                                None
                            }
                        }
                    },
                );
                let body = StreamBody::new(guarded).boxed_unsync();
                builder
                    .body(body)
                    .map_err(|error| AppError::Message(format!("构建代理响应失败：{error}")))
            }
        }
    }

    async fn send_upstream(
        &self,
        method: &hyper::Method,
        url: &str,
        headers: &HeaderMap,
        access_token: &str,
        account_id: Option<&str>,
        body: reqwest::Body,
    ) -> Result<reqwest::Response> {
        let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .map_err(|_| AppError::Message("不支持的 HTTP 方法".into()))?;
        let mut request = self
            .client
            .request(method, url)
            .headers(headers.clone())
            .bearer_auth(access_token)
            .body(body);
        if let Some(account_id) = account_id {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        Ok(request.send().await?)
    }

    fn credentials(&self, id: Uuid) -> Result<(String, Option<String>)> {
        let raw = fs::read_to_string(snapshot_path(&self.config, id))?;
        let value = serde_json::from_str(&raw)?;
        let (_, access, discovered_account, _, _) = auth_tokens(&value)?;
        let account = discovered_account.or_else(|| {
            self.accounts
                .read()
                .accounts
                .iter()
                .find(|account| account.id == id)
                .and_then(|account| account.account_id.clone())
        });
        Ok((access, account))
    }

    fn account_reset(&self, id: Uuid) -> Option<DateTime<Utc>> {
        self.accounts
            .read()
            .accounts
            .iter()
            .find(|account| account.id == id)
            .and_then(|account| {
                [
                    account.status.primary.as_ref(),
                    account.status.secondary.as_ref(),
                ]
                .into_iter()
                .flatten()
                .filter_map(|quota| quota.resets_at)
                .min()
            })
            .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
    }

    fn circuit_for_failure(
        &self,
        account_id: Uuid,
        failure: &UpstreamFailure,
    ) -> (CircuitReason, Option<DateTime<Utc>>) {
        match failure.kind {
            UpstreamFailureKind::TokenExpired => (CircuitReason::Reauth, None),
            UpstreamFailureKind::InvalidCredential => (CircuitReason::InvalidCredential, None),
            UpstreamFailureKind::OrganizationMismatch => {
                (CircuitReason::OrganizationMismatch, None)
            }
            UpstreamFailureKind::MembershipRemoved => (CircuitReason::MembershipRemoved, None),
            UpstreamFailureKind::IpAllowlist => (CircuitReason::IpAllowlist, None),
            UpstreamFailureKind::Forbidden => (CircuitReason::Forbidden, None),
            UpstreamFailureKind::QuotaBlocked => (CircuitReason::QuotaBlocked, None),
            UpstreamFailureKind::UnknownUnauthorized => (CircuitReason::Unauthorized, None),
            UpstreamFailureKind::TemporaryRateLimit | UpstreamFailureKind::UnknownRateLimit => {
                let until = failure
                    .retry_after_seconds
                    .map(|seconds| {
                        Utc::now() + chrono::Duration::seconds(seconds.min(i64::MAX as u64) as i64)
                    })
                    .or_else(|| self.account_reset(account_id))
                    .or_else(|| {
                        let seconds = self.proxy_config.auth_policy.rate_limit_fallback_seconds;
                        (seconds > 0).then(|| {
                            Utc::now()
                                + chrono::Duration::seconds(seconds.min(i64::MAX as u64) as i64)
                        })
                    });
                (CircuitReason::RateLimited, until)
            }
        }
    }

    pub async fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Relaxed);
    }
    pub fn connection_tracker(&self) -> &ConnectionTracker {
        &self.connection_tracker
    }
    pub fn router(&self) -> &Router {
        &self.router
    }

    pub fn attach_metadata_store(&self, store: Arc<MetadataStore>) {
        self.router.attach_store(store.clone());
        *self.metadata_store.write() = Some(store);
    }
}

async fn inspect_upstream(response: reqwest::Response) -> Result<InspectedUpstream> {
    let status = response.status();
    if !matches!(status.as_u16(), 401 | 403 | 429) {
        return Ok(InspectedUpstream::Streaming {
            response,
            failure: None,
        });
    }

    let headers = response.headers().clone();
    let declared_length = response.content_length();
    if declared_length.is_some_and(|length| length <= MAX_ERROR_BODY_BYTES) {
        let body = response.bytes().await?;
        if body.len() as u64 > MAX_ERROR_BODY_BYTES {
            return Err(AppError::Message("上游错误响应超过 1 MiB，拒绝缓冲".into()));
        }
        let failure = classify(status, &headers, &body);
        return Ok(InspectedUpstream::Buffered {
            status,
            headers,
            body,
            failure,
        });
    }

    let failure = classify(status, &headers, &[]);
    Ok(InspectedUpstream::Streaming {
        response,
        failure: Some(failure),
    })
}

fn localized_route_message(reason: &str) -> Option<crate::i18n::LocalizedMessage> {
    let key = match reason {
        "会话/进程粘性" => "route-sticky",
        "用户手动选择" => "route-manual",
        _ if reason.ends_with(" 策略") => "route-strategy",
        _ => return None,
    };
    let mut message = crate::i18n::LocalizedMessage::new(key);
    if key == "route-strategy" {
        message
            .args
            .insert("strategy".into(), reason.trim_end_matches(" 策略").into());
    }
    Some(message)
}

fn target_url(target_base: &str, uri: &hyper::Uri) -> Result<String> {
    let base = target_base.trim_end_matches('/');
    let path = if uri.path().starts_with("/backend-api/codex") {
        uri.path().to_owned()
    } else {
        format!("/backend-api/codex{}", uri.path())
    };
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let official = base == "https://chatgpt.com";
    let test_loopback = base.starts_with("http://127.0.0.1:")
        || base.starts_with("http://localhost:")
        || base.starts_with("http://[::1]:");
    if !official && !test_loopback {
        return Err(AppError::Message(
            "生产上游固定为 https://chatgpt.com".into(),
        ));
    }
    Ok(format!("{base}{path}{query}"))
}

fn filtered_headers(headers: &HeaderMap) -> HeaderMap {
    let mut forbidden: HashSet<HeaderName> = [
        HOST,
        CONTENT_LENGTH,
        TRANSFER_ENCODING,
        CONNECTION,
        HeaderName::from_static("authorization"),
        HeaderName::from_static("proxy-authorization"),
        HeaderName::from_static("cookie"),
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("chatgpt-account-id"),
        HeaderName::from_static("openai-organization"),
        HeaderName::from_static("openai-project"),
        HeaderName::from_static("x-codex-switcher-tenant"),
        HeaderName::from_static("x-codex-switcher-device"),
        HeaderName::from_static("x-codex-switcher-process"),
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("te"),
        HeaderName::from_static("trailer"),
        HeaderName::from_static("upgrade"),
    ]
    .into_iter()
    .collect();
    if let Some(connection) = headers
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
    {
        forbidden.extend(
            connection
                .split(',')
                .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok()),
        );
    }
    headers
        .iter()
        .filter(|(name, _)| !forbidden.contains(*name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn responses_error(
    status: StatusCode,
    reason: &str,
    retry_after: Option<u64>,
    recovery: Option<DateTime<Utc>>,
) -> Response<BoxBody> {
    let message = match recovery {
        Some(time) => format!(
            "Codex Switcher：{reason}；最早恢复时间 {}",
            time.to_rfc3339()
        ),
        None if status == StatusCode::SERVICE_UNAVAILABLE => {
            format!("Codex Switcher：{reason}；最早恢复时间未知")
        }
        None => format!("Codex Switcher：{reason}"),
    };
    let payload = json!({"error": {"message": message, "type": "codex_switcher_error", "code": "accounts_unavailable"}});
    let mut builder = Response::builder()
        .status(status)
        .header("content-type", "application/json");
    if let Some(seconds) = retry_after {
        builder = builder.header("retry-after", HeaderValue::from(seconds));
    }
    builder
        .body(full(payload.to_string()))
        .expect("static error response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        proxy::ProxyState,
        types::{Account, CheckStatus, Quota, StatusKind},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn target_maps_provider_path_exactly_once() {
        let uri: hyper::Uri = "/backend-api/codex/responses?x=1".parse().unwrap();
        assert_eq!(
            target_url("https://chatgpt.com", &uri).unwrap(),
            "https://chatgpt.com/backend-api/codex/responses?x=1"
        );
        let uri: hyper::Uri = "/responses".parse().unwrap();
        assert_eq!(
            target_url("https://chatgpt.com", &uri).unwrap(),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn hop_and_auth_headers_are_removed() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("connection", HeaderValue::from_static("x-private"));
        headers.insert("x-private", HeaderValue::from_static("no"));
        headers.insert("x-safe", HeaderValue::from_static("yes"));
        let filtered = filtered_headers(&headers);
        assert!(filtered.get("authorization").is_none());
        assert!(filtered.get("x-private").is_none());
        assert_eq!(filtered.get("x-safe").unwrap(), "yes");
    }

    #[tokio::test]
    async fn unavailable_error_is_responses_shaped_and_bounded() {
        let response = responses_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "代理池为空",
            Some(30),
            None,
        );
        assert_eq!(response.headers().get("retry-after").unwrap(), "30");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .starts_with("Codex Switcher：")
        );
        assert!(
            value
                .pointer("/error/message")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("最早恢复时间")
        );
    }

    #[tokio::test]
    async fn sse_first_chunk_is_forwarded_before_upstream_finishes() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = upstream.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").await.unwrap();
            let first = b"data: first\n\n";
            socket
                .write_all(format!("{:X}\r\n", first.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(first).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            let second = b"data: second\n\n";
            socket
                .write_all(format!("{:X}\r\n", second.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(second).await.unwrap();
            socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        });

        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = reserve.local_addr().unwrap();
        drop(reserve);
        let root = std::env::temp_dir().join(format!("codex-switcher-stream-{}", Uuid::new_v4()));
        let accounts_dir = root.join("accounts");
        fs::create_dir_all(&accounts_dir).unwrap();
        let account_id = Uuid::new_v4();
        fs::write(accounts_dir.join(format!("{account_id}.auth.json")),
            r#"{"tokens":{"access_token":"test-token","refresh_token":"","id_token":"","account_id":"workspace"}}"#).unwrap();
        let account = Account {
            id: account_id,
            label: "stream".into(),
            source: "test".into(),
            imported_at: Utc::now(),
            email: None,
            plan: None,
            account_id: Some("workspace".into()),
            status: CheckStatus {
                kind: StatusKind::Live,
                checked_at: Some(Utc::now()),
                detail: "ok".into(),
                primary: Some(Quota {
                    used_percent: 1.0,
                    window_minutes: Some(300),
                    resets_at: None,
                }),
                secondary: None,
            },
            tenant_id: "local".into(),
            proxy_enabled: true,
            enabled: true,
            priority: 100,
            concurrency_limit: 0,
            revision: 1,
        };
        let mut config = Config::defaults();
        config.accounts_dir = accounts_dir;
        config.proxy.listen_addr = proxy_addr.to_string();
        config.proxy.target_base = format!("http://127.0.0.1:{}", upstream_addr.port());
        let state = ProxyState::new();
        let server = Arc::new(ProxyServer::new(
            config.clone(),
            config.proxy.clone(),
            Arc::new(RwLock::new(AccountIndex {
                accounts: vec![account],
            })),
            Arc::new(RwLock::new(None)),
            state.stats,
        ));
        let task = tokio::spawn(server.clone().serve());
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let response = reqwest::get(format!("http://{proxy_addr}/backend-api/codex/responses"))
            .await
            .unwrap();
        let mut chunks = response.bytes_stream();
        let first = tokio::time::timeout(std::time::Duration::from_millis(200), chunks.next())
            .await
            .expect("first SSE chunk must not wait for the full response")
            .unwrap()
            .unwrap();
        assert_eq!(&first[..], b"data: first\n\n");
        let second = chunks.next().await.unwrap().unwrap();
        assert_eq!(&second[..], b"data: second\n\n");
        server.stop_accepting().await;
        task.await.unwrap().unwrap();
    }
}
