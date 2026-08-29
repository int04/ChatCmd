use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, Payload},
    KeyInit,
};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chatcmd_storage::SqliteRepository;
use futures_util::{SinkExt, StreamExt};
use hkdf::Hkdf;
use p256::{
    PublicKey,
    ecdh::EphemeralSecret,
    elliptic_curve::rand_core::{OsRng, RngCore},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::broadcast;
use uuid::Uuid;

const WS_CRYPTO_PROTOCOL: u8 = 1;
const WS_AAD: &[u8] = b"chatcmd/ws/v1";
const WS_HKDF_INFO: &[u8] = b"chatcmd/ws/aes-256-gcm/v1";
const WS_HANDSHAKE_TIMEOUT_SECONDS: u64 = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub payload: Value,
}

impl AppEvent {
    pub(crate) fn new(event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            occurred_at: crate::api::iso_now(),
            task_id: None,
            session_id: None,
            turn_id: None,
            payload: redact_value(payload, 0),
        }
    }
}

pub(crate) struct AppState {
    pub repository: SqliteRepository,
    pub database_path: String,
    pub bind_address: String,
    pub port: u16,
    pub started_at: String,
    pub device: chatcmd_core::LocalDevice,
    pub shell: chatcmd_runtime::ShellRuntime,
    pub skills: chatcmd_runtime::SkillService,
    pub activities: crate::runtime_host::ActivityRegistry,
    events: broadcast::Sender<AppEvent>,
    connected_clients: AtomicUsize,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        repository: SqliteRepository,
        database_path: String,
        bind_address: String,
        port: u16,
        device: chatcmd_core::LocalDevice,
        shell: chatcmd_runtime::ShellRuntime,
        skills: chatcmd_runtime::SkillService,
        activities: crate::runtime_host::ActivityRegistry,
        events: broadcast::Sender<AppEvent>,
    ) -> Self {
        Self {
            repository,
            database_path,
            bind_address,
            port,
            started_at: crate::api::iso_now(),
            device,
            shell,
            skills,
            activities,
            events,
            connected_clients: AtomicUsize::new(0),
        }
    }

    pub(crate) fn connected_clients(&self) -> usize {
        self.connected_clients.load(Ordering::Relaxed)
    }

    pub(crate) fn publish(&self, event: AppEvent) {
        let _ = self.events.send(event);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientHello {
    #[serde(rename = "type")]
    message_type: String,
    protocol: u8,
    public_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerHello {
    #[serde(rename = "type")]
    message_type: &'static str,
    protocol: u8,
    public_key: String,
    salt: String,
}

pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    state.connected_clients.fetch_add(1, Ordering::Relaxed);

    let Some(cipher) = establish_encrypted_session(&mut socket).await else {
        state.connected_clients.fetch_sub(1, Ordering::Relaxed);
        return;
    };

    let mut receiver = state.events.subscribe();
    let (mut sender, mut incoming) = socket.split();
    let connected = AppEvent::new(
        "system.connected",
        json!({ "connectedClients": state.connected_clients() }),
    );
    if send_encrypted_json(&mut sender, &cipher, &connected).await.is_err() {
        state.connected_clients.fetch_sub(1, Ordering::Relaxed);
        return;
    }

    loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Ok(event) => {
                    if send_encrypted_json(&mut sender, &cipher, &event).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Binary(data))) => {
                    if decrypt_client_payload(&cipher, data.as_ref()).is_none() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(data))) => {
                    if sender.send(Message::Pong(data)).await.is_err() { break; }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Text(_))) => {
                    // Once key agreement succeeds, plaintext application frames are forbidden.
                    break;
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            }
        }
    }
    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
}

async fn establish_encrypted_session(socket: &mut WebSocket) -> Option<Aes256Gcm> {
    let incoming = tokio::time::timeout(
        std::time::Duration::from_secs(WS_HANDSHAKE_TIMEOUT_SECONDS),
        socket.next(),
    )
    .await
    .ok()??
    .ok()?;

    let Message::Text(text) = incoming else {
        return None;
    };
    let hello: ClientHello = serde_json::from_str(text.as_str()).ok()?;
    if hello.message_type != "crypto.clientHello" || hello.protocol != WS_CRYPTO_PROTOCOL {
        return None;
    }

    let client_public_bytes = URL_SAFE_NO_PAD.decode(hello.public_key).ok()?;
    let client_public = PublicKey::from_sec1_bytes(&client_public_bytes).ok()?;
    let server_secret = EphemeralSecret::random(&mut OsRng);
    let server_public = PublicKey::from(&server_secret);
    let shared_secret = server_secret.diffie_hellman(&client_public);

    let mut salt = [0_u8; 32];
    OsRng.fill_bytes(&mut salt);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared_secret.raw_secret_bytes().as_slice());
    let mut session_key = [0_u8; 32];
    hkdf.expand(WS_HKDF_INFO, &mut session_key).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&session_key).ok()?;
    session_key.fill(0);

    let response = ServerHello {
        message_type: "crypto.serverHello",
        protocol: WS_CRYPTO_PROTOCOL,
        public_key: URL_SAFE_NO_PAD.encode(server_public.to_sec1_bytes()),
        salt: URL_SAFE_NO_PAD.encode(salt),
    };
    let response_text = serde_json::to_string(&response).ok()?;
    socket.send(Message::Text(response_text.into())).await.ok()?;
    Some(cipher)
}

async fn send_encrypted_json<S, T>(
    sender: &mut S,
    cipher: &Aes256Gcm,
    value: &T,
) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
    T: Serialize,
{
    let plaintext = serde_json::to_vec(value).map_err(|_| ())?;
    let packet = encrypt_payload(cipher, &plaintext).ok_or(())?;
    sender.send(Message::Binary(packet.into())).await.map_err(|_| ())
}

fn encrypt_payload(cipher: &Aes256Gcm, plaintext: &[u8]) -> Option<Vec<u8>> {
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: WS_AAD,
            },
        )
        .ok()?;

    let mut packet = Vec::with_capacity(1 + nonce_bytes.len() + ciphertext.len());
    packet.push(WS_CRYPTO_PROTOCOL);
    packet.extend_from_slice(&nonce_bytes);
    packet.extend_from_slice(&ciphertext);
    Some(packet)
}

fn decrypt_client_payload(cipher: &Aes256Gcm, packet: &[u8]) -> Option<Value> {
    if packet.len() <= 13 || packet[0] != WS_CRYPTO_PROTOCOL {
        return None;
    }
    let nonce = Nonce::from_slice(&packet[1..13]);
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &packet[13..],
                aad: WS_AAD,
            },
        )
        .ok()?;
    serde_json::from_slice(&plaintext).ok()
}

fn redact_value(value: Value, depth: usize) -> Value {
    if depth >= 8 {
        return Value::String("[TRUNCATED]".to_owned());
    }
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let value = if lower.contains("token")
                        || lower.contains("secret")
                        || lower == "authorization"
                    {
                        Value::String("[REDACTED]".to_owned())
                    } else {
                        redact_value(value, depth + 1)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(500)
                .map(|value| redact_value(value, depth + 1))
                .collect(),
        ),
        Value::String(value) if value.len() > 65_536 => {
            Value::String(value.chars().take(65_536).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_payload_round_trips_and_plaintext_is_not_visible() {
        let key = [7_u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).expect("valid AES key");
        let plaintext = br#"{\"type\":\"task.updated\",\"payload\":\"secret-data\"}"#;

        let packet = encrypt_payload(&cipher, plaintext).expect("encrypt");
        assert_eq!(packet[0], WS_CRYPTO_PROTOCOL);
        assert!(!packet.windows(b"secret-data".len()).any(|part| part == b"secret-data"));

        let decrypted = cipher
            .decrypt(
                Nonce::from_slice(&packet[1..13]),
                Payload { msg: &packet[13..], aad: WS_AAD },
            )
            .expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn client_payload_rejects_tampering() {
        let key = [9_u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).expect("valid AES key");
        let mut packet = encrypt_payload(&cipher, br#"{\"type\":\"client.ready\"}"#).expect("encrypt");
        let last = packet.last_mut().expect("packet byte");
        *last ^= 0x01;

        assert!(decrypt_client_payload(&cipher, &packet).is_none());
    }
}
