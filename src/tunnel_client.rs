use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chatcmd_core::LocalDevice;
use chatcmd_storage::SqliteRepository;
use futures_util::{SinkExt, StreamExt};
use reqwest::{
    Method, Url,
    header::{HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, RwLock, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tracing::{info, warn};

const TUNNEL_SETTING_KEY: &str = "chatcmd_tunnel_connection";
const DEFAULT_TUNNEL_SERVER_URL: &str = "https://tunnel.chatcmd.net";
const RESPONSE_CHUNK_BYTES: usize = 32 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUEST_BODY_BASE64_BYTES: usize = (MAX_REQUEST_BODY_BYTES / 3 + 1) * 4;
const MAX_REQUEST_ID_BYTES: usize = 200;
const MAX_HEADER_COUNT: usize = 256;
const MAX_HEADER_VALUE_BYTES: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TunnelConnectionStatus {
    pub state: String,
    pub connected: bool,
    pub server_url: String,
    pub key: Option<String>,
    pub public_url: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTunnelConnection {
    server_url: String,
    key: String,
    secret: String,
    public_url: String,
    websocket_url: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelRegistrationResponse {
    key: String,
    connection_secret: Option<String>,
    public_url: String,
    web_socket_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelServerFrame {
    #[serde(rename = "type")]
    message_type: String,
    request_id: Option<String>,
    method: Option<String>,
    path_and_query: Option<String>,
    headers: Option<HashMap<String, Vec<String>>>,
    body_base64: Option<String>,
}

struct TunnelControl {
    id: u64,
    stop: watch::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}

pub(crate) struct TunnelClientManager {
    repository: SqliteRepository,
    device: LocalDevice,
    local_port: u16,
    configured_server_url: String,
    http: reqwest::Client,
    status: RwLock<TunnelConnectionStatus>,
    control: Mutex<Option<TunnelControl>>,
    next_control_id: AtomicU64,
}

impl TunnelClientManager {
    pub(crate) fn new(
        repository: SqliteRepository,
        device: LocalDevice,
        local_port: u16,
    ) -> Arc<Self> {
        let configured_server_url = std::env::var("CHATCMD_TUNNEL_SERVER_URL")
            .unwrap_or_else(|_| DEFAULT_TUNNEL_SERVER_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        Arc::new(Self {
            repository,
            device,
            local_port,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("create tunnel HTTP client"),
            status: RwLock::new(TunnelConnectionStatus {
                state: "disconnected".to_owned(),
                connected: false,
                server_url: configured_server_url.clone(),
                key: None,
                public_url: None,
                last_error: None,
            }),
            configured_server_url,
            control: Mutex::new(None),
            next_control_id: AtomicU64::new(1),
        })
    }

    pub(crate) async fn status(&self) -> TunnelConnectionStatus {
        self.status.read().await.clone()
    }

    pub(crate) async fn start_if_enabled(self: &Arc<Self>) {
        match self.load_persisted().await {
            Ok(Some(connection)) if connection.enabled => {
                if let Err(error) = self.connect().await {
                    warn!(error = %error, "could not restore ChatCMD tunnel connection");
                }
            }
            Ok(_) => {}
            Err(error) => warn!(error = %error, "could not read persisted ChatCMD tunnel state"),
        }
    }

    pub(crate) async fn connect(self: &Arc<Self>) -> Result<TunnelConnectionStatus> {
        let control_id = self.next_control_id.fetch_add(1, Ordering::Relaxed);
        let ready_rx = {
            let mut control = self.control.lock().await;
            if control.is_some() {
                drop(control);
                return Ok(self.status().await);
            }

            let (stop_tx, stop_rx) = watch::channel(false);
            let (ready_tx, ready_rx) = oneshot::channel();
            let manager = Arc::clone(self);
            let handle = tokio::spawn(async move {
                manager.connection_worker(stop_rx, ready_tx).await;
            });
            *control = Some(TunnelControl {
                id: control_id,
                stop: stop_tx,
                handle: Some(handle),
            });
            ready_rx
        };

        match ready_rx.await {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(message)) => {
                self.clear_control(control_id).await;
                Err(anyhow!(message))
            }
            Err(_) => {
                self.clear_control(control_id).await;
                Err(anyhow!("tunnel connection worker stopped unexpectedly"))
            }
        }
    }

    async fn connection_worker(
        self: Arc<Self>,
        mut stop: watch::Receiver<bool>,
        ready: oneshot::Sender<Result<TunnelConnectionStatus, String>>,
    ) {
        self.set_status("connecting", false, None).await;
        let mut ready = Some(ready);
        let mut retry_delay = Duration::from_secs(1);
        loop {
            match self.establish_connection(&mut stop).await {
                Ok((connection, socket)) => {
                    self.set_connected_status(&connection, "connected", None)
                        .await;
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(self.status().await));
                    }
                    self.run_loop(connection, stop, Some(socket)).await;
                    return;
                }
                Err(_) if *stop.borrow() => {
                    self.set_status("disconnected", false, None).await;
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Err("tunnel connection cancelled".to_owned()));
                    }
                    return;
                }
                Err(error) => {
                    warn!(error = %error, "ChatCMD tunnel connection failed; retrying");
                    self.set_status("reconnecting", false, Some(error.to_string()))
                        .await;
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(self.status().await));
                    }
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(retry_delay) => {}
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        self.set_status("disconnected", false, None).await;
                        return;
                    }
                }
            }
            retry_delay = (retry_delay * 2).min(Duration::from_secs(15));
        }
    }

    async fn establish_connection(
        &self,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<(
        PersistedTunnelConnection,
        WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    )> {
        let persisted = self.load_persisted().await?;
        let server_url = persisted
            .as_ref()
            .map_or(self.configured_server_url.as_str(), |value| {
                value.server_url.as_str()
            })
            .trim_end_matches('/')
            .to_owned();
        let registration = tokio::select! {
            biased;
            changed = stop.changed() => {
                let _ = changed;
                bail!("tunnel connection cancelled");
            }
            result = self.register_device(&server_url, persisted.as_ref()) => result?,
        };
        let secret = registration
            .connection_secret
            .clone()
            .or_else(|| persisted.as_ref().map(|value| value.secret.clone()))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("tunnel server did not return a connection secret"))?;
        let connection = PersistedTunnelConnection {
            server_url,
            key: registration.key,
            secret,
            public_url: registration.public_url,
            websocket_url: registration.web_socket_url,
            enabled: true,
        };

        // Persist the assigned key and secret before opening the socket. If the
        // WebSocket endpoint is temporarily unavailable, restart recovery must
        // resume this tunnel instead of registering a new one.
        self.save_persisted(&connection).await?;
        let socket = tokio::select! {
            biased;
            changed = stop.changed() => {
                let _ = changed;
                bail!("tunnel connection cancelled");
            }
            result = self.open_socket(&connection) => {
                result.context("could not open ChatCMD tunnel WebSocket")?
            }
        };
        self.save_public_tunnel(&connection.public_url).await?;
        if *stop.borrow() {
            bail!("tunnel connection cancelled");
        }
        Ok((connection, socket))
    }

    pub(crate) async fn disconnect(self: &Arc<Self>) -> Result<TunnelConnectionStatus> {
        let (control_id, handle) = {
            let mut slot = self.control.lock().await;
            if let Some(control) = slot.as_mut() {
                let _ = control.stop.send(true);
                (Some(control.id), control.handle.take())
            } else {
                (None, None)
            }
        };
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        if let Some(mut persisted) = self.load_persisted().await? {
            persisted.enabled = false;
            self.save_persisted(&persisted).await?;
            self.set_connected_status(&persisted, "disconnected", None)
                .await;
        } else {
            self.set_status("disconnected", false, None).await;
        }
        if let Some(control_id) = control_id {
            self.remove_control(control_id).await;
        }
        Ok(self.status().await)
    }

    async fn clear_control(&self, control_id: u64) {
        let control = {
            let mut slot = self.control.lock().await;
            if slot.as_ref().is_some_and(|value| value.id == control_id) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(control) = control {
            let _ = control.stop.send(true);
            if let Some(handle) = control.handle {
                let _ = handle.await;
            }
        }
    }

    async fn remove_control(&self, control_id: u64) {
        let mut slot = self.control.lock().await;
        if slot.as_ref().is_some_and(|value| value.id == control_id) {
            slot.take();
        }
    }

    async fn run_loop(
        self: Arc<Self>,
        connection: PersistedTunnelConnection,
        mut stop: watch::Receiver<bool>,
        initial_socket: Option<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>,
    ) {
        let mut socket = initial_socket;
        let mut retry_delay = Duration::from_secs(1);
        loop {
            if *stop.borrow() {
                break;
            }
            let next_socket = if let Some(existing) = socket.take() {
                Ok(existing)
            } else {
                self.open_socket(&connection).await
            };

            match next_socket {
                Ok(stream) => {
                    retry_delay = Duration::from_secs(1);
                    self.set_connected_status(&connection, "connected", None)
                        .await;
                    if let Err(error) = self.serve_socket(stream, &mut stop).await {
                        if *stop.borrow() {
                            break;
                        }
                        warn!(error = %error, key = %connection.key, "ChatCMD tunnel socket interrupted");
                        self.set_connected_status(
                            &connection,
                            "reconnecting",
                            Some(error.to_string()),
                        )
                        .await;
                    }
                }
                Err(error) => {
                    if *stop.borrow() {
                        break;
                    }
                    self.set_connected_status(&connection, "reconnecting", Some(error.to_string()))
                        .await;
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(retry_delay) => {},
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() { break; }
                }
            }
            retry_delay = (retry_delay * 2).min(Duration::from_secs(15));
        }
        self.set_connected_status(&connection, "disconnected", None)
            .await;
    }

    async fn serve_socket(
        self: &Arc<Self>,
        stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<()> {
        let (mut writer, mut reader) = stream.split();
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<Message>(128);
        let writer_task = tokio::spawn(async move {
            while let Some(message) = outgoing_rx.recv().await {
                if writer.send(message).await.is_err() {
                    break;
                }
            }
        });
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut proxy_tasks = JoinSet::new();

        let result = loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        let _ = outgoing_tx.try_send(Message::Close(None));
                        break Ok(());
                    }
                }
                _ = heartbeat.tick() => {
                    let _ = outgoing_tx.try_send(Message::Text(json!({"type":"heartbeat"}).to_string().into()));
                }
                Some(_) = proxy_tasks.join_next(), if !proxy_tasks.is_empty() => {}
                incoming = reader.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<TunnelServerFrame>(&text) {
                                Ok(frame) if frame.message_type == "request" => {
                                    let manager = Arc::clone(self);
                                    let outgoing = outgoing_tx.clone();
                                    proxy_tasks.spawn(async move {
                                        manager.proxy_request(frame, outgoing).await;
                                    });
                                }
                                Ok(frame) if frame.message_type == "heartbeatAck" => {}
                                Ok(_) => warn!("ignored unsupported tunnel server frame"),
                                Err(error) => warn!(error = %error, "ignored malformed tunnel server frame"),
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => { let _ = outgoing_tx.try_send(Message::Pong(payload)); }
                        Some(Ok(Message::Close(_))) | None => break Err(anyhow!("tunnel WebSocket closed")),
                        Some(Ok(_)) => {}
                        Some(Err(error)) => break Err(anyhow!(error)),
                    }
                }
            }
        };

        proxy_tasks.abort_all();
        while proxy_tasks.join_next().await.is_some() {}
        drop(outgoing_tx);
        let _ = writer_task.await;
        result
    }

    async fn proxy_request(
        self: Arc<Self>,
        frame: TunnelServerFrame,
        outgoing: mpsc::Sender<Message>,
    ) {
        let Some(request_id) = frame.request_id.clone() else {
            return;
        };
        if let Err(error) = self.proxy_request_inner(&frame, &outgoing).await {
            let message = json!({
                "type": "responseError",
                "requestId": request_id,
                "error": error.to_string()
            });
            let _ = outgoing
                .send(Message::Text(message.to_string().into()))
                .await;
        }
    }

    async fn proxy_request_inner(
        &self,
        frame: &TunnelServerFrame,
        outgoing: &mpsc::Sender<Message>,
    ) -> Result<()> {
        let request_id = frame.request_id.as_deref().context("missing requestId")?;
        if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES {
            bail!("invalid requestId");
        }
        let method = frame.method.as_deref().context("missing method")?;
        let path_and_query = frame
            .path_and_query
            .as_deref()
            .context("missing pathAndQuery")?;
        if !is_valid_path_and_query(path_and_query) {
            bail!("invalid proxied path");
        }
        let local_url = format!("http://127.0.0.1:{}{path_and_query}", self.local_port);
        let mut request = self.http.request(
            Method::from_bytes(method.as_bytes()).context("invalid HTTP method")?,
            &local_url,
        );

        if let Some(headers) = &frame.headers {
            let header_value_count = headers.values().map(Vec::len).sum::<usize>();
            if headers.len() > MAX_HEADER_COUNT || header_value_count > MAX_HEADER_COUNT {
                bail!("too many proxied request headers");
            }
            for (name, values) in headers {
                if is_hop_by_hop_header(name) {
                    continue;
                }
                let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) else {
                    continue;
                };
                for value in values {
                    if value.len() > MAX_HEADER_VALUE_BYTES {
                        bail!("proxied request header is too large");
                    }
                    if let Ok(header_value) = HeaderValue::from_str(value) {
                        request = request.header(header_name.clone(), header_value);
                    }
                }
            }
        }
        if let Some(body) = &frame.body_base64 {
            if body.len() > MAX_REQUEST_BODY_BASE64_BYTES {
                bail!("proxied request body exceeds 16 MiB limit");
            }
            let body = STANDARD
                .decode(body)
                .context("invalid request body base64")?;
            if body.len() > MAX_REQUEST_BODY_BYTES {
                bail!("proxied request body exceeds 16 MiB limit");
            }
            request = request.body(body);
        }

        let response = request
            .send()
            .await
            .context("send request to local ChatCMD server")?;
        let status = response.status().as_u16();
        let headers = response_headers(response.headers());
        send_json(
            outgoing,
            json!({
                "type": "responseStart",
                "requestId": request_id,
                "statusCode": status,
                "headers": headers
            }),
        )
        .await?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("read local response stream")?;
            for part in chunk.chunks(RESPONSE_CHUNK_BYTES) {
                send_json(
                    outgoing,
                    json!({
                        "type": "responseChunk",
                        "requestId": request_id,
                        "dataBase64": STANDARD.encode(part)
                    }),
                )
                .await?;
            }
        }
        send_json(
            outgoing,
            json!({
                "type": "responseEnd",
                "requestId": request_id
            }),
        )
        .await?;
        Ok(())
    }

    async fn register_device(
        &self,
        server_url: &str,
        persisted: Option<&PersistedTunnelConnection>,
    ) -> Result<TunnelRegistrationResponse> {
        let device_key = self
            .device
            .machine_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.device.installation_id);
        let response = self
            .http
            .post(format!("{server_url}/api/tunnel/register"))
            .json(&json!({
                "deviceKey": device_key,
                "deviceName": self.device.name,
                "connectionSecret": persisted.map(|value| value.secret.as_str())
            }))
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .context("contact ChatCMD tunnel server")?;
        let status = response.status();
        let payload: Value = response
            .json()
            .await
            .context("decode tunnel registration response")?;
        if !status.is_success() {
            let detail = payload
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("tunnel registration failed");
            bail!("{detail} (HTTP {})", status.as_u16());
        }
        serde_json::from_value(payload).context("parse tunnel registration")
    }

    async fn open_socket(
        &self,
        connection: &PersistedTunnelConnection,
    ) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>> {
        let mut url =
            Url::parse(&connection.websocket_url).context("invalid tunnel WebSocket URL")?;
        url.query_pairs_mut()
            .append_pair("secret", &connection.secret);
        let (socket, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(url.as_str()))
            .await
            .context("tunnel WebSocket connection timed out")?
            .context("connect tunnel WebSocket")?;
        info!(key = %connection.key, public_url = %connection.public_url, "ChatCMD tunnel connected");
        Ok(socket)
    }

    async fn load_persisted(&self) -> Result<Option<PersistedTunnelConnection>> {
        let value = sqlx::query_scalar::<_, String>("SELECT value_json FROM settings WHERE key=?")
            .bind(TUNNEL_SETTING_KEY)
            .fetch_optional(self.repository.pool())
            .await
            .context("read persisted tunnel connection")?;
        value
            .map(|json| serde_json::from_str(&json).context("decode persisted tunnel connection"))
            .transpose()
    }

    async fn save_persisted(&self, value: &PersistedTunnelConnection) -> Result<()> {
        let now = crate::api::now_ms();
        let json = serde_json::to_string(value).context("encode persisted tunnel connection")?;
        sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at_ms=excluded.updated_at_ms")
            .bind(TUNNEL_SETTING_KEY)
            .bind(json)
            .bind(now)
            .execute(self.repository.pool())
            .await
            .context("persist tunnel connection")?;
        Ok(())
    }

    async fn save_public_tunnel(&self, public_url: &str) -> Result<()> {
        let now = crate::api::now_ms();
        sqlx::query("INSERT INTO tunnels(base_url,created_at_ms,updated_at_ms) VALUES(?,?,?) ON CONFLICT(base_url) DO UPDATE SET updated_at_ms=excluded.updated_at_ms")
            .bind(public_url.trim_end_matches('/'))
            .bind(now)
            .bind(now)
            .execute(self.repository.pool())
            .await
            .context("save assigned ChatCMD tunnel URL")?;
        Ok(())
    }

    async fn set_status(&self, state: &str, connected: bool, error: Option<String>) {
        let mut status = self.status.write().await;
        status.state = state.to_owned();
        status.connected = connected;
        status.last_error = error;
    }

    async fn set_connected_status(
        &self,
        connection: &PersistedTunnelConnection,
        state: &str,
        error: Option<String>,
    ) {
        let mut status = self.status.write().await;
        status.state = state.to_owned();
        status.connected = state == "connected";
        status.server_url = connection.server_url.clone();
        status.key = Some(connection.key.clone());
        status.public_url = Some(connection.public_url.clone());
        status.last_error = error;
    }
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
            | "host"
    )
}

fn is_valid_path_and_query(value: &str) -> bool {
    value.starts_with('/')
        && !value.starts_with("//")
        && !value.contains(['\\', '#'])
        && !value.chars().any(char::is_control)
}

fn response_headers(headers: &reqwest::header::HeaderMap) -> HashMap<String, Vec<String>> {
    let mut output = HashMap::<String, Vec<String>>::new();
    for (name, value) in headers {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            output
                .entry(name.as_str().to_owned())
                .or_default()
                .push(value.to_owned());
        }
    }
    output
}

async fn send_json(outgoing: &mpsc::Sender<Message>, value: Value) -> Result<()> {
    outgoing
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| anyhow!("tunnel WebSocket writer is closed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        extract::{Query, State},
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use chatcmd_storage::SqliteRepository;
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tempfile::tempdir;
    use tokio::sync::oneshot;

    #[derive(Debug)]
    struct CapturedRequest {
        query: HashMap<String, String>,
        header: String,
        body: Bytes,
    }

    #[tokio::test]
    async fn proxy_forwards_post_semantics_and_streams_large_response() {
        let (captured_tx, captured_rx) = oneshot::channel();
        let captured_tx = Arc::new(std::sync::Mutex::new(Some(captured_tx)));
        let app = Router::new()
            .route(
                "/echo",
                post(
                    |State(captured_tx): State<
                        Arc<std::sync::Mutex<Option<oneshot::Sender<CapturedRequest>>>>,
                    >,
                     Query(query): Query<HashMap<String, String>>,
                     headers: HeaderMap,
                     body: Bytes| async move {
                        let captured = CapturedRequest {
                            query,
                            header: headers
                                .get("x-forwarded-test")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_owned(),
                            body,
                        };
                        if let Some(sender) = captured_tx.lock().expect("capture lock").take() {
                            let _ = sender.send(captured);
                        }
                        (
                            StatusCode::MULTI_STATUS,
                            [("x-tunnel-test", "forwarded")],
                            vec![b'x'; RESPONSE_CHUNK_BYTES * 2 + 17],
                        )
                    },
                ),
            )
            .with_state(captured_tx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let port = listener.local_addr().expect("mock address").port();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve mock");
        });

        let directory = tempdir().expect("temp directory");
        let (repository, bootstrap) =
            SqliteRepository::open(&directory.path().join("tunnel-test.db"), 1)
                .await
                .expect("open test database");
        let manager = TunnelClientManager::new(repository, bootstrap.device, port);
        let request_body = br#"{"hello":"tunnel"}"#;
        let frame = TunnelServerFrame {
            message_type: "request".to_owned(),
            request_id: Some("request-1".to_owned()),
            method: Some("POST".to_owned()),
            path_and_query: Some("/echo?answer=42".to_owned()),
            headers: Some(HashMap::from([
                ("x-forwarded-test".to_owned(), vec!["yes".to_owned()]),
                ("host".to_owned(), vec!["evil.example".to_owned()]),
            ])),
            body_base64: Some(STANDARD.encode(request_body)),
        };
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(16);

        manager
            .proxy_request_inner(&frame, &outgoing_tx)
            .await
            .expect("proxy request");
        drop(outgoing_tx);

        let captured = captured_rx.await.expect("captured request");
        assert_eq!(captured.query.get("answer").map(String::as_str), Some("42"));
        assert_eq!(captured.header, "yes");
        assert_eq!(captured.body.as_ref(), request_body);

        let mut frames = Vec::new();
        while let Some(Message::Text(text)) = outgoing_rx.recv().await {
            frames.push(serde_json::from_str::<Value>(&text).expect("response frame JSON"));
        }
        assert_eq!(frames[0]["type"], "responseStart");
        assert_eq!(frames[0]["statusCode"], 207);
        assert_eq!(frames[0]["headers"]["x-tunnel-test"][0], "forwarded");
        assert!(
            frames
                .iter()
                .filter(|frame| frame["type"] == "responseChunk")
                .count()
                >= 3
        );
        assert_eq!(frames.last().expect("response end")["type"], "responseEnd");

        server.abort();
    }

    #[tokio::test]
    async fn concurrent_connect_uses_one_worker_and_stop_ends_retries() {
        let registrations = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/api/tunnel/register",
                post(|State(registrations): State<Arc<AtomicUsize>>| async move {
                    registrations.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({
                        "key": "stable-key",
                        "connectionSecret": "stable-secret",
                        "publicUrl": "http://tunnel.test/tunnel/stable-key",
                        "webSocketUrl": "ws://127.0.0.1:9/api/tunnel/connect/stable-key"
                    }))
                }),
            )
            .with_state(Arc::clone(&registrations));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind registration server");
        let server_url = format!(
            "http://{}",
            listener.local_addr().expect("registration address")
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve registration mock");
        });

        let directory = tempdir().expect("temp directory");
        let (repository, bootstrap) =
            SqliteRepository::open(&directory.path().join("connect-race-test.db"), 1)
                .await
                .expect("open test database");
        let persisted = PersistedTunnelConnection {
            server_url,
            key: "stable-key".to_owned(),
            secret: "stable-secret".to_owned(),
            public_url: "http://tunnel.test/tunnel/stable-key".to_owned(),
            websocket_url: "ws://127.0.0.1:9/api/tunnel/connect/stable-key".to_owned(),
            enabled: false,
        };
        sqlx::query(
            "INSERT INTO settings(key,value_json,updated_at_ms) VALUES(?,?,?) \
             ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        )
        .bind(TUNNEL_SETTING_KEY)
        .bind(serde_json::to_string(&persisted).expect("persisted JSON"))
        .bind(1_i64)
        .execute(repository.pool())
        .await
        .expect("seed tunnel settings");
        let manager = TunnelClientManager::new(repository, bootstrap.device, 8080);

        let (first, second) = tokio::join!(manager.connect(), manager.connect());
        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(registrations.load(Ordering::SeqCst), 1);

        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(registrations.load(Ordering::SeqCst) >= 2);
        manager.disconnect().await.expect("disconnect tunnel");
        let count_after_stop = registrations.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert_eq!(registrations.load(Ordering::SeqCst), count_after_stop);
        assert_eq!(manager.status().await.state, "disconnected");

        server.abort();
    }

    #[test]
    fn path_validation_rejects_authority_absolute_and_malformed_paths() {
        assert!(is_valid_path_and_query("/api/ping?a=1"));
        for invalid in [
            "http://169.254.169.254/latest/meta-data",
            "//evil.example/path",
            "/\\evil.example/path",
            "/api/ping#fragment",
            "/api/\nping",
        ] {
            assert!(!is_valid_path_and_query(invalid), "accepted {invalid:?}");
        }
    }

    #[test]
    fn hop_by_hop_headers_are_filtered() {
        for name in [
            "Connection",
            "Keep-Alive",
            "Transfer-Encoding",
            "Upgrade",
            "Host",
            "Content-Length",
            "TE",
            "Trailer",
            "Proxy-Authorization",
            "Proxy-Authenticate",
        ] {
            assert!(is_hop_by_hop_header(name), "did not filter {name}");
        }
        assert!(!is_hop_by_hop_header("content-type"));
    }
}
