//! UnifiedPush (AGENTS.md: never FCM). Each device registers the endpoint its
//! distributor (ntfy) gave it, with its WebPush keys; the server POSTs a tiny
//! JSON message there, encrypted to those keys (`webpush`), so the push
//! server relays ciphertext. The message names only an event and a game id:
//! no names, words, scores or racks. The app fetches the rest itself.

use std::time::Duration;

use serde::Serialize;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::webpush::{self, DeviceKeys};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Event {
    /// It is now your turn.
    Turn,
    /// Someone nudged you.
    Nudge,
    /// You were put in a new game.
    NewGame,
    /// A game you play in ended.
    Finished,
}

#[derive(Serialize)]
struct Message {
    event: Event,
    game: Uuid,
}

pub fn client() -> reqwest::Client {
    // reqwest is built without a default crypto provider (aws-lc needs cmake
    // everywhere); ring is installed once, here, before any client exists.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        // The endpoint was checked against WORDWARZ_PUSH_HOSTS; a redirect
        // could lead anywhere.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
}

/// Checks an endpoint against the declared push hosts: https (http only
/// when the server allows it, for tests), a listed host, no credentials.
pub fn check_endpoint(st: &AppState, url: &str) -> Result<String, ApiError> {
    if st.push_hosts.is_empty() {
        return Err(ApiError::bad("push is not set up on this server (WORDWARZ_PUSH_HOSTS)"));
    }
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| if st.push_allow_http { url.strip_prefix("http://") } else { None })
        .ok_or_else(|| ApiError::bad("a push endpoint is an https:// URL"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_lowercase();
    if url.len() > 1000 || host.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::bad("that is not a push endpoint"));
    }
    if !st.push_hosts.contains(&host) {
        return Err(ApiError::bad(format!("this server only sends pushes to: {}", st.push_hosts.join(", "))));
    }
    Ok(url.to_string())
}

/// Stores this device's endpoint. `keys` are the connector's WebPush public
/// key and auth secret (base64url); checked here so a bad pair fails now,
/// not silently at every push.
pub async fn register(st: &AppState, token_id: Uuid, url: &str, keys: Option<(&str, &str)>) -> Result<(), ApiError> {
    let url = check_endpoint(st, url)?;
    if let Some((k, a)) = keys {
        DeviceKeys::parse(k, a).map_err(|e| ApiError::bad(e.to_string()))?;
    }
    sqlx::query(
        "INSERT INTO push_endpoints (token_id, url, pub_key, auth) VALUES ($1, $2, $3, $4)
         ON CONFLICT (token_id) DO UPDATE SET url = EXCLUDED.url, pub_key = EXCLUDED.pub_key,
                                              auth = EXCLUDED.auth, created_at = now()",
    )
    .bind(token_id)
    .bind(url)
    .bind(keys.map(|k| k.0))
    .bind(keys.map(|k| k.1))
    .execute(&st.pool)
    .await?;
    Ok(())
}

pub async fn unregister(st: &AppState, token_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM push_endpoints WHERE token_id = $1").bind(token_id).execute(&st.pool).await?;
    Ok(())
}

/// Sends `event` to every device of `user`, in the background. Failures are
/// logged without the URL (it is a capability); an endpoint the push server
/// says is gone (404, 410) is dropped.
pub async fn notify(st: &AppState, user: Uuid, event: Event, game: Uuid) {
    #[allow(clippy::type_complexity)]
    let endpoints: Vec<(Uuid, String, Option<String>, Option<String>)> = match sqlx::query_as(
        "SELECT e.token_id, e.url, e.pub_key, e.auth FROM push_endpoints e JOIN api_tokens t ON t.id = e.token_id
          WHERE t.user_id = $1 AND t.revoked_at IS NULL",
    )
    .bind(user)
    .fetch_all(&st.pool)
    .await
    {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "push endpoints");
            return;
        }
    };
    for (token_id, url, pub_key, auth) in endpoints {
        // Re-checked at send time: the allowed hosts may have changed since.
        if check_endpoint(st, &url).is_err() {
            continue;
        }
        let http = st.http.clone();
        let pool = st.pool.clone();
        let plain = serde_json::to_vec(&Message { event, game }).expect("serializable");
        let keys = match (pub_key.as_deref(), auth.as_deref()) {
            (Some(k), Some(a)) => DeviceKeys::parse(k, a).ok(),
            _ => None,
        };
        tokio::spawn(async move {
            let req = http.post(&url).header("ttl", "86400").header("urgency", "normal");
            let req = match keys {
                Some(k) => req
                    .header("content-encoding", "aes128gcm")
                    .header("content-type", "application/octet-stream")
                    .body(webpush::encrypt(&k, &plain)),
                None => req.header("content-type", "application/json").body(plain),
            };
            let sent = req.send().await;
            match sent {
                Ok(r) if r.status().is_success() => {}
                Ok(r) if matches!(r.status().as_u16(), 404 | 410) => {
                    let _ = sqlx::query("DELETE FROM push_endpoints WHERE token_id = $1 AND url = $2")
                        .bind(token_id)
                        .bind(&url)
                        .execute(&pool)
                        .await;
                }
                Ok(r) => tracing::warn!(status = %r.status(), "push refused"),
                Err(e) => tracing::warn!(error = %e.without_url(), "push failed"),
            }
        });
    }
}
