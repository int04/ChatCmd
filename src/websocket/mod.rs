use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
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
use tokio::sync::{Mutex, RwLock, broadcast};
use uuid::Uuid;

const WS_CRYPTO_PROTOCOL: u8 = 1;
const WS_AAD: &[u8] = b"chatcmd/ws/v1";
const WS_HANDSHAKE_AAD: &[u8] = b"chatcmd/ws/handshake-obfuscation/v1";
const WS_HKDF_INFO: &[u8] = b"chatcmd/ws/aes-256-gcm/v1";
const WS_HANDSHAKE_TIMEOUT_SECONDS: u64 = 10;

// This fixed key only obfuscates the public ECDH handshake. It is intentionally
// not used as the session security boundary; session secrecy still comes from
// ephemeral ECDH + HKDF. Split/XOR storage merely avoids a directly searchable
// 32-byte key literal in either client or server source.
const WS_HANDSHAKE_KEY_A: [u8; 32] = [
    0x9d, 0x23, 0x71, 0xc4, 0x5a, 0xe8, 0x16, 0x3b, 0x42, 0xaf, 0xd1, 0x67, 0x08, 0xbe, 0x95, 0xf2,
    0x31, 0x6c, 0xa9, 0x0d, 0x77, 0xd4, 0x58, 0x83, 0xe1, 0x4f, 0xb6, 0x2a, 0xc8, 0x19, 0x65, 0x90,
];
const WS_HANDSHAKE_KEY_B: [u8; 32] = [
    0x4a, 0x91, 0xc6, 0x3e, 0xeb, 0x52, 0xa7, 0xd0, 0xf5, 0x1b, 0x64, 0x92, 0xbd, 0x07, 0x2c, 0x49,
    0xe8, 0xd3, 0x15, 0xba, 0x20, 0x6f, 0xc1, 0x34, 0x97, 0xaa, 0x03, 0xfd, 0x5e, 0xb2, 0x48, 0x27,
];

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
    pub backend_api: crate::backend_api::BackendApiClient,
    pub tunnel: Arc<crate::tunnel_client::TunnelClientManager>,
    pub updater: Arc<crate::updater::UpdateManager>,
    pub auth_refresh_lock: Mutex<()>,
    events: broadcast::Sender<AppEvent>,
    connected_clients: AtomicUsize,
    api_crypto_sessions: RwLock<HashMap<String, Arc<Aes256Gcm>>>,
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
        backend_api: crate::backend_api::BackendApiClient,
        events: broadcast::Sender<AppEvent>,
    ) -> Self {
        let tunnel = crate::tunnel_client::TunnelClientManager::new(
            repository.clone(),
            device.clone(),
            port,
        );
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
            backend_api,
            tunnel,
            updater: crate::updater::UpdateManager::new(),
            auth_refresh_lock: Mutex::new(()),
            events,
            connected_clients: AtomicUsize::new(0),
            api_crypto_sessions: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn connected_clients(&self) -> usize {
        self.connected_clients.load(Ordering::Relaxed)
    }

    pub(crate) fn publish(&self, event: AppEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) async fn put_api_crypto_session(&self, id: String, cipher: Aes256Gcm) {
        let mut sessions = self.api_crypto_sessions.write().await;
        if sessions.len() >= 512 {
            if let Some(oldest) = sessions.keys().next().cloned() {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(id, Arc::new(cipher));
    }

    pub(crate) async fn api_crypto_session(&self, id: &str) -> Option<Arc<Aes256Gcm>> {
        self.api_crypto_sessions.read().await.get(id).cloned()
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
    if send_encrypted_json(&mut sender, &cipher, &connected)
        .await
        .is_err()
    {
        state.connected_clients.fetch_sub(1, Ordering::Relaxed);
        return;
    }

    loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Ok(event) => {
                    if send_encrypted_json(&mut sender, &cipher, &event).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let resync = AppEvent::new(
                        "system.resync_required",
                        json!({ "reason": "broadcast_lag", "skippedEvents": skipped }),
                    );
                    if send_encrypted_json(&mut sender, &cipher, &resync).await.is_err() { break; }
                },
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

    let Message::Binary(packet) = incoming else {
        return None;
    };
    let handshake_cipher = handshake_cipher()?;
    let hello_plaintext = decrypt_handshake_payload(&handshake_cipher, packet.as_ref())?;
    let hello: ClientHello = serde_json::from_slice(&hello_plaintext).ok()?;
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
    let response_plaintext = serde_json::to_vec(&response).ok()?;
    let response_packet = encrypt_handshake_payload(&handshake_cipher, &response_plaintext)?;
    socket
        .send(Message::Binary(response_packet.into()))
        .await
        .ok()?;
    Some(cipher)
}

fn handshake_cipher() -> Option<Aes256Gcm> {
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = WS_HANDSHAKE_KEY_A[index] ^ WS_HANDSHAKE_KEY_B[index];
    }
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    key.fill(0);
    Some(cipher)
}

fn encrypt_handshake_payload(cipher: &Aes256Gcm, plaintext: &[u8]) -> Option<Vec<u8>> {
    encrypt_payload_with_aad(cipher, plaintext, WS_HANDSHAKE_AAD)
}

fn decrypt_handshake_payload(cipher: &Aes256Gcm, packet: &[u8]) -> Option<Vec<u8>> {
    decrypt_payload_with_aad(cipher, packet, WS_HANDSHAKE_AAD)
}

async fn send_encrypted_json<S, T>(sender: &mut S, cipher: &Aes256Gcm, value: &T) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
    T: Serialize,
{
    let plaintext = serde_json::to_vec(value).map_err(|_| ())?;
    let packet = encrypt_payload(cipher, &plaintext).ok_or(())?;
    sender
        .send(Message::Binary(packet.into()))
        .await
        .map_err(|_| ())
}

fn encrypt_payload(cipher: &Aes256Gcm, plaintext: &[u8]) -> Option<Vec<u8>> {
    encrypt_payload_with_aad(cipher, plaintext, WS_AAD)
}

fn encrypt_payload_with_aad(cipher: &Aes256Gcm, plaintext: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .ok()?;

    let mut packet = Vec::with_capacity(1 + nonce_bytes.len() + ciphertext.len());
    packet.push(WS_CRYPTO_PROTOCOL);
    packet.extend_from_slice(&nonce_bytes);
    packet.extend_from_slice(&ciphertext);
    Some(packet)
}

fn decrypt_payload_with_aad(cipher: &Aes256Gcm, packet: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    if packet.len() <= 13 || packet[0] != WS_CRYPTO_PROTOCOL {
        return None;
    }
    let nonce = Nonce::from_slice(&packet[1..13]);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: &packet[13..],
                aad,
            },
        )
        .ok()
}

fn decrypt_client_payload(cipher: &Aes256Gcm, packet: &[u8]) -> Option<Value> {
    let plaintext = decrypt_payload_with_aad(cipher, packet, WS_AAD)?;
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
        assert!(
            !packet
                .windows(b"secret-data".len())
                .any(|part| part == b"secret-data")
        );

        let decrypted = cipher
            .decrypt(
                Nonce::from_slice(&packet[1..13]),
                Payload {
                    msg: &packet[13..],
                    aad: WS_AAD,
                },
            )
            .expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn handshake_payload_is_binary_obfuscated_and_round_trips() {
        let cipher = handshake_cipher().expect("handshake cipher");
        let plaintext = br#"{\"type\":\"crypto.clientHello\",\"protocol\":1,\"publicKey\":\"visible-public-key\"}"#;
        let packet = encrypt_handshake_payload(&cipher, plaintext).expect("encrypt handshake");

        assert_eq!(packet[0], WS_CRYPTO_PROTOCOL);
        assert!(
            !packet
                .windows(b"crypto.clientHello".len())
                .any(|part| part == b"crypto.clientHello")
        );
        assert!(
            !packet
                .windows(b"visible-public-key".len())
                .any(|part| part == b"visible-public-key")
        );
        assert_eq!(
            decrypt_handshake_payload(&cipher, &packet).expect("decrypt handshake"),
            plaintext
        );
    }

    #[test]
    fn client_payload_rejects_tampering() {
        let key = [9_u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).expect("valid AES key");
        let mut packet =
            encrypt_payload(&cipher, br#"{\"type\":\"client.ready\"}"#).expect("encrypt");
        let last = packet.last_mut().expect("packet byte");
        *last ^= 0x01;

        assert!(decrypt_client_payload(&cipher, &packet).is_none());
    }
}
