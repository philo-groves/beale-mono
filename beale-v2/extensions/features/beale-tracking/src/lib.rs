//! Host-owned tracking history and its bounded client read route.

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::get,
};
use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, TRACKING_PATH,
    v1::{TrackingModelDay, TrackingResponse, TrackingSubscriptionPoint},
};
use beale_storage::{HostDatabase, SubscriptionUsageRow};
use prost::Message;
use std::collections::BTreeMap;

pub const FEATURE_ID: &str = "tracking";
pub const FEATURE_NAME: &str = "Tracking";
pub const FEATURE_DESCRIPTION: &str =
    "See observed subscription limits and Beale session model usage over the past week.";
pub const HAS_PROMPT_EXTENSION: bool = false;
pub const TOOL_COUNT: u32 = 0;
pub const PROMPT_EXTENSION: &str = "";

#[derive(Clone)]
struct TrackingState {
    token: String,
    db: HostDatabase,
}

pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(TRACKING_PATH, get(history))
        .with_state(TrackingState { token, db })
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(presented) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    presented.len() == token.len()
        && presented
            .as_bytes()
            .iter()
            .zip(token.as_bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn sample_subscriptions(points: Vec<SubscriptionUsageRow>) -> Vec<SubscriptionUsageRow> {
    let mut series: BTreeMap<(i32, String), Vec<SubscriptionUsageRow>> = BTreeMap::new();
    for point in points {
        series
            .entry((point.provider, point.window_id.clone()))
            .or_default()
            .push(point);
    }
    let per_series = (5_000 / series.len().max(1)).max(2);
    series
        .into_values()
        .flat_map(|values| {
            let len = values.len();
            let count = len.min(per_series);
            values
                .into_iter()
                .enumerate()
                .filter(move |(index, _)| {
                    len <= count
                        || *index == 0
                        || *index == len - 1
                        || *index * (count - 1) / (len - 1)
                            != (*index - 1) * (count - 1) / (len - 1)
                })
                .map(|(_, point)| point)
        })
        .collect()
}

async fn history(
    State(state): State<TrackingState>,
    headers: HeaderMap,
) -> Result<([(header::HeaderName, &'static str); 1], Vec<u8>), StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !state
        .db
        .feature_enabled(FEATURE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_millis() as i64;
    let since_ms = now_ms.saturating_sub(7 * 86_400_000);
    let subscriptions = sample_subscriptions(
        state
            .db
            .subscription_history(since_ms)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_iter()
    .map(|point| {
        if !(0..=100).contains(&point.used_percent) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(TrackingSubscriptionPoint {
            provider: point.provider,
            window_id: point.window_id,
            label: point.label,
            observed_at_ms: u64::try_from(point.observed_at_ms)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            remaining_percent: 100 - point.used_percent as u32,
            resets_at_unix: u64::try_from(point.resets_at_unix)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        })
    })
    .collect::<Result<Vec<_>, StatusCode>>()?;
    let model_days = state
        .db
        .model_usage_days(since_ms)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .map(|day| {
            Ok(TrackingModelDay {
                day_start_ms: u64::try_from(day.day_start_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                provider: day.provider,
                method: day.method,
                model: day.model,
                role: day.role,
                input_tokens: u64::try_from(day.input_tokens)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                output_tokens: u64::try_from(day.output_tokens)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                duration_ms: u64::try_from(day.duration_ms)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                measured_calls: u32::try_from(day.measured_calls)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                total_calls: u32::try_from(day.total_calls)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            })
        })
        .collect::<Result<Vec<_>, StatusCode>>()?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        TrackingResponse {
            subscriptions,
            model_days,
        }
        .encode_to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_long_window_history_with_endpoints() {
        let points = (0..8_000)
            .map(|observed_at_ms| SubscriptionUsageRow {
                provider: 1,
                window_id: "weekly".to_owned(),
                label: "Week".to_owned(),
                observed_at_ms,
                used_percent: 35,
                resets_at_unix: 0,
            })
            .collect();
        let sampled = sample_subscriptions(points);
        assert!(sampled.len() <= 5_000);
        assert_eq!(sampled.first().unwrap().observed_at_ms, 0);
        assert_eq!(sampled.last().unwrap().observed_at_ms, 7_999);
    }

    #[tokio::test]
    async fn history_requires_auth_and_enabled_feature() {
        let root = std::env::temp_dir().join(format!(
            "beale-tracking-example-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = HostDatabase::open(&root).unwrap();
        let state = TrackingState {
            token: "example-token".to_owned(),
            db: db.clone(),
        };
        assert_eq!(
            history(State(state.clone()), HeaderMap::new())
                .await
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer example-token".parse().unwrap(),
        );
        assert_eq!(
            history(State(state.clone()), headers.clone())
                .await
                .unwrap_err(),
            StatusCode::NOT_FOUND
        );
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        db.record_subscription_usage(&[SubscriptionUsageRow {
            provider: 1,
            window_id: "weekly".to_owned(),
            label: "Week".to_owned(),
            observed_at_ms: now_ms,
            used_percent: 35,
            resets_at_unix: 0,
        }])
        .unwrap();
        let (_, body) = history(State(state), headers).await.unwrap();
        let response = TrackingResponse::decode(body.as_slice()).unwrap();
        assert_eq!(response.subscriptions.len(), 1);
        assert_eq!(response.subscriptions[0].remaining_percent, 65);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
