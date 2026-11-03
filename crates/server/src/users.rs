//! Accounts, invites, device links and API tokens. An account is a username;
//! everything else is optional (AGENTS.md decision 12).

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{self, INVITE_PREFIX, LINK_PREFIX, Role};

pub const MIN_PASSWORD: usize = 10;
/// How long an invite works. A code shown on a screen for someone in the room
/// lives minutes, so a photo of it is useless soon after; a link sent by
/// message lives days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteKind {
    InPerson,
    Link,
}

impl InviteKind {
    pub fn lifetime(self) -> Duration {
        match self {
            InviteKind::InPerson => Duration::minutes(IN_PERSON_MINUTES),
            InviteKind::Link => Duration::days(INVITE_DAYS),
        }
    }
}

pub const INVITE_DAYS: i64 = 7;
pub const IN_PERSON_MINUTES: i64 = 10;
pub const LINK_MINUTES: i64 = 10;

/// Lowercase a-z, 0-9 and `_`, 3 to 20 characters, starting with a letter.
pub fn normalize_username(name: &str) -> Result<String> {
    let u = name.trim().to_lowercase();
    let ok = (3..=20).contains(&u.len())
        && u.starts_with(|c: char| c.is_ascii_lowercase())
        && u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        bail!("a username is 3 to 20 letters, digits or _, starting with a letter");
    }
    Ok(u)
}

/// Optional, shown instead of the username; control characters dropped.
pub fn clean_display_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(30).collect()
}

pub fn check_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD {
        bail!("use at least {MIN_PASSWORD} characters");
    }
    if password.len() > 1024 {
        bail!("that passphrase is too long");
    }
    Ok(())
}

pub async fn create(pool: &PgPool, username: &str, role: Role) -> Result<Uuid> {
    let username = normalize_username(username)?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, role) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(&username)
        .bind(role.as_str())
        .execute(pool)
        .await
        .context("create user (is the username taken?)")?;
    Ok(id)
}

/// Sets (or replaces) the passphrase. Ends every web session.
pub async fn set_password(pool: &PgPool, username: &str, password: &str) -> Result<()> {
    check_password(password)?;
    let hash = crate::web::session::hash_password(password);
    let id: Option<Uuid> = sqlx::query_scalar("UPDATE users SET password_hash = $2 WHERE username = $1 RETURNING id")
        .bind(username.trim().to_lowercase())
        .bind(hash)
        .fetch_optional(pool)
        .await?;
    let Some(id) = id else { bail!("no user {username}") };
    sqlx::query("DELETE FROM web_sessions WHERE user_id = $1").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn id_by_username(pool: &PgPool, username: &str) -> Result<Uuid> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(username.trim().to_lowercase())
        .fetch_optional(pool)
        .await?;
    id.with_context(|| format!("no user {username}"))
}

/// The device label: what the person sees under Devices.
pub fn clean_label(label: &str) -> Result<String> {
    let label: String = label.trim().chars().filter(|c| !c.is_control()).collect();
    if label.is_empty() || label.chars().count() > 60 {
        bail!("give the device a name of at most 60 characters");
    }
    Ok(label)
}

/// Mints a device token and returns the plaintext (shown once).
pub async fn new_token(pool: &PgPool, user_id: Uuid, label: &str) -> Result<String> {
    let label = clean_label(label)?;
    let token = auth::new_token();
    sqlx::query("INSERT INTO api_tokens (id, user_id, label, token_sha256) VALUES ($1, $2, $3, $4)")
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(label)
        .bind(auth::token_hash(&token))
        .execute(pool)
        .await?;
    Ok(token)
}

/// Creates a one-time invite; returns the plaintext token for the link.
/// Only admins invite (AGENTS.md decision 13); callers check the role.
pub async fn create_invite(
    pool: &PgPool,
    by: Uuid,
    role: Role,
    note: &str,
    kind: InviteKind,
) -> Result<(String, DateTime<Utc>)> {
    let token = auth::new_secret(INVITE_PREFIX);
    let expires = Utc::now() + kind.lifetime();
    sqlx::query(
        "INSERT INTO invites (id, token_sha256, role, note, created_by, expires_at) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(auth::token_hash(&token))
    .bind(role.as_str())
    .bind(note.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>())
    .bind(by)
    .bind(expires)
    .execute(pool)
    .await?;
    Ok((token, expires))
}

/// The role an invite grants, when it is still usable.
pub async fn invite_role(pool: &PgPool, token: &str) -> Result<Option<Role>> {
    if !token.starts_with(INVITE_PREFIX) {
        return Ok(None);
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM invites WHERE token_sha256 = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(auth::token_hash(token))
    .fetch_optional(pool)
    .await?;
    Ok(role.map(|r| Role::parse(&r)))
}

#[derive(Debug)]
pub enum AcceptError {
    Invalid,
    Input(String),
}

/// Redeems an invite: claims it and creates the account in one transaction,
/// so two people racing on one link cannot both get in. The passphrase is
/// optional (the app's QR join needs none; the website asks for one).
pub async fn accept_invite(
    pool: &PgPool,
    token: &str,
    username: &str,
    display_name: &str,
    password: Option<&str>,
) -> Result<Uuid, AcceptError> {
    let username = normalize_username(username).map_err(|e| AcceptError::Input(e.to_string()))?;
    if let Some(p) = password {
        check_password(p).map_err(|e| AcceptError::Input(e.to_string()))?;
    }
    let hash = password.map(crate::web::session::hash_password);
    let mut tx = pool.begin().await.map_err(|_| AcceptError::Invalid)?;
    let invite: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE invites SET used_at = now()
          WHERE token_sha256 = $1 AND used_at IS NULL AND expires_at > now()
          RETURNING id, role",
    )
    .bind(auth::token_hash(token))
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AcceptError::Invalid)?;
    let Some((invite_id, role)) = invite else { return Err(AcceptError::Invalid) };
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (username) DO NOTHING",
    )
    .bind(id)
    .bind(&username)
    .bind(clean_display_name(display_name))
    .bind(&role)
    .bind(hash)
    .execute(&mut *tx)
    .await
    .map_err(|_| AcceptError::Invalid)?;
    if inserted.rows_affected() == 0 {
        // Rolls back, so the invite stays usable with another name.
        return Err(AcceptError::Input("that username is taken".into()));
    }
    sqlx::query("UPDATE invites SET used_by = $2 WHERE id = $1")
        .bind(invite_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AcceptError::Invalid)?;
    tx.commit().await.map_err(|_| AcceptError::Invalid)?;
    Ok(id)
}

/// A one-time code that signs another device in to `user` (shown as a QR
/// code). `by` made it: the person themselves, or an admin restoring the
/// account onto a new phone.
pub async fn create_link(pool: &PgPool, user: Uuid, by: Uuid) -> Result<(String, DateTime<Utc>)> {
    let code = auth::new_secret(LINK_PREFIX);
    let expires = Utc::now() + Duration::minutes(LINK_MINUTES);
    sqlx::query("DELETE FROM device_links WHERE expires_at < now()").execute(pool).await?;
    sqlx::query(
        "INSERT INTO device_links (id, code_sha256, user_id, expires_at, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(auth::token_hash(&code))
    .bind(user)
    .bind(expires)
    .bind(by)
    .execute(pool)
    .await?;
    Ok((code, expires))
}

/// Redeems a device link: the owner, and a new token for this device. None
/// when the code is unknown, used or expired.
pub async fn claim_link(pool: &PgPool, code: &str, device: &str) -> Result<Option<(Uuid, String)>> {
    let label = clean_label(device)?;
    if !code.starts_with(LINK_PREFIX) {
        return Ok(None);
    }
    let user: Option<Uuid> = sqlx::query_scalar(
        "UPDATE device_links l SET used_at = now() FROM users u
          WHERE l.code_sha256 = $1 AND l.used_at IS NULL AND l.expires_at > now()
            AND u.id = l.user_id AND u.disabled_at IS NULL
          RETURNING l.user_id",
    )
    .bind(auth::token_hash(code))
    .fetch_optional(pool)
    .await?;
    let Some(user) = user else { return Ok(None) };
    Ok(Some((user, new_token(pool, user, &label).await?)))
}

/// Signs `user` out everywhere: every device token is revoked and its push
/// endpoint dropped. For a lost or stolen phone. Returns how many devices.
pub async fn revoke_devices(pool: &PgPool, user: Uuid) -> Result<u64, sqlx::Error> {
    sqlx::query("DELETE FROM push_endpoints e USING api_tokens t WHERE t.id = e.token_id AND t.user_id = $1")
        .bind(user)
        .execute(pool)
        .await?;
    let n = sqlx::query("UPDATE api_tokens SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
        .bind(user)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n)
}

/// Active admins other than `except`. Guards against locking everyone out.
pub async fn other_admins(pool: &PgPool, except: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL AND id <> $1")
        .bind(except)
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_normalised_and_checked() {
        assert_eq!(normalize_username("  Mila_2016 ").unwrap(), "mila_2016");
        for bad in ["me", "2fast", "a b c", "émile", "x".repeat(21).as_str(), "dad!"] {
            assert!(normalize_username(bad).is_err(), "{bad}");
        }
        assert_eq!(clean_display_name(" Papa\u{7}\n "), "Papa");
    }
}
