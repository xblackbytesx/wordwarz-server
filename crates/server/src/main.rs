use anyhow::{Result, bail};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;
use wordwarz_server::auth::Role;
use wordwarz_server::config::Config;
use wordwarz_server::{AppState, MIGRATOR, games, lexicons, router, users};

const USAGE: &str = "usage:
  wordwarz-server [serve]
  wordwarz-server create-admin <username>   (then set its passphrase)
  wordwarz-server set-password <username>   (reads the passphrase from stdin)
  wordwarz-server invite <username> [note]  (a one-time invite link made by <username>)";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let cfg = Config::from_env()?;
    let pool = PgPoolOptions::new().max_connections(10).connect(&cfg.database_url).await?;
    MIGRATOR.run(&pool).await?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] | ["serve"] => {
            lexicons::warm();
            let state = AppState {
                cookie_secure: cfg.cookie_secure,
                public_url: cfg.public_url.clone(),
                push_hosts: cfg.push_hosts.clone(),
                ..AppState::new(pool)
            };
            if state.push_hosts.is_empty() {
                tracing::info!("push is off (WORDWARZ_PUSH_HOSTS unset); apps long-poll while open");
            }
            tokio::spawn(games::run_timers(state.clone()));
            let app = router(state);
            let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
            tracing::info!(bind = %cfg.bind, "listening");
            axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
        }
        ["create-admin", username] => {
            let id = users::create(&pool, username, Role::Admin).await?;
            println!("created admin {username} ({id}); now run set-password {username}");
        }
        ["set-password", username] => {
            // stdin, never argv: argv lands in shell history and `ps`.
            eprint!("New passphrase for {username}: ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            users::set_password(&pool, username, line.trim_end_matches(['\r', '\n'])).await?;
            println!("passphrase set; web sessions for {username} were ended");
        }
        ["invite", username, note @ ..] => {
            let by = users::id_by_username(&pool, username).await?;
            let (token, expires) =
                users::create_invite(&pool, by, Role::User, &note.join(" "), users::InviteKind::Link).await?;
            let base = cfg.public_url.as_deref().unwrap_or("https://<your host>");
            println!("{base}/invite/{token}\n(one use, until {expires})");
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
