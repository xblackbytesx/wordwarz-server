//! WordWarZ server library: the router and stores, split from `main` so
//! integration tests drive the real app.

pub mod api;
pub mod auth;
pub mod changes;
pub mod config;
pub mod error;
pub mod games;
pub mod lexicons;
pub mod push;
pub mod stats;
pub mod users;
pub mod web;
pub mod webpush;

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use sqlx::PgPool;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// `Secure` flag on the web session cookie (and https in links).
    pub cookie_secure: bool,
    /// `WORDWARZ_PUBLIC_URL`, the base of invite and device-link QR codes.
    pub public_url: Option<String>,
    /// UnifiedPush delivery (no redirects).
    pub http: reqwest::Client,
    /// `WORDWARZ_PUSH_HOSTS`; empty turns push off.
    pub push_hosts: Vec<String>,
    /// Tests only: allow plain-http push endpoints (a local stand-in).
    pub push_allow_http: bool,
    /// Long-poll counters, one per person.
    pub changes: Arc<changes::Changes>,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            cookie_secure: true,
            public_url: None,
            http: push::client(),
            push_hosts: Vec::new(),
            push_allow_http: false,
            changes: Arc::new(changes::Changes::default()),
        }
    }

    /// This server as devices and invitees reach it.
    pub fn public_base(&self, headers: &HeaderMap) -> String {
        self.public_url.clone().unwrap_or_else(|| web::base_url(headers, self.cookie_secure))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api/v1", api::routes())
        .merge(web::routes())
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz(State(st): State<AppState>) -> Result<&'static str, error::ApiError> {
    sqlx::query("SELECT 1").execute(&st.pool).await?;
    Ok("ok")
}
