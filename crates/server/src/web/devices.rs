//! Devices: the tokens a person's devices use, and a QR code that signs a
//! new device in (the app scans it).

use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::session::WebUser;
use super::{Nav, html, is_htmx, local_day, local_time, qr_svg};
use crate::AppState;
use crate::error::{ApiError, WebError};
use crate::users;

type Page = Result<Response, WebError>;

pub struct TokenRow {
    pub id: Uuid,
    pub label: String,
    pub created: String,
    pub last_used: String,
    pub push: bool,
}

#[derive(Template)]
#[template(path = "devices.html")]
struct DevicesPage {
    nav: Nav,
    tokens: Vec<TokenRow>,
}

pub async fn page(State(st): State<AppState>, user: WebUser) -> Page {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, DateTime<Utc>, Option<DateTime<Utc>>, bool)> = sqlx::query_as(
        "SELECT t.id, t.label, t.created_at, t.last_used_at, e.token_id IS NOT NULL FROM api_tokens t
           LEFT JOIN push_endpoints e ON e.token_id = t.id
          WHERE t.user_id = $1 AND t.revoked_at IS NULL ORDER BY t.created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&st.pool)
    .await?;
    let tokens = rows
        .into_iter()
        .map(|(id, label, created, last_used, push)| TokenRow {
            id,
            label,
            created: local_day(created, user.tz),
            last_used: last_used.map(|t| local_day(t, user.tz)).unwrap_or_else(|| "never".into()),
            push,
        })
        .collect();
    html(DevicesPage { nav: Nav::new(&user, "devices"), tokens })
}

#[derive(Template)]
#[template(path = "link_created.html")]
struct LinkCreated {
    qr: String,
    expires: String,
}

/// HTMX: a one-time QR code (10 minutes) that signs a device in to this account.
pub async fn create_link(State(st): State<AppState>, user: WebUser, headers: HeaderMap) -> Page {
    let (code, expires) = users::create_link(&st.pool, user.id).await.map_err(ApiError::Internal)?;
    let url = format!("{}/link/{code}", st.public_base(&headers));
    html(LinkCreated { qr: qr_svg(&url), expires: local_time(expires, user.tz) })
}

/// The device is signed out at once.
pub async fn revoke(State(st): State<AppState>, user: WebUser, Path(id): Path<Uuid>, headers: HeaderMap) -> Page {
    sqlx::query("UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
        .bind(id)
        .bind(user.id)
        .execute(&st.pool)
        .await?;
    sqlx::query("DELETE FROM push_endpoints WHERE token_id = $1").bind(id).execute(&st.pool).await?;
    if is_htmx(&headers) {
        return Ok("".into_response());
    }
    Ok(Redirect::to("/devices").into_response())
}
