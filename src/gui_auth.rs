use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use chatcmd_storage::SqliteRepository;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

const PASSWORD_SETTING_KEY: &str = "gui_password_hash_v1";
pub(crate) const SESSION_COOKIE: &str = "chatcmd_gui_session";
pub(crate) const SESSION_IDLE_SECONDS: u64 = 30 * 60;
const MIN_PASSWORD_CHARS: usize = 8;
const MAX_PASSWORD_CHARS: usize = 256;

#[derive(Debug, Clone)]
struct SessionEntry {
    last_seen: Instant,
}

#[derive(Clone)]
pub(crate) struct GuiAuth {
    repository: SqliteRepository,
    sessions: Arc<RwLock<HashMap<String, SessionEntry>>>,
    password_write: Arc<Mutex<()>>,
}

impl GuiAuth {
    pub(crate) fn new(repository: SqliteRepository) -> Self {
        Self {
            repository,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            password_write: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) async fn has_password(&self) -> Result<bool> {
        Ok(self.password_hash().await?.is_some())
    }

    pub(crate) async fn setup_password(&self, password: String) -> Result<String> {
        validate_new_password(&password)?;
        let _guard = self.password_write.lock().await;
        if self.password_hash().await?.is_some() {
            bail!("a GUI password is already configured");
        }
        let hash = hash_password(password).await?;
        self.store_password_hash(&hash).await?;
        self.create_session().await
    }

    pub(crate) async fn login(&self, password: String) -> Result<Option<String>> {
        let Some(hash) = self.password_hash().await? else {
            bail!("GUI password is not configured");
        };
        if !verify_password(password, hash).await? {
            return Ok(None);
        }
        Ok(Some(self.create_session().await?))
    }

    pub(crate) async fn change_password(
        &self,
        current: String,
        new_password: String,
    ) -> Result<Option<String>> {
        validate_new_password(&new_password)?;
        let _guard = self.password_write.lock().await;
        let Some(hash) = self.password_hash().await? else {
            bail!("GUI password is not configured");
        };
        if !verify_password(current, hash).await? {
            return Ok(None);
        }
        let hash = hash_password(new_password).await?;
        self.store_password_hash(&hash).await?;
        self.sessions.write().await.clear();
        Ok(Some(self.create_session().await?))
    }

    pub(crate) async fn authenticate_cookie(&self, cookie_header: Option<&str>) -> bool {
        let Some(token) = cookie_value(cookie_header, SESSION_COOKIE) else {
            return false;
        };
        self.touch_session(token).await
    }

    pub(crate) async fn logout_cookie(&self, cookie_header: Option<&str>) {
        if let Some(token) = cookie_value(cookie_header, SESSION_COOKIE) {
            self.sessions.write().await.remove(token);
        }
    }

    async fn create_session(&self) -> Result<String> {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let now = Instant::now();
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| now.duration_since(session.last_seen) < session_idle());
        if sessions.len() >= 256 {
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(token, _)| token.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(token.clone(), SessionEntry { last_seen: now });
        Ok(token)
    }

    async fn touch_session(&self, token: &str) -> bool {
        let now = Instant::now();
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| now.duration_since(session.last_seen) < session_idle());
        let Some(session) = sessions.get_mut(token) else {
            return false;
        };
        session.last_seen = now;
        true
    }

    async fn password_hash(&self) -> Result<Option<String>> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT value_json FROM settings WHERE key=?")
                .bind(PASSWORD_SETTING_KEY)
                .fetch_optional(self.repository.pool())
                .await
                .context("load GUI password hash")?;
        value
            .map(|json| serde_json::from_str::<String>(&json).context("decode GUI password hash"))
            .transpose()
    }

    async fn store_password_hash(&self, hash: &str) -> Result<()> {
        let value_json = serde_json::to_string(hash).context("encode GUI password hash")?;
        let updated_at_ms = crate::api::now_ms();
        sqlx::query("INSERT INTO settings(key,value_json,updated_at_ms) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at_ms=excluded.updated_at_ms")
            .bind(PASSWORD_SETTING_KEY)
            .bind(value_json)
            .bind(updated_at_ms)
            .execute(self.repository.pool())
            .await
            .context("store GUI password hash")?;
        Ok(())
    }
}

fn validate_new_password(password: &str) -> Result<()> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_CHARS {
        bail!("password must contain at least {MIN_PASSWORD_CHARS} characters");
    }
    if length > MAX_PASSWORD_CHARS {
        bail!("password must contain at most {MAX_PASSWORD_CHARS} characters");
    }
    Ok(())
}

async fn hash_password(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| anyhow::anyhow!("hash GUI password: {error}"))
    })
    .await
    .context("join GUI password hashing task")?
}

async fn verify_password(password: String, hash: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash)
            .map_err(|error| anyhow::anyhow!("parse GUI password hash: {error}"))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .context("join GUI password verification task")?
}

pub(crate) fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_IDLE_SECONDS}"
    )
}

pub(crate) fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

fn session_idle() -> Duration {
    Duration::from_secs(SESSION_IDLE_SECONDS)
}

fn cookie_value<'a>(header: Option<&'a str>, name: &str) -> Option<&'a str> {
    header?.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name && !value.is_empty()).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_cookie() {
        assert_eq!(
            cookie_value(Some("a=1; chatcmd_gui_session=token; b=2"), SESSION_COOKIE),
            Some("token")
        );
        assert_eq!(cookie_value(Some("a=1"), SESSION_COOKIE), None);
    }

    #[test]
    fn rejects_short_passwords() {
        assert!(validate_new_password("1234567").is_err());
        assert!(validate_new_password("12345678").is_ok());
    }
}
