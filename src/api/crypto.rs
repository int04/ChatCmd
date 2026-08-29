use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{OriginalUri, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::{
    PublicKey,
    ecdh::EphemeralSecret,
    elliptic_curve::rand_core::{OsRng, RngCore},
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use super::Problem;
use crate::websocket::AppState;

const API_CRYPTO_PROTOCOL: u8 = 1;
const API_HANDSHAKE_AAD: &[u8] = b"chatcmd/api/handshake-obfuscation/v1";
const API_HKDF_INFO: &[u8] = b"chatcmd/api/aes-256-gcm/v1";
const MAX_API_BODY_BYTES: usize = 64 * 1024 * 1024;

const API_HANDSHAKE_KEY_A: [u8; 32] = [
    0x9d, 0x23, 0x71, 0xc4, 0x5a, 0xe8, 0x16, 0x3b, 0x42, 0xaf, 0xd1, 0x67, 0x08, 0xbe, 0x95, 0xf2,
    0x31, 0x6c, 0xa9, 0x0d, 0x77, 0xd4, 0x58, 0x83, 0xe1, 0x4f, 0xb6, 0x2a, 0xc8, 0x19, 0x65, 0x90,
];
const API_HANDSHAKE_KEY_B: [u8; 32] = [
    0x4a, 0x91, 0xc6, 0x3e, 0xeb, 0x52, 0xa7, 0xd0, 0xf5, 0x1b, 0x64, 0x92, 0xbd, 0x07, 0x2c, 0x49,
    0xe8, 0xd3, 0x15, 0xba, 0x20, 0x6f, 0xc1, 0x34, 0x97, 0xaa, 0x03, 0xfd, 0x5e, 0xb2, 0x48, 0x27,
];

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
    session_id: String,
    public_key: String,
    salt: String,
}

pub(super) async fn handshake(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, Problem> {
    let handshake_cipher = handshake_cipher().ok_or_else(crypto_problem)?;
    let plaintext = decrypt_packet(&handshake_cipher, body.as_ref(), API_HANDSHAKE_AAD)
        .ok_or_else(crypto_problem)?;
    let hello: ClientHello = serde_json::from_slice(&plaintext).map_err(|_| crypto_problem())?;
    if hello.message_type != "crypto.clientHello" || hello.protocol != API_CRYPTO_PROTOCOL {
        return Err(crypto_problem());
    }

    let client_public_bytes = URL_SAFE_NO_PAD
        .decode(hello.public_key)
        .map_err(|_| crypto_problem())?;
    let client_public =
        PublicKey::from_sec1_bytes(&client_public_bytes).map_err(|_| crypto_problem())?;
    let server_secret = EphemeralSecret::random(&mut OsRng);
    let server_public = PublicKey::from(&server_secret);
    let shared_secret = server_secret.diffie_hellman(&client_public);

    let mut salt = [0_u8; 32];
    OsRng.fill_bytes(&mut salt);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared_secret.raw_secret_bytes().as_slice());
    let mut session_key = [0_u8; 32];
    hkdf.expand(API_HKDF_INFO, &mut session_key)
        .map_err(|_| crypto_problem())?;
    let session_cipher = Aes256Gcm::new_from_slice(&session_key).map_err(|_| crypto_problem())?;
    session_key.fill(0);

    let session_id = Uuid::new_v4().to_string();
    state
        .put_api_crypto_session(session_id.clone(), session_cipher)
        .await;

    let response = ServerHello {
        message_type: "crypto.serverHello",
        protocol: API_CRYPTO_PROTOCOL,
        session_id,
        public_key: URL_SAFE_NO_PAD.encode(server_public.to_sec1_bytes()),
        salt: URL_SAFE_NO_PAD.encode(salt),
    };
    let response_plaintext = serde_json::to_vec(&response).map_err(|_| crypto_problem())?;
    let packet = encrypt_packet(&handshake_cipher, &response_plaintext, API_HANDSHAKE_AAD)
        .ok_or_else(crypto_problem)?;
    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        Bytes::from(packet),
    )
        .into_response())
}

pub(super) async fn encrypted_local_api(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.uri().path().ends_with("/crypto/handshake") {
        return next.run(request).await;
    }

    if request.headers().get("x-chatcmdclient") != Some(&HeaderValue::from_static("local-ui")) {
        return next.run(request).await;
    }

    if request.headers().get("x-chatcmd-crypto") != Some(&HeaderValue::from_static("1")) {
        return reset_response(
            StatusCode::UPGRADE_REQUIRED,
            "encrypted local API is required",
        );
    }

    let Some(session_id) = request
        .headers()
        .get("x-chatcmd-crypto-session")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return reset_response(StatusCode::UNAUTHORIZED, "API crypto session is missing");
    };
    let Some(cipher) = state.api_crypto_session(&session_id).await else {
        return reset_response(StatusCode::UNAUTHORIZED, "API crypto session is invalid");
    };

    let method = request.method().as_str().to_owned();
    let path = request
        .extensions()
        .get::<OriginalUri>()
        .and_then(|original| {
            original
                .0
                .path_and_query()
                .map(|value| value.as_str().to_owned())
        })
        .or_else(|| {
            request
                .uri()
                .path_and_query()
                .map(|value| value.as_str().to_owned())
        })
        .unwrap_or_else(|| request.uri().path().to_owned());

    let (mut parts, body) = request.into_parts();
    let request_bytes = match to_bytes(body, MAX_API_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return encrypted_problem(
                &cipher,
                &method,
                &path,
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body is too large",
            );
        }
    };
    let plaintext_body = if request_bytes.is_empty() {
        Bytes::new()
    } else {
        let aad = api_aad("request", &method, &path, None);
        let Some(plaintext) = decrypt_packet(&cipher, request_bytes.as_ref(), aad.as_bytes())
        else {
            return encrypted_problem(
                &cipher,
                &method,
                &path,
                StatusCode::BAD_REQUEST,
                "Request encryption is invalid",
            );
        };
        Bytes::from(plaintext)
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    if !plaintext_body.is_empty() {
        parts.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    request = Request::from_parts(parts, Body::from(plaintext_body));

    let response = next.run(request).await;
    if response.status() == StatusCode::NO_CONTENT {
        return response;
    }
    let status = response.status();
    let (mut parts, body) = response.into_parts();
    let response_bytes = match to_bytes(body, MAX_API_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return encrypted_problem(
                &cipher,
                &method,
                &path,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Response body is too large",
            );
        }
    };
    if response_bytes.is_empty() {
        return Response::from_parts(parts, Body::empty());
    }

    let aad = api_aad("response", &method, &path, Some(status.as_u16()));
    let Some(packet) = encrypt_packet(&cipher, response_bytes.as_ref(), aad.as_bytes()) else {
        return reset_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "response encryption failed",
        );
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    parts
        .headers
        .insert("x-chatcmd-crypto", HeaderValue::from_static("1"));
    Response::from_parts(parts, Body::from(packet))
}

fn api_aad(direction: &str, method: &str, path: &str, status: Option<u16>) -> String {
    match status {
        Some(status) => format!("chatcmd/api/v1|{direction}|{method}|{path}|{status}"),
        None => format!("chatcmd/api/v1|{direction}|{method}|{path}"),
    }
}

fn handshake_cipher() -> Option<Aes256Gcm> {
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = API_HANDSHAKE_KEY_A[index] ^ API_HANDSHAKE_KEY_B[index];
    }
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    key.fill(0);
    Some(cipher)
}

fn encrypt_packet(cipher: &Aes256Gcm, plaintext: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
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
    let mut packet = Vec::with_capacity(13 + ciphertext.len());
    packet.push(API_CRYPTO_PROTOCOL);
    packet.extend_from_slice(&nonce_bytes);
    packet.extend_from_slice(&ciphertext);
    Some(packet)
}

fn decrypt_packet(cipher: &Aes256Gcm, packet: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    if packet.len() <= 13 || packet[0] != API_CRYPTO_PROTOCOL {
        return None;
    }
    cipher
        .decrypt(
            Nonce::from_slice(&packet[1..13]),
            Payload {
                msg: &packet[13..],
                aad,
            },
        )
        .ok()
}

fn reset_response(status: StatusCode, detail: &str) -> Response {
    let mut response = Problem::new(status, "API encryption error", detail).into_response();
    response
        .headers_mut()
        .insert("x-chatcmd-crypto-reset", HeaderValue::from_static("1"));
    response
}

fn encrypted_problem(
    cipher: &Aes256Gcm,
    method: &str,
    path: &str,
    status: StatusCode,
    detail: &str,
) -> Response {
    let payload = serde_json::json!({
        "type": "about:blank",
        "title": "API encryption error",
        "status": status.as_u16(),
        "detail": detail,
    });
    let plaintext = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
    let aad = api_aad("response", method, path, Some(status.as_u16()));
    let Some(packet) = encrypt_packet(cipher, &plaintext, aad.as_bytes()) else {
        return reset_response(StatusCode::INTERNAL_SERVER_ERROR, "API encryption failed");
    };
    let mut response = (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        Bytes::from(packet),
    )
        .into_response();
    response
        .headers_mut()
        .insert("x-chatcmd-crypto", HeaderValue::from_static("1"));
    response
}

fn crypto_problem() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "API encryption error",
        "encrypted API handshake is invalid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_packet_obfuscates_public_key() {
        let cipher = handshake_cipher().expect("handshake cipher");
        let plaintext = br#"{\"type\":\"crypto.clientHello\",\"publicKey\":\"public-value\"}"#;
        let packet = encrypt_packet(&cipher, plaintext, API_HANDSHAKE_AAD).expect("encrypt");
        assert!(
            !packet
                .windows(b"public-value".len())
                .any(|part| part == b"public-value")
        );
        assert_eq!(
            decrypt_packet(&cipher, &packet, API_HANDSHAKE_AAD).expect("decrypt"),
            plaintext
        );
    }

    #[test]
    fn aad_binds_method_path_and_status() {
        assert_ne!(
            api_aad("request", "GET", "/api/local/tasks", None),
            api_aad("request", "POST", "/api/local/tasks", None),
        );
        assert_ne!(
            api_aad("response", "GET", "/api/local/tasks", Some(200)),
            api_aad("response", "GET", "/api/local/tasks", Some(404)),
        );
    }
}
