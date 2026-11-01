//! Password email is delivered in-process. There is no in-memory job queue to
//! lose on restart; HTTP success follows the SMTP relay's acceptance.
use std::time::Duration;

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
};
use tokio::sync::Semaphore;

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    public_url: String,
    slots: Semaphore,
}

impl Mailer {
    /// Read configuration through an injected lookup so tests never mutate the
    /// process environment. An absent SMTP_HOST disables outbound email.
    pub fn from_settings(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let Some(host) = get("SMTP_HOST").filter(|s| !s.is_empty()) else {
            if [
                "SMTP_PORT",
                "SMTP_TLS",
                "SMTP_USERNAME",
                "SMTP_PASSWORD",
                "SMTP_FROM",
            ]
            .iter()
            .any(|key| get(key).is_some_and(|v| !v.is_empty()))
            {
                return Err("SMTP_HOST is required when SMTP settings are supplied".into());
            }
            return Ok(None);
        };
        if host.chars().any(char::is_whitespace) || host.contains(['/', '@']) {
            return Err("SMTP_HOST must be a host name or IP address".into());
        }
        let public_url = get("FEEDS_PUBLIC_URL")
            .or_else(|| get("TELESINK_PUBLIC_URL"))
            .ok_or("FEEDS_PUBLIC_URL is required for email")?;
        let public_url = crate::config::public_origin(public_url)?;
        let from: Mailbox = get("SMTP_FROM")
            .ok_or("SMTP_FROM is required for email")?
            .parse()
            .map_err(|_| "SMTP_FROM must contain one valid sender address")?;
        let mode = get("SMTP_TLS").unwrap_or_else(|| "starttls".into());
        let mut transport = match mode.as_str() {
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
            "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &host,
            )),
            _ => return Err("SMTP_TLS must be starttls, tls, or none".into()),
        }
        .map_err(|_| "could not initialize SMTP TLS")?;
        let default_port = match mode.as_str() {
            "tls" => 465,
            "none" => 25,
            _ => 587,
        };
        let port = get("SMTP_PORT")
            .map(|p| p.parse::<u16>())
            .transpose()
            .map_err(|_| "SMTP_PORT must be an integer from 1 to 65535")?
            .unwrap_or(default_port);
        if port == 0 {
            return Err("SMTP_PORT must be an integer from 1 to 65535".into());
        }
        match (get("SMTP_USERNAME"), get("SMTP_PASSWORD")) {
            (Some(username), Some(password)) if !username.is_empty() && !password.is_empty() => {
                transport = transport.credentials(Credentials::new(username, password));
            }
            (None, None) => {}
            _ => {
                return Err(
                    "SMTP_USERNAME and SMTP_PASSWORD must both be set or both absent".into(),
                );
            }
        }
        Ok(Some(Self {
            transport: transport
                .port(port)
                .timeout(Some(Duration::from_secs(5)))
                .build(),
            from,
            public_url: public_url.trim_end_matches('/').into(),
            slots: Semaphore::new(2),
        }))
    }

    pub async fn password_reset(&self, to: &str, token: &str) -> Result<(), &'static str> {
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| "email capacity reached")?;
        let url = format!("{}/passwords/{token}/edit", self.public_url);
        let text = format!(
            "You can reset your password on\n{url}\n\nThis link will expire in 15 minutes.\n"
        );
        let html = format!(
            "<p>You can reset your password on <a href=\"{}\">this password reset page</a>. This link will expire in 15 minutes.</p>",
            escape_html(&url)
        );
        let message = Message::builder()
            .from(self.from.clone())
            .to(to.parse().map_err(|_| "invalid recipient")?)
            .subject("Reset your password")
            .multipart(
                MultiPart::alternative()
                    .singlepart(SinglePart::plain(text))
                    .singlepart(SinglePart::html(html)),
            )
            .map_err(|_| "invalid message")?;
        tokio::time::timeout(Duration::from_secs(20), self.transport.send(message))
            .await
            .map_err(|_| "email delivery timed out")?
            .map_err(|_| "email relay rejected delivery")?;
        Ok(())
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_configuration_requires_complete_settings_and_a_trusted_origin() {
        let settings = [
            ("SMTP_HOST", "smtp.example.test"),
            ("SMTP_FROM", "Feeds <reset@example.test>"),
            ("TELESINK_PUBLIC_URL", "https://events.example.test/"),
        ];
        let check = |override_key: &str, override_value: &str| {
            Mailer::from_settings(|key| {
                if key == override_key {
                    Some(override_value.into())
                } else {
                    settings
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.to_string())
                }
            })
        };
        assert!(Mailer::from_settings(|_| None).unwrap().is_none());
        assert!(check("", "").unwrap().is_some());
        for url in [
            "//evil.test",
            "https://user:password@events.test",
            "https://events.test/reset",
            "https://events.test?x=1",
            "https://events.test/#x",
            "javascript:alert(1)",
        ] {
            assert!(check("TELESINK_PUBLIC_URL", url).is_err(), "{url}");
        }
        for (key, value) in [
            ("SMTP_PORT", "0"),
            ("SMTP_PORT", "65536"),
            ("SMTP_TLS", "opportunistic"),
            ("SMTP_FROM", "bad address"),
            ("SMTP_USERNAME", "missing-password"),
            ("SMTP_HOST", ""),
        ] {
            assert!(check(key, value).is_err(), "{key}");
        }
    }
}
