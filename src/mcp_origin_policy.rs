use chatcmd_mcp::OriginPolicy;
use chatcmd_runtime::{BoxFuture, RuntimeError, RuntimeResult};
use chatcmd_storage::SqliteRepository;

pub(crate) struct ConfiguredOrigins {
    repository: SqliteRepository,
    port: u16,
    allow_missing: bool,
}

impl ConfiguredOrigins {
    pub(crate) fn new(repository: SqliteRepository, port: u16, allow_missing: bool) -> Self {
        Self {
            repository,
            port,
            allow_missing,
        }
    }

    fn is_local_origin(&self, origin: &str) -> bool {
        [
            format!("http://localhost:{}", self.port),
            format!("http://127.0.0.1:{}", self.port),
            format!("https://localhost:{}", self.port),
            format!("https://127.0.0.1:{}", self.port),
        ]
        .iter()
        .any(|candidate| candidate == origin)
    }

    async fn is_configured_public_origin(&self, origin: &str) -> RuntimeResult<bool> {
        let configured =
            sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM tunnels WHERE base_url=?)")
                .bind(origin.trim_end_matches('/'))
                .fetch_one(self.repository.pool())
                .await
                .map_err(|_| {
                    RuntimeError::new(
                        "origin_validation_failed",
                        "could not validate MCP request origin",
                    )
                })?;
        Ok(configured != 0)
    }
}

impl OriginPolicy for ConfiguredOrigins {
    fn authorize<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, RuntimeResult<()>> {
        Box::pin(async move {
            let allowed = if origin.is_empty() {
                self.allow_missing
            } else if self.is_local_origin(origin) {
                true
            } else {
                self.is_configured_public_origin(origin).await?
            };

            if allowed {
                Ok(())
            } else {
                Err(RuntimeError::new("origin_denied", "origin is not allowed"))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn policy(allow_missing: bool) -> (TempDir, ConfiguredOrigins) {
        let directory = tempfile::tempdir().expect("temp directory");
        let database = directory.path().join("origin-policy.db");
        let (repository, _) = SqliteRepository::open(&database, 1)
            .await
            .expect("open repository");
        (
            directory,
            ConfiguredOrigins::new(repository, 8080, allow_missing),
        )
    }

    #[tokio::test]
    async fn missing_origin_only_allowed_for_loopback_listener() {
        let (_directory, loopback) = policy(true).await;
        assert!(loopback.authorize("").await.is_ok());

        let (_directory, remote) = policy(false).await;
        assert_eq!(
            remote
                .authorize("")
                .await
                .expect_err("missing origin must fail")
                .code,
            "origin_denied"
        );
    }

    #[tokio::test]
    async fn local_origins_remain_allowed() {
        let (_directory, policy) = policy(true).await;
        assert!(policy.authorize("http://localhost:8080").await.is_ok());
        assert!(policy.authorize("https://127.0.0.1:8080").await.is_ok());
    }

    #[tokio::test]
    async fn configured_tunnel_origin_is_allowed() {
        let (_directory, policy) = policy(true).await;
        sqlx::query("INSERT INTO tunnels(base_url,created_at_ms,updated_at_ms) VALUES(?,?,?)")
            .bind("https://mcp.example.com")
            .bind(1_i64)
            .bind(1_i64)
            .execute(policy.repository.pool())
            .await
            .expect("insert tunnel");

        assert!(policy.authorize("https://mcp.example.com").await.is_ok());
        assert_eq!(
            policy
                .authorize("https://evil.example")
                .await
                .expect_err("unconfigured public origin must fail")
                .code,
            "origin_denied"
        );
    }
}
