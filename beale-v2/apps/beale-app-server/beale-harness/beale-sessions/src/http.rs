use axum::http::{HeaderMap, StatusCode, header};
use beale_contracts::PROTOBUF_CONTENT_TYPE;
use prost::Message;

pub(super) fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(presented) = value.strip_prefix("Bearer ") else {
        return false;
    };
    let bytes = presented.as_bytes();
    if bytes.len() != token.len() {
        return false;
    }
    bytes
        .iter()
        .zip(token.as_bytes())
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

pub(super) fn checked_body<T: Message + Default>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    T::decode(body).map_err(|_| StatusCode::BAD_REQUEST)
}

pub(super) fn response<T: Message>(
    message: T,
) -> ([(header::HeaderName, &'static str); 1], Vec<u8>) {
    (
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        message.encode_to_vec(),
    )
}
