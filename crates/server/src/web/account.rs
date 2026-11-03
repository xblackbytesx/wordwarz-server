//! Login, joining with an invite on the website, device-link landing page,
//! and the person's own settings: profile, passphrase, export, deletion.

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, SET_COOKIE};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::session::{self, WebUser};
use super::{Nav, html, qr_svg};
use crate::AppState;
use crate::error::{ApiError, WebError};
use crate::users::{self, AcceptError};

type Page = Result<Response, WebError>;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    username: String,
    error: Option<&'static str>,
}

pub async fn login_page() -> Page {
    html(LoginPage { username: String::new(), error: None })
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    passphrase: String,
}

pub async fn login(State(st): State<AppState>, Form(f): Form<LoginForm>) -> Page {
    match session::check_login(&st.pool, &f.username, &f.passphrase).await? {
        Some(user) => {
            let cookie = session::create(&st.pool, user, st.cookie_secure).await?;
            Ok(([(SET_COOKIE, cookie)], Redirect::to("/")).into_response())
        }
        None => {
            // Same answer for unknown name, no passphrase set, wrong passphrase.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let mut r = html(LoginPage { username: f.username, error: Some("Wrong username or passphrase.") })?;
            *r.status_mut() = StatusCode::UNAUTHORIZED;
            Ok(r)
        }
    }
}

pub async fn logout(State(st): State<AppState>, user: WebUser) -> Page {
    session::destroy(&st.pool, &user.token).await?;
    Ok(([(SET_COOKIE, session::cookie("", 0, st.cookie_secure))], Redirect::to("/login")).into_response())
}

#[derive(Template)]
#[template(path = "invite.html")]
struct InvitePage {
    token: String,
    valid: bool,
    admin: bool,
    /// The page's own address as a QR code, for scanning with the app.
    qr: String,
    username: String,
    name: String,
    error: Option<String>,
}

pub async fn invite_page(
    State(st): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(token): Path<String>,
) -> Page {
    let role = users::invite_role(&st.pool, &token).await.map_err(ApiError::Internal)?;
    let qr =
        if role.is_some() { qr_svg(&format!("{}/invite/{token}", st.public_base(&headers))) } else { String::new() };
    let mut r = html(InvitePage {
        valid: role.is_some(),
        admin: role == Some(crate::auth::Role::Admin),
        qr,
        token,
        username: String::new(),
        name: String::new(),
        error: None,
    })?;
    if role.is_none() {
        *r.status_mut() = StatusCode::NOT_FOUND;
    }
    Ok(r)
}

#[derive(Deserialize)]
pub struct InviteForm {
    username: String,
    name: String,
    passphrase: String,
    passphrase2: String,
}

/// On the website a passphrase is required (it is how the website logs in);
/// joining in the app needs none.
pub async fn accept_invite(State(st): State<AppState>, Path(token): Path<String>, Form(f): Form<InviteForm>) -> Page {
    let again = |error: String, valid: bool| InvitePage {
        token: token.clone(),
        valid,
        admin: false,
        qr: String::new(),
        username: f.username.clone(),
        name: f.name.clone(),
        error: Some(error),
    };
    if f.passphrase != f.passphrase2 {
        return html(again("The passphrases do not match.".into(), true));
    }
    match users::accept_invite(&st.pool, &token, &f.username, &f.name, Some(&f.passphrase)).await {
        Ok(id) => {
            let cookie = session::create(&st.pool, id, st.cookie_secure).await?;
            Ok(([(SET_COOKIE, cookie)], Redirect::to("/")).into_response())
        }
        Err(AcceptError::Input(m)) => html(again(m, true)),
        Err(AcceptError::Invalid) => html(again("This invite link is used up or has expired.".into(), false)),
    }
}

#[derive(Template)]
#[template(path = "link.html")]
struct LinkPage {}

/// A device-link QR code opened in a browser: it only works in the app.
pub async fn link_page(Path(_code): Path<String>) -> Page {
    html(LinkPage {})
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    nav: Nav,
    username: String,
    name: String,
    tz: String,
    zones: Vec<&'static str>,
    notice: Option<&'static str>,
}

#[derive(Deserialize)]
pub struct Notice {
    saved: Option<String>,
}

pub async fn settings(user: WebUser, Query(q): Query<Notice>) -> Page {
    let notice = match q.saved.as_deref() {
        Some("profile") => Some("Profile saved."),
        Some("passphrase") => Some("Passphrase changed. Other browsers were logged out."),
        _ => None,
    };
    html(SettingsPage {
        nav: Nav::new(&user, "settings"),
        username: user.username.clone(),
        name: user.name.clone(),
        tz: user.tz.name().to_string(),
        zones: chrono_tz::TZ_VARIANTS.iter().map(|z| z.name()).collect(),
        notice,
    })
}

#[derive(Deserialize)]
pub struct ProfileForm {
    name: String,
    tz: String,
}

pub async fn save_profile(State(st): State<AppState>, user: WebUser, Form(f): Form<ProfileForm>) -> Page {
    if f.tz.parse::<chrono_tz::Tz>().is_err() {
        return Err(ApiError::bad("unknown time zone").into());
    }
    sqlx::query("UPDATE users SET display_name = $2, tz = $3 WHERE id = $1")
        .bind(user.id)
        .bind(users::clean_display_name(&f.name))
        .bind(&f.tz)
        .execute(&st.pool)
        .await?;
    Ok(Redirect::to("/settings?saved=profile").into_response())
}

#[derive(Deserialize)]
pub struct PasswordForm {
    current: String,
    passphrase: String,
    passphrase2: String,
}

/// Needs the current passphrase: a borrowed, unlocked browser must not be
/// enough to take the account over. Ends every other session.
pub async fn change_password(State(st): State<AppState>, user: WebUser, Form(f): Form<PasswordForm>) -> Page {
    if session::check_login(&st.pool, &user.username, &f.current).await?.is_none() {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        return Err(ApiError::bad("the current passphrase is wrong").into());
    }
    if f.passphrase != f.passphrase2 {
        return Err(ApiError::bad("the new passphrases do not match").into());
    }
    users::check_password(&f.passphrase).map_err(|e| ApiError::bad(e.to_string()))?;
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(user.id)
        .bind(session::hash_password(&f.passphrase))
        .execute(&st.pool)
        .await?;
    sqlx::query("DELETE FROM web_sessions WHERE user_id = $1 AND token_sha256 <> $2")
        .bind(user.id)
        .bind(crate::auth::token_hash(&user.token))
        .execute(&st.pool)
        .await?;
    Ok(Redirect::to("/settings?saved=passphrase").into_response())
}

/// Everything about this person as a JSON download (same as `/api/v1/me/export`).
pub async fn export(State(st): State<AppState>, user: WebUser) -> Page {
    let data = crate::api::export_for(&st, user.id).await?;
    let body = serde_json::to_vec_pretty(&data).map_err(|e| ApiError::Internal(e.into()))?;
    let disposition = format!("attachment; filename=\"wordwarz-{}.json\"", user.username);
    Ok(([(CONTENT_TYPE, "application/json".to_string()), (CONTENT_DISPOSITION, disposition)], body).into_response())
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: String,
}

/// Typing the username confirms it; running games are resigned first.
pub async fn delete(State(st): State<AppState>, user: WebUser, Form(f): Form<DeleteForm>) -> Page {
    if f.confirm.trim().to_lowercase() != user.username {
        return Err(ApiError::bad("type your username to confirm").into());
    }
    crate::games::delete_account(&st, user.id).await?;
    Ok(([(SET_COOKIE, session::cookie("", 0, st.cookie_secure))], Redirect::to("/login")).into_response())
}
