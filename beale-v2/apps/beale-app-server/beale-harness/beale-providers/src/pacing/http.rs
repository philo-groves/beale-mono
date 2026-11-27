//! Trusted host control endpoint; worker credentials cannot call this router.
use super::*;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};

#[derive(Clone)]
struct HttpState {
    db: HostDatabase,
    token: String,
}
pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(PATH, post(handle))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(HttpState { db, token })
}
async fn handle(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<AccountingReply>, StatusCode> {
    if headers.get("authorization").and_then(|s| s.to_str().ok())
        != Some(&format!("Bearer {}", state.token))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: AccountingRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if let AccountingRequest::Drain {
        group_id,
        generation,
        participant_id,
    } = &request
    {
        let store = AccountingStore::open(state.db.clone()).map_err(|_| StatusCode::CONFLICT)?;
        if store.host_id().map_err(|_| StatusCode::CONFLICT)? == *participant_id {
            super::drain(&state.db, group_id, *generation)
                .await
                .map_err(|_| StatusCode::CONFLICT)?;
            return Ok(Json(AccountingReply::Saved));
        }
    }
    execute(&state.db, request)
        .map(Json)
        .map_err(|_| StatusCode::CONFLICT)
}
pub(super) fn execute(db: &HostDatabase, request: AccountingRequest) -> Result<AccountingReply> {
    let store = AccountingStore::open(db.clone())?;
    match request {
        AccountingRequest::Drain {
            group_id,
            generation,
            participant_id,
        } => store.drain(&group_id, generation, &participant_id)?,
        AccountingRequest::Resolve {
            provider,
            subscription,
            model,
        } => {
            let provider = match provider {
                1 => Provider::OpenAi,
                2 => Provider::Xai,
                3 => Provider::Anthropic,
                4 => Provider::Zai,
                5 => Provider::OpenRouter,
                6 => Provider::Custom,
                _ => anyhow::bail!("invalid provider"),
            };
            ensure!(model.len() <= 512, "invalid model");
            return Ok(AccountingReply::Binding(resolve(
                db,
                provider,
                if subscription {
                    AuthMode::Subscription
                } else {
                    AuthMode::ApiKey
                },
                &model,
            )?));
        }
        AccountingRequest::Bind { route, binding } => store.bind(&route, binding)?,
        AccountingRequest::Status { group_id } => {
            return Ok(AccountingReply::Status(Box::new(
                beale_storage::provider_accounting::bounded_snapshot(store.snapshot(&group_id)?),
            )));
        }
        AccountingRequest::Admit { request } => {
            return Ok(AccountingReply::Admission(store.admit(&request, now_ms())?));
        }
        AccountingRequest::Settle { request, charge } => store.settle(&request, &charge)?,
        AccountingRequest::Reconcile { request, charge } => {
            store.reconcile(&request, &charge, now_ms())?
        }
        AccountingRequest::Cancel { request } => store.cancel(&request)?,
        AccountingRequest::Observe {
            group_id,
            generation,
            observation,
        } => store.observe(&group_id, generation, observation, now_ms())?,
        AccountingRequest::Price { group_id, price } => store.price(&group_id, price)?,
        AccountingRequest::Freeze {
            group_id,
            generation,
        } => {
            return Ok(AccountingReply::Status(Box::new(
                store.freeze(&group_id, generation)?,
            )));
        }
        AccountingRequest::Import {
            snapshot,
            owner_host_id,
        } => store.import(*snapshot, &owner_host_id)?,
    }
    Ok(AccountingReply::Saved)
}
