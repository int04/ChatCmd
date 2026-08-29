use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, Payload},
    KeyInit,
};
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::{
    PublicKey,
    ecdh::EphemeralSecret,
    elliptic_curve::rand_core::{OsRng, RngCore},
};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::RwLock;

const PROTOCOL: u8 = 1;
const HANDSHAKE_AAD: &[u8] = b"chatcmd/backend-api/handshake-obfuscation/v1";
const HKDF_INFO: &[u8] = b"chatcmd/backend-api/aes-256-gcm/v1";
const KEY_A: [u8; 32] = [
    0x9d,0x23,0x71,0xc4,0x5a,0xe8,0x16,0x3b,0x42,0xaf,0xd1,0x67,0x08,0xbe,0x95,0xf2,
    0x31,0x6c,0xa9,0x0d,0x77,0xd4,0x58,0x83,0xe1,0x4f,0xb6,0x2a,0xc8,0x19,0x65,0x90,
];
const KEY_B: [u8; 32] = [
    0x4a,0x91,0xc6,0x3e,0xeb,0x52,0xa7,0xd0,0xf5,0x1b,0x64,0x92,0xbd,0x07,0x2c,0x49,
    0xe8,0xd3,0x15,0xba,0x20,0x6f,0xc1,0x34,0x97,0xaa,0x03,0xfd,0x5e,0xb2,0x48,0x27,
];

#[derive(Clone)]
pub(crate) struct BackendApiClient {
    http: Client,
    base_url: String,
    session: Arc<RwLock<Option<BackendSession>>>,
}

#[derive(Clone)]
struct BackendSession {
    id: String,
    cipher: Arc<Aes256Gcm>,
}

pub(crate) struct BackendApiResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientHello {
    #[serde(rename = "type")]
    message_type: &'static str,
    protocol: u8,
    public_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerHello {
    #[serde(rename = "type")]
    message_type: String,
    protocol: u8,
    session_id: String,
    public_key: String,
    salt: String,
}

impl BackendApiClient {
    pub(crate) fn from_environment() -> Result<Self> {
        let base_url = configured_backend_url()?;
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("build backend API client")?;
        Ok(Self {
            http,
            base_url,
            session: Arc::new(RwLock::new(None)),
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) async fn request(
        &self,
        method: Method,
        path_and_query: &str,
        body: &[u8],
        authorization: Option<&str>,
        accept_language: Option<&str>,
    ) -> Result<BackendApiResponse> {
        match self.request_once(method.clone(), path_and_query, body, authorization, accept_language).await? {
            RequestAttempt::Complete(response) => Ok(response),
            RequestAttempt::Reset => {
                *self.session.write().await = None;
                match self.request_once(method, path_and_query, body, authorization, accept_language).await? {
                    RequestAttempt::Complete(response) => Ok(response),
                    RequestAttempt::Reset => bail!("backend API crypto session reset twice"),
                }
            }
        }
    }

    async fn request_once(
        &self,
        method: Method,
        path_and_query: &str,
        body: &[u8],
        authorization: Option<&str>,
        accept_language: Option<&str>,
    ) -> Result<RequestAttempt> {
        let session = self.session().await?;
        let aad = api_aad("request", method.as_str(), path_and_query, None);
        let encrypted_body = if body.is_empty() {
            Vec::new()
        } else {
            encrypt_packet(&session.cipher, body, aad.as_bytes()).ok_or_else(|| anyhow!("encrypt backend request"))?
        };
        let url = format!("{}{}", self.base_url, path_and_query);
        let mut request = self.http.request(method.clone(), url)
            .header("x-chatcmd-crypto", "1")
            .header("x-chatcmd-crypto-session", &session.id)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
        if let Some(value) = authorization { request = request.header(reqwest::header::AUTHORIZATION, value); }
        if let Some(value) = accept_language { request = request.header(reqwest::header::ACCEPT_LANGUAGE, value); }
        if !encrypted_body.is_empty() { request = request.body(encrypted_body); }
        let response = request.send().await.context("send encrypted backend request")?;
        if response.headers().get("x-chatcmd-crypto-reset").is_some() {
            return Ok(RequestAttempt::Reset);
        }
        let status = response.status();
        let content_type = response.headers().get("x-chatcmd-plain-content-type")
            .or_else(|| response.headers().get(reqwest::header::CONTENT_TYPE))
            .and_then(|value| value.to_str().ok()).map(str::to_owned);
        let is_encrypted = response.headers().get("x-chatcmd-crypto")
            .and_then(|value| value.to_str().ok()) == Some("1");
        let bytes = response.bytes().await.context("read backend response")?;
        let body = if bytes.is_empty() || status == StatusCode::NO_CONTENT {
            Vec::new()
        } else {
            if !is_encrypted { bail!("backend API returned plaintext response"); }
            let aad = api_aad("response", method.as_str(), path_and_query, Some(status.as_u16()));
            decrypt_packet(&session.cipher, &bytes, aad.as_bytes())
                .ok_or_else(|| anyhow!("decrypt backend response"))?
        };
        Ok(RequestAttempt::Complete(BackendApiResponse { status: status.as_u16(), content_type, body }))
    }

    async fn session(&self) -> Result<BackendSession> {
        if let Some(session) = self.session.read().await.clone() {
            return Ok(session);
        }
        let created = self.handshake().await?;
        let mut guard = self.session.write().await;
        if let Some(existing) = guard.clone() {
            return Ok(existing);
        }
        *guard = Some(created.clone());
        Ok(created)
    }

    async fn handshake(&self) -> Result<BackendSession> {
        let handshake_cipher = handshake_cipher().ok_or_else(|| anyhow!("build backend handshake cipher"))?;
        let secret = EphemeralSecret::random(&mut OsRng);
        let public = PublicKey::from(&secret);
        let hello = ClientHello {
            message_type: "crypto.clientHello",
            protocol: PROTOCOL,
            public_key: URL_SAFE_NO_PAD.encode(public.to_sec1_bytes()),
        };
        let plaintext = serde_json::to_vec(&hello).context("serialize backend client hello")?;
        let packet = encrypt_packet(&handshake_cipher, &plaintext, HANDSHAKE_AAD)
            .ok_or_else(|| anyhow!("encrypt backend client hello"))?;
        let response = self.http
            .post(format!("{}/api/crypto/handshake", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(packet)
            .send().await.context("send backend crypto handshake")?;
        if !response.status().is_success() { bail!("backend crypto handshake failed ({})", response.status()); }
        let response_packet = response.bytes().await.context("read backend server hello")?;
        let response_plaintext = decrypt_packet(&handshake_cipher, &response_packet, HANDSHAKE_AAD)
            .ok_or_else(|| anyhow!("decrypt backend server hello"))?;
        let server: ServerHello = serde_json::from_slice(&response_plaintext).context("parse backend server hello")?;
        if server.message_type != "crypto.serverHello" || server.protocol != PROTOCOL || server.session_id.is_empty() {
            bail!("unsupported backend crypto handshake");
        }
        let server_public_bytes = URL_SAFE_NO_PAD.decode(server.public_key).context("decode backend public key")?;
        let server_public = PublicKey::from_sec1_bytes(&server_public_bytes).context("parse backend public key")?;
        let shared = secret.diffie_hellman(&server_public);
        let salt = URL_SAFE_NO_PAD.decode(server.salt).context("decode backend HKDF salt")?;
        let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared.raw_secret_bytes().as_slice());
        let mut key = [0_u8; 32];
        hkdf.expand(HKDF_INFO, &mut key).map_err(|_| anyhow!("derive backend session key"))?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("create backend session cipher"))?;
        key.fill(0);
        Ok(BackendSession { id: server.session_id, cipher: Arc::new(cipher) })
    }
}

enum RequestAttempt { Complete(BackendApiResponse), Reset }

fn configured_backend_url() -> Result<String> {
    if let Ok(value) = std::env::var("CHATCMD_BACKEND_API_URL") {
        let value = value.trim().trim_end_matches('/');
        if !value.is_empty() { return Ok(value.to_owned()); }
    }
    if cfg!(debug_assertions) {
        return Ok("http://127.0.0.1:5121".to_owned());
    }
    option_env!("CHATCMD_BACKEND_API_RELEASE_URL")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
        .ok_or_else(|| anyhow!("release backend URL is missing; set CHATCMD_BACKEND_API_RELEASE_URL when building"))
}

fn api_aad(direction: &str, method: &str, path: &str, status: Option<u16>) -> String {
    match status {
        Some(status) => format!("chatcmd/backend-api/v1|{direction}|{method}|{path}|{status}"),
        None => format!("chatcmd/backend-api/v1|{direction}|{method}|{path}"),
    }
}

fn handshake_cipher() -> Option<Aes256Gcm> {
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() { *byte = KEY_A[index] ^ KEY_B[index]; }
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    key.fill(0);
    Some(cipher)
}

fn encrypt_packet(cipher: &Aes256Gcm, plaintext: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad }).ok()?;
    let mut packet = Vec::with_capacity(13 + ciphertext.len());
    packet.push(PROTOCOL); packet.extend_from_slice(&nonce); packet.extend_from_slice(&ciphertext); Some(packet)
}

fn decrypt_packet(cipher: &Aes256Gcm, packet: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    if packet.len() <= 13 || packet[0] != PROTOCOL { return None; }
    cipher.decrypt(Nonce::from_slice(&packet[1..13]), Payload { msg: &packet[13..], aad }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn debug_default_is_local_backend() {
        if cfg!(debug_assertions) && std::env::var("CHATCMD_BACKEND_API_URL").is_err() {
            assert_eq!(configured_backend_url().expect("url"), "http://127.0.0.1:5121");
        }
    }
    #[test]
    fn packet_round_trip_binds_aad() {
        let cipher = Aes256Gcm::new_from_slice(&[4_u8; 32]).expect("cipher");
        let aad = api_aad("request", "POST", "/api/system/ping", None);
        let packet = encrypt_packet(&cipher, b"{\"hello\":1}", aad.as_bytes()).expect("encrypt");
        assert_eq!(decrypt_packet(&cipher, &packet, aad.as_bytes()).expect("decrypt"), b"{\"hello\":1}");
        assert!(decrypt_packet(&cipher, &packet, b"wrong").is_none());
    }

    #[tokio::test]
    async fn local_dotnet_backend_interop_when_enabled() {
        if std::env::var_os("CHATCMD_TEST_BACKEND_INTEROP").is_none() {
            return;
        }
        let client = BackendApiClient::from_environment().expect("backend client");
        let response = client
            .request(Method::GET, "/api/system/ping", &[], None, None)
            .await
            .expect("encrypted backend request");
        assert_eq!(response.status, 200);
        let payload: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON response");
        assert_eq!(payload.get("status").and_then(serde_json::Value::as_str), Some("ok"));
    }
}
