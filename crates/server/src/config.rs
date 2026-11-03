use anyhow::{Result, bail};

pub struct Config {
    pub database_url: String,
    pub bind: String,
    /// `Secure` on the web session cookie. On by default; turn off only for a
    /// plain-http LAN setup, or browsers will not send the cookie back.
    pub cookie_secure: bool,
    /// This server as devices reach it (`https://wordwarz.example.com`): the
    /// base of invite and device-link QR codes. Default: the request's host.
    pub public_url: Option<String>,
    /// `WORDWARZ_PUSH_HOSTS`: the UnifiedPush servers devices may register
    /// endpoints on (e.g. `ntfy.example.com`). Declared, never inferred: the
    /// server POSTs to these URLs, so an open list would let any account make
    /// it call arbitrary hosts. Empty: push is off.
    pub push_hosts: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: var("DATABASE_URL").ok_or_else(|| anyhow::anyhow!("DATABASE_URL is required"))?,
            bind: var("WORDWARZ_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            cookie_secure: !matches!(var("WORDWARZ_COOKIE_SECURE").as_deref(), Some("false" | "0" | "no")),
            public_url: public_url(var("WORDWARZ_PUBLIC_URL").as_deref())?,
            push_hosts: push_hosts(var("WORDWARZ_PUSH_HOSTS").as_deref())?,
        })
    }
}

pub fn public_url(v: Option<&str>) -> Result<Option<String>> {
    let Some(v) = v else { return Ok(None) };
    if !(v.starts_with("https://") || v.starts_with("http://")) {
        bail!("WORDWARZ_PUBLIC_URL must start with https:// (or http:// on a LAN)");
    }
    Ok(Some(v.trim_end_matches('/').to_string()))
}

/// Comma-separated bare host names (an optional `:port` allowed).
pub fn push_hosts(v: Option<&str>) -> Result<Vec<String>> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let hosts: Vec<String> = v.split(',').map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()).collect();
    for h in &hosts {
        if h.contains('/') || h.contains('@') || h.contains(char::is_whitespace) {
            bail!("WORDWARZ_PUSH_HOSTS takes host names like ntfy.example.com, not URLs: {h}");
        }
    }
    Ok(hosts)
}

/// Trimmed; empty means unset.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_is_a_base_url() {
        assert_eq!(public_url(None).unwrap(), None);
        assert_eq!(public_url(Some("https://w.example.com/")).unwrap().as_deref(), Some("https://w.example.com"));
        assert!(public_url(Some("w.example.com")).is_err());
    }

    #[test]
    fn push_hosts_are_bare_names() {
        assert_eq!(push_hosts(None).unwrap(), Vec::<String>::new());
        assert_eq!(
            push_hosts(Some(" ntfy.example.com, Push.Example.org:8443 ")).unwrap(),
            vec!["ntfy.example.com".to_string(), "push.example.org:8443".to_string()]
        );
        assert!(push_hosts(Some("https://ntfy.example.com/")).is_err());
    }
}
