//! People: everyone on the server, and for admins invites, roles and
//! disabling. Admins manage accounts, never other people's games or devices.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use super::session::{AdminUser, WebUser};
use super::{Nav, html, local_day, local_time, qr_svg};
use crate::AppState;
use crate::auth::Role;
use crate::error::{ApiError, WebError};
use crate::users;

type Page = Result<Response, WebError>;

struct PersonRow {
    id: Uuid,
    username: String,
    name: String,
    admin: bool,
    created: String,
    disabled: bool,
    me: bool,
}

struct InviteRow {
    id: Uuid,
    role: String,
    note: String,
    expires: String,
}

#[derive(Template)]
#[template(path = "people.html")]
struct PeoplePage {
    nav: Nav,
    admin: bool,
    people: Vec<PersonRow>,
    invites: Vec<InviteRow>,
}

pub async fn page(State(st): State<AppState>, me: WebUser) -> Page {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, username, display_name, role, created_at, disabled_at FROM users
          WHERE disabled_at IS NULL OR $1 ORDER BY username",
    )
    .bind(me.is_admin())
    .fetch_all(&st.pool)
    .await?;
    let people = rows
        .into_iter()
        .map(|(id, username, name, role, created, disabled)| PersonRow {
            id,
            me: id == me.id,
            username,
            name,
            admin: role == "admin",
            created: local_day(created, me.tz),
            disabled: disabled.is_some(),
        })
        .collect();
    let inv: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, role, note, expires_at FROM invites
          WHERE used_at IS NULL AND expires_at > now() AND $1
          ORDER BY created_at DESC",
    )
    .bind(me.is_admin())
    .fetch_all(&st.pool)
    .await?;
    let invites = inv
        .into_iter()
        .map(|(id, role, note, expires)| InviteRow { id, role, note, expires: local_time(expires, me.tz) })
        .collect();
    html(PeoplePage { admin: me.is_admin(), nav: Nav::new(&me, "people"), people, invites })
}

#[derive(Template)]
#[template(path = "invite_created.html")]
struct InviteCreated {
    link: String,
    qr: String,
    expires: String,
    admin: bool,
}

#[derive(Deserialize)]
pub struct InviteForm {
    #[serde(default)]
    role: String,
    note: String,
    /// `scan` (10 minutes, for someone in the room) or `link` (7 days).
    #[serde(default)]
    validity: String,
}

/// HTMX: the link and its QR code replace the form, shown once. Admins only.
pub async fn create_invite(
    State(st): State<AppState>,
    AdminUser(me): AdminUser,
    headers: HeaderMap,
    Form(f): Form<InviteForm>,
) -> Page {
    let role = Role::parse(&f.role);
    let kind = if f.validity == "link" { users::InviteKind::Link } else { users::InviteKind::InPerson };
    let (token, expires) =
        users::create_invite(&st.pool, me.id, role, &f.note, kind).await.map_err(ApiError::Internal)?;
    let link = format!("{}/invite/{token}", st.public_base(&headers));
    html(InviteCreated { qr: qr_svg(&link), link, expires: local_time(expires, me.tz), admin: role == Role::Admin })
}

pub async fn delete_invite(State(st): State<AppState>, _a: AdminUser, Path(id): Path<Uuid>) -> Page {
    sqlx::query("DELETE FROM invites WHERE id = $1 AND used_at IS NULL").bind(id).execute(&st.pool).await?;
    Ok("".into_response())
}

#[derive(Deserialize)]
pub struct RoleForm {
    role: String,
}

pub async fn set_role(
    State(st): State<AppState>,
    AdminUser(me): AdminUser,
    Path(id): Path<Uuid>,
    Form(f): Form<RoleForm>,
) -> Page {
    let role = Role::parse(&f.role);
    if role == Role::User && id == me.id && users::other_admins(&st.pool, me.id).await? == 0 {
        return Err(ApiError::bad("you are the only admin; make someone else admin first").into());
    }
    sqlx::query("UPDATE users SET role = $2 WHERE id = $1").bind(id).bind(role.as_str()).execute(&st.pool).await?;
    Ok(Redirect::to("/people").into_response())
}

/// Disabling ends the person's web sessions and stops their device tokens
/// (both checks require `disabled_at IS NULL`). Their games stay; enabling
/// restores access.
pub async fn disable(State(st): State<AppState>, AdminUser(me): AdminUser, Path(id): Path<Uuid>) -> Page {
    if id == me.id {
        return Err(ApiError::bad("you cannot disable yourself").into());
    }
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1 AND disabled_at IS NULL")
        .bind(id)
        .execute(&st.pool)
        .await?;
    sqlx::query("DELETE FROM web_sessions WHERE user_id = $1").bind(id).execute(&st.pool).await?;
    Ok(Redirect::to("/people").into_response())
}

pub async fn enable(State(st): State<AppState>, _a: AdminUser, Path(id): Path<Uuid>) -> Page {
    sqlx::query("UPDATE users SET disabled_at = NULL WHERE id = $1").bind(id).execute(&st.pool).await?;
    Ok(Redirect::to("/people").into_response())
}
