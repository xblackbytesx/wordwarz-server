//! `/api/v1`: what the app talks to. Bearer tokens (one per device); every
//! route acts as the token's owner. Joining with an invite, signing in and
//! claiming a device link are the only routes without a token. Versioned: a
//! breaking change gets a new prefix, never an edit here.

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wordwarz_core::game::Action;

use crate::AppState;
use crate::auth::{AuthUser, Role};
use crate::error::ApiError;
use crate::games::{self, ActResult, GameEnvelope, GameSummary, NewGame};
use crate::stats::{self, Scoreboard};
use crate::users::{self, AcceptError};
use crate::{push, web};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/join", post(join))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/link", post(create_link))
        .route("/link/claim", post(claim_link))
        .route("/me", get(me).patch(update_me).delete(delete_me))
        .route("/me/export", get(export))
        .route("/me/passphrase", put(set_passphrase))
        .route("/people", get(people))
        .route("/invites", post(invite))
        .route("/games", get(list_games).post(create_game))
        .route("/games/{id}", get(game))
        .route("/games/{id}/actions", post(action))
        .route("/games/{id}/nudge", post(nudge))
        .route("/scoreboard", get(scoreboard))
        .route("/changes", get(changes))
        .route("/push", put(put_push).delete(delete_push))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    id: Uuid,
    username: String,
    display_name: String,
    admin: bool,
    has_passphrase: bool,
}

async fn load_me(st: &AppState, id: Uuid) -> Result<Me, ApiError> {
    let (username, display_name, role, has_passphrase): (String, String, String, bool) =
        sqlx::query_as("SELECT username, display_name, role, password_hash IS NOT NULL FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&st.pool)
            .await?;
    Ok(Me { id, username, display_name, admin: role == "admin", has_passphrase })
}

/// A new device token and who it belongs to.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedIn {
    token: String,
    me: Me,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JoinRequest {
    invite: String,
    username: String,
    #[serde(default)]
    display_name: String,
    /// Optional: without one the account signs in by device only.
    #[serde(default)]
    passphrase: Option<String>,
    device: String,
}

/// Creates the account from an invite (the scanned QR code) and signs this
/// device in at once.
async fn join(State(st): State<AppState>, Json(r): Json<JoinRequest>) -> Result<Json<SignedIn>, ApiError> {
    users::clean_label(&r.device).map_err(|e| ApiError::bad(e.to_string()))?;
    let passphrase = r.passphrase.as_deref().filter(|p| !p.is_empty());
    let id = match users::accept_invite(&st.pool, &r.invite, &r.username, &r.display_name, passphrase).await {
        Ok(id) => id,
        Err(AcceptError::Input(m)) => return Err(ApiError::BadRequest(m)),
        Err(AcceptError::Invalid) => return Err(ApiError::Forbidden("this invite is used up or has expired".into())),
    };
    let token = users::new_token(&st.pool, id, &r.device).await?;
    Ok(Json(SignedIn { token, me: load_me(&st, id).await? }))
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    passphrase: String,
    device: String,
}

async fn login(State(st): State<AppState>, Json(r): Json<LoginRequest>) -> Result<Json<SignedIn>, ApiError> {
    users::clean_label(&r.device).map_err(|e| ApiError::bad(e.to_string()))?;
    let Some(id) = web::session::check_login(&st.pool, &r.username, &r.passphrase).await? else {
        // Same answer for unknown, disabled, passphrase-less and wrong.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        return Err(ApiError::Unauthorized);
    };
    let token = users::new_token(&st.pool, id, &r.device).await?;
    Ok(Json(SignedIn { token, me: load_me(&st, id).await? }))
}

/// Signs this device out: its token stops working.
async fn logout(State(st): State<AppState>, u: AuthUser) -> Result<Response, ApiError> {
    sqlx::query("UPDATE api_tokens SET revoked_at = now() WHERE id = $1").bind(u.token_id).execute(&st.pool).await?;
    push::unregister(&st, u.token_id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    /// What the QR code holds.
    url: String,
    expires_at: DateTime<Utc>,
}

/// A one-time QR code that signs another device in to this account.
async fn create_link(State(st): State<AppState>, u: AuthUser, headers: HeaderMap) -> Result<Json<Link>, ApiError> {
    let (code, expires_at) = users::create_link(&st.pool, u.id).await?;
    Ok(Json(Link { url: format!("{}/link/{code}", st.public_base(&headers)), expires_at }))
}

#[derive(Deserialize)]
struct ClaimRequest {
    code: String,
    device: String,
}

async fn claim_link(State(st): State<AppState>, Json(r): Json<ClaimRequest>) -> Result<Json<SignedIn>, ApiError> {
    let claimed = users::claim_link(&st.pool, &r.code, &r.device).await.map_err(|e| ApiError::bad(e.to_string()))?;
    let Some((id, token)) = claimed else {
        return Err(ApiError::Forbidden("this code is used up or has expired; make a new one".into()));
    };
    Ok(Json(SignedIn { token, me: load_me(&st, id).await? }))
}

async fn me(State(st): State<AppState>, u: AuthUser) -> Result<Json<Me>, ApiError> {
    Ok(Json(load_me(&st, u.id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeUpdate {
    display_name: String,
}

async fn update_me(State(st): State<AppState>, u: AuthUser, Json(r): Json<MeUpdate>) -> Result<Json<Me>, ApiError> {
    sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
        .bind(u.id)
        .bind(users::clean_display_name(&r.display_name))
        .execute(&st.pool)
        .await?;
    Ok(Json(load_me(&st, u.id).await?))
}

/// Deletes the account: resigns running games, keeps finished ones with the
/// seat shown as deleted.
async fn delete_me(State(st): State<AppState>, u: AuthUser) -> Result<Response, ApiError> {
    games::delete_account(&st, u.id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct PassphraseRequest {
    /// Required when the account already has one.
    #[serde(default)]
    current: Option<String>,
    passphrase: String,
}

/// Sets the passphrase (for the website and signing in elsewhere). An account
/// that joined by QR code has none yet; changing one needs the current one,
/// so a borrowed unlocked phone cannot take the account over.
async fn set_passphrase(
    State(st): State<AppState>,
    u: AuthUser,
    Json(r): Json<PassphraseRequest>,
) -> Result<Response, ApiError> {
    let (username, has): (String, bool) =
        sqlx::query_as("SELECT username, password_hash IS NOT NULL FROM users WHERE id = $1")
            .bind(u.id)
            .fetch_one(&st.pool)
            .await?;
    if has {
        let ok = match r.current.as_deref() {
            Some(c) => web::session::check_login(&st.pool, &username, c).await?.is_some(),
            None => false,
        };
        if !ok {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            return Err(ApiError::Forbidden("the current passphrase is wrong".into()));
        }
    }
    users::check_password(&r.passphrase).map_err(|e| ApiError::bad(e.to_string()))?;
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(u.id)
        .bind(web::session::hash_password(&r.passphrase))
        .execute(&st.pool)
        .await?;
    // Every browser signs in again; devices keep their own tokens.
    sqlx::query("DELETE FROM web_sessions WHERE user_id = $1").bind(u.id).execute(&st.pool).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub account: Me,
    pub created_at: DateTime<Utc>,
    pub devices: Vec<ExportDevice>,
    pub games: Vec<GameEnvelope>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ExportDevice {
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Everything the server holds about this person, as JSON.
pub async fn export_for(st: &AppState, id: Uuid) -> Result<Export, ApiError> {
    let created_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT created_at FROM users WHERE id = $1").bind(id).fetch_one(&st.pool).await?;
    let devices: Vec<ExportDevice> = sqlx::query_as(
        "SELECT label, created_at, last_used_at, revoked_at FROM api_tokens WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await?;
    let mut list = Vec::new();
    for g in games::list(&st.pool, id).await? {
        list.push(games::get(&st.pool, id, g.id).await?);
    }
    Ok(Export { account: load_me(st, id).await?, created_at, devices, games: list })
}

async fn export(State(st): State<AppState>, u: AuthUser) -> Result<Json<Export>, ApiError> {
    Ok(Json(export_for(&st, u.id).await?))
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Person {
    id: Uuid,
    username: String,
    display_name: String,
}

/// Everyone on this server, to pick opponents from.
async fn people(State(st): State<AppState>, _u: AuthUser) -> Result<Json<Vec<Person>>, ApiError> {
    let rows: Vec<Person> =
        sqlx::query_as("SELECT id, username, display_name FROM users WHERE disabled_at IS NULL ORDER BY username")
            .fetch_all(&st.pool)
            .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InviteRequest {
    #[serde(default)]
    note: String,
    /// The new account is an admin too.
    #[serde(default)]
    admin: bool,
    /// Shown on this screen for someone in the room: valid 10 minutes
    /// instead of 7 days.
    #[serde(default)]
    in_person: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteLink {
    url: String,
    expires_at: DateTime<Utc>,
}

/// Admins only (AGENTS.md decision 13).
async fn invite(
    State(st): State<AppState>,
    u: AuthUser,
    headers: HeaderMap,
    Json(r): Json<InviteRequest>,
) -> Result<Json<InviteLink>, ApiError> {
    if u.role != Role::Admin {
        return Err(ApiError::Forbidden("only an admin can invite people".into()));
    }
    let role = if r.admin { Role::Admin } else { Role::User };
    let kind = if r.in_person { users::InviteKind::InPerson } else { users::InviteKind::Link };
    let (token, expires_at) = users::create_invite(&st.pool, u.id, role, &r.note, kind).await?;
    Ok(Json(InviteLink { url: format!("{}/invite/{token}", st.public_base(&headers)), expires_at }))
}

async fn list_games(State(st): State<AppState>, u: AuthUser) -> Result<Json<Vec<GameSummary>>, ApiError> {
    Ok(Json(games::list(&st.pool, u.id).await?))
}

async fn create_game(
    State(st): State<AppState>,
    u: AuthUser,
    Json(r): Json<NewGame>,
) -> Result<(StatusCode, Json<GameEnvelope>), ApiError> {
    let id = games::create(&st, u.id, r).await?;
    Ok((StatusCode::CREATED, Json(games::get(&st.pool, u.id, id).await?)))
}

async fn game(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>) -> Result<Json<GameEnvelope>, ApiError> {
    Ok(Json(games::get(&st.pool, u.id, id).await?))
}

async fn action(
    State(st): State<AppState>,
    u: AuthUser,
    Path(id): Path<Uuid>,
    Json(a): Json<Action>,
) -> Result<Json<ActResult>, ApiError> {
    Ok(Json(games::act(&st, u.id, id, a).await?))
}

async fn nudge(State(st): State<AppState>, u: AuthUser, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    games::nudge(&st, u.id, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn scoreboard(State(st): State<AppState>, _u: AuthUser) -> Result<Json<Scoreboard>, ApiError> {
    Ok(Json(stats::scoreboard(&st.pool).await?))
}

#[derive(Deserialize)]
struct SinceQuery {
    since: Option<u64>,
}

#[derive(Serialize)]
struct ChangesAnswer {
    generation: u64,
    changed: bool,
}

/// Long-poll: answers when something changed for this person, or after ~25 s.
async fn changes(State(st): State<AppState>, u: AuthUser, Query(q): Query<SinceQuery>) -> Json<ChangesAnswer> {
    let (generation, changed) = st.changes.wait(u.id, q.since.unwrap_or(u64::MAX), crate::changes::WAIT).await;
    Json(ChangesAnswer { generation, changed })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushRequest {
    url: String,
    /// The connector's WebPush keys (base64url); pushes are encrypted to them.
    #[serde(default)]
    pub_key: Option<String>,
    #[serde(default)]
    auth: Option<String>,
}

/// This device's UnifiedPush endpoint (replaces an earlier one).
async fn put_push(State(st): State<AppState>, u: AuthUser, Json(r): Json<PushRequest>) -> Result<Response, ApiError> {
    let keys = match (r.pub_key.as_deref(), r.auth.as_deref()) {
        (Some(k), Some(a)) => Some((k, a)),
        (None, None) => None,
        _ => return Err(ApiError::bad("send both pubKey and auth, or neither")),
    };
    push::register(&st, u.token_id, &r.url, keys).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_push(State(st): State<AppState>, u: AuthUser) -> Result<Response, ApiError> {
    push::unregister(&st, u.token_id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
