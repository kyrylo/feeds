use std::{env, net::SocketAddr, sync::Arc};

/// New installations use FEEDS_ settings; existing deployments keep working.
pub fn setting(name: &str) -> Result<String, env::VarError> {
    env::var(name).or_else(|error| match (error, name.strip_prefix("FEEDS_")) {
        (env::VarError::NotPresent, Some(suffix)) => env::var(format!("TELESINK_{suffix}")),
        (error, _) => Err(error),
    })
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub secret_key_base: String,
    pub public_url: Option<String>,
    pub bind: SocketAddr,
    pub database_connections: u32,
    pub mailer: Option<Arc<crate::mail::Mailer>>,
    pub demo: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        if [
            "POSTGRES_HOST",
            "POSTGRES_PORT",
            "POSTGRES_USER",
            "POSTGRES_PASSWORD",
            "SOLID_QUEUE_IN_PUMA",
        ]
        .iter()
        .any(|name| env::var_os(name).is_some())
        {
            return Err("legacy PostgreSQL/Rails settings are present; complete the SQLite migration and update the deployment configuration before starting this runtime".into());
        }
        let database_url = crate::schema::database_url();
        let secret_key_base = env::var("SECRET_KEY_BASE")
            .map_err(|_| "SECRET_KEY_BASE is required; generate with openssl rand -hex 64 (keep the existing value when upgrading)")?;
        if secret_key_base.len() < 64 {
            return Err("SECRET_KEY_BASE must contain at least 64 bytes".into());
        }
        let public_url = setting("FEEDS_PUBLIC_URL")
            .ok()
            .map(public_origin)
            .transpose()?;
        let bind = setting("FEEDS_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3000".into())
            .parse()
            .map_err(|_| "FEEDS_BIND must be an IP address and port")?;
        let database_connections = setting("FEEDS_DATABASE_CONNECTIONS")
            .unwrap_or_else(|_| "5".into())
            .parse::<u32>()
            .map_err(|_| "FEEDS_DATABASE_CONNECTIONS must be a positive integer")?;
        if !(1..=100).contains(&database_connections) {
            return Err("FEEDS_DATABASE_CONNECTIONS must be between 1 and 100".into());
        }
        let demo = match setting("FEEDS_DEMO").ok().as_deref() {
            Some("true" | "1") => true,
            Some("false" | "0") => false,
            None => env::var("RAILS_ENV").is_ok_and(|v| v == "demo"),
            _ => return Err("FEEDS_DEMO must be true, false, 1, or 0".into()),
        };
        Ok(Self {
            database_url,
            secret_key_base,
            public_url,
            bind,
            database_connections,
            mailer: crate::mail::Mailer::from_settings(|name| setting(name).ok())?.map(Arc::new),
            demo,
        })
    }
}

pub fn public_origin(value: String) -> Result<String, String> {
    let uri = value
        .parse::<axum::http::Uri>()
        .map_err(|_| "FEEDS_PUBLIC_URL must be an http(s) origin")?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || !matches!(uri.path(), "" | "/")
        || uri.query().is_some()
        || value.contains('#')
    {
        return Err(
            "FEEDS_PUBLIC_URL must be an http(s) origin without credentials, path or query".into(),
        );
    }
    Ok(value.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::public_origin;

    #[test]
    fn public_origin_accepts_only_http_origins() {
        assert_eq!(
            public_origin("https://events.example.test/".into()).unwrap(),
            "https://events.example.test"
        );
        assert_eq!(
            public_origin("http://[::1]:3000".into()).unwrap(),
            "http://[::1]:3000"
        );
        for invalid in [
            "",
            "example.test",
            "ftp://example.test",
            "https://user:secret@example.test",
            "https://example.test/path",
            "https://example.test?query",
            "https://example.test#fragment",
        ] {
            assert!(public_origin(invalid.into()).is_err());
        }
    }
}
