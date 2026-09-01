use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const PAYMENT_CREATE_URL: &str = "https://payment.toicontre.net/api/payment/create";
const MAX_PAYMENT_AMOUNT: u64 = 300_000_000;
const MAX_CONTENT_CHARS: usize = 140;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreatePaymentRequest {
    amount: u64,
    content: String,
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "success": false,
            "code": code,
            "message": message,
        })),
    )
        .into_response()
}

pub(super) async fn create_payment(Json(input): Json<CreatePaymentRequest>) -> Response {
    if input.amount == 0 || input.amount > MAX_PAYMENT_AMOUNT {
        return error_response(
            StatusCode::BAD_REQUEST,
            "INVALID_AMOUNT",
            "amount must be between 1 and 300000000 VND.",
        );
    }

    let content = input.content.trim();
    if content.is_empty() || content.chars().count() > MAX_CONTENT_CHARS {
        return error_response(
            StatusCode::BAD_REQUEST,
            "INVALID_CONTENT",
            "content must be between 1 and 140 characters.",
        );
    }

    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PAYMENT_INTERNAL_ERROR",
                "could not initialize payment client.",
            );
        }
    };

    let response = match client
        .post(PAYMENT_CREATE_URL)
        .json(&CreatePaymentRequest {
            amount: input.amount,
            content: content.to_owned(),
        })
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "PAYMENT_PROVIDER_UNAVAILABLE",
                "payment service is temporarily unavailable.",
            );
        }
    };

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let payload = response.json::<Value>().await.ok();

    if !status.is_success() {
        if let Some(payload) = payload {
            if payload.get("success").and_then(Value::as_bool) == Some(false)
                && payload.get("code").and_then(Value::as_str).is_some()
            {
                return (status, Json(payload)).into_response();
            }
        }

        if status.is_client_error() {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "PAYMENT_PROVIDER_REJECTED",
                "payment provider rejected the request.",
            );
        }

        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "PAYMENT_PROVIDER_UNAVAILABLE",
            "payment service is temporarily unavailable.",
        );
    }

    let Some(payload) = payload else {
        return error_response(
            StatusCode::BAD_GATEWAY,
            "PAYMENT_PROVIDER_REJECTED",
            "payment service returned an invalid response.",
        );
    };

    (status, Json(payload)).into_response()
}
