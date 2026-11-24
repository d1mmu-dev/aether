//! slskd search (M7 Task 2, D-292), upstream `routes/soulseek.ts` at kima-hub `b927e30`:
//! `POST /soulseek/search` starts a background search and answers its id at once,
//! `GET /soulseek/search/{searchId}` reads the caller's results. Any signed-in caller, as
//! upstream (`mountAccess.ts`: `/api/soulseek` is signed-in; downloads are a later task). The
//! slskd address and key are admin settings and are never returned.

use aether_net::slskd::{MAX_QUERY_CHARS, SearchError, SlskdApiKey, SlskdUrl};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::soulseek::format_response;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/soulseek/search", post(start))
        .route("/soulseek/search/{search_id}", get(results))
}

const NOT_CONFIGURED: &str =
    "Soulseek credentials not configured. Add username/password in System Settings.";
const QUERY_REQUIRED: &str = "Either 'query' or both 'artist' and 'title' are required";

/// The search text: `query`, else `"<artist> <title>"` (upstream truthiness, strings only).
fn search_text(body: &[u8]) -> Result<String, ApiError> {
    let required = || ApiError::BadRequest(QUERY_REQUIRED.into());
    let value: Value = serde_json::from_slice(body).map_err(|_| required())?;
    let obj = value.as_object().ok_or_else(required)?;
    let text = |name: &str| {
        obj.get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let query = match (text("query"), text("artist"), text("title")) {
        (Some(q), _, _) => q.to_string(),
        (None, Some(artist), Some(title)) => format!("{artist} {title}"),
        _ => return Err(required()),
    };
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(ApiError::BadRequest("query is too long".into()));
    }
    Ok(query)
}

async fn start(
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    // One settings snapshot: the key is the one bound to this URL (D-291).
    let kv = state.settings.get().await?;
    let url = kv
        .soulseek_enabled
        .then(|| {
            kv.slskd_url
                .as_deref()
                .and_then(|u| SlskdUrl::parse(u).ok())
        })
        .flatten()
        .ok_or(ApiError::Forbidden(NOT_CONFIGURED))?;
    let key = kv.slskd_api_key.as_deref().and_then(SlskdApiKey::new);
    let query = search_text(&body)?;
    let permit = state.net.slskd_search_permit(&url).map_err(|e| match e {
        SearchError::Busy => {
            ApiError::TooManyRequests("Too many Soulseek searches, try again shortly")
        }
        _ => ApiError::Conflict("Outbound connections are turned off"),
    })?;

    let searches = state.soulseek.clone();
    let id = searches.open(user.profile);
    let net = state.net.clone();
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        let limits = searches.limits.clone();
        let sink = searches.clone();
        let search = net.search_slskd(&permit, &url, key.as_ref(), &query, &limits, |resp| {
            sink.append(id, format_response(resp))
        });
        // Shutdown drops the search; its guard still deletes the remote search.
        let end = tokio::select! {
            end = search => Some(end),
            () = shutdown.cancelled() => None,
        };
        // Outcome kind only: never the query, address, key or peer text.
        tracing::debug!(?end, "slskd search ended");
        searches.finish(id);
        drop(permit);
    });

    Ok(Json(json!({ "searchId": id, "message": "Search started" })).into_response())
}

async fn results(
    State(state): State<AppState>,
    user: AuthUser,
    Path(search_id): Path<String>,
) -> Response {
    let view = Uuid::parse_str(&search_id)
        .ok()
        .and_then(|id| state.soulseek.view(id, user.profile));
    match view {
        Some(view) => Json(json!({
            "results": view.results,
            "count": view.results.len(),
            "complete": view.complete,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Search not found or expired", "results": [], "count": 0 })),
        )
            .into_response(),
    }
}
