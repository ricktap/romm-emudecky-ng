//! RomM API error taxonomy and its mapping onto the canonical failure shape.
//!
//! Every callable that can fail answers `{success: false, reason, message}`.
//! `reason` is a coarse slug the frontend routes on; several error kinds fold
//! onto one slug (transport failures → `server_unreachable`, 401/403 →
//! `auth_failed`) while `message` stays distinct per kind.

use serde_json::{Value, json};

/// Coarse `reason` slugs shared with the frontend.
pub mod reason {
    pub const SERVER_UNREACHABLE: &str = "server_unreachable";
    pub const AUTH_FAILED: &str = "auth_failed";
    pub const NOT_FOUND: &str = "not_found";
    pub const UNKNOWN: &str = "unknown";
    pub const VERSION_ERROR: &str = "version_error";
    pub const CONFIG_ERROR: &str = "config_error";
}

/// A failed RomM request (or a refusal to send one).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RommError {
    /// 401 — bad or revoked credentials.
    #[error("{0}")]
    Auth(String),
    /// 403 — authenticated but not permitted (or a Cloudflare edge block).
    #[error("{0}")]
    Forbidden(String),
    /// 404 — `detail` carries the FastAPI `detail` string when the request read it.
    #[error("{message}")]
    NotFound { message: String, detail: Option<String> },
    /// 5xx, or 429 rate limiting.
    #[error("{message}")]
    Server { status: u16, message: String },
    /// Connection refused, DNS failure, reset.
    #[error("{0}")]
    Connection(String),
    #[error("{0}")]
    Timeout(String),
    /// TLS certificate verification failure.
    #[error("{0}")]
    Ssl(String),
    /// The stored token was minted for a different origin than `romm_url`.
    #[error("Stored token was minted for a different server than the configured URL")]
    TokenHostMismatch,
    /// Any other RomM API failure (other 4xx, malformed body, …).
    #[error("{0}")]
    Api(String),
    #[error("Pairing code is invalid or expired")]
    PairingCodeInvalid,
    #[error("Pairing token no longer exists")]
    PairingTokenGone,
    #[error("Pairing token owner is disabled")]
    PairingOwnerDisabled,
    #[error("Too many pairing-code exchange attempts")]
    PairingRateLimited,
}

impl RommError {
    /// Transient failures worth retrying. A token-host mismatch never is.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Server { .. } | Self::Connection(_) | Self::Timeout(_))
    }

    /// `(reason, user-facing message)` for this error.
    pub fn classify(&self) -> (&'static str, String) {
        match self {
            Self::Auth(_) => (
                reason::AUTH_FAILED,
                "Authentication failed \u{2014} check your username and password".into(),
            ),
            Self::Forbidden(_) => (
                reason::AUTH_FAILED,
                "Access denied \u{2014} your account lacks permissions for this action".into(),
            ),
            Self::Ssl(_) => (
                reason::SERVER_UNREACHABLE,
                "SSL certificate error \u{2014} enable 'Allow Insecure SSL' in settings for self-signed certs".into(),
            ),
            Self::Timeout(_) => (
                reason::SERVER_UNREACHABLE,
                "Request timed out \u{2014} server may be overloaded or network is slow".into(),
            ),
            Self::Connection(_) => (
                reason::SERVER_UNREACHABLE,
                "Server unreachable \u{2014} check your URL and ensure RomM is running".into(),
            ),
            Self::Server { status, .. } => (
                reason::SERVER_UNREACHABLE,
                format!("Server error ({status}) \u{2014} check your RomM server logs"),
            ),
            Self::NotFound { .. } => (reason::NOT_FOUND, "Resource not found on server".into()),
            Self::TokenHostMismatch => (
                reason::CONFIG_ERROR,
                "Your saved RomM login is for a different server. Sign in again to continue.".into(),
            ),
            other => (reason::SERVER_UNREACHABLE, other.to_string()),
        }
    }

    /// The canonical `{success: false, reason, message}` failure for this error.
    pub fn to_failure(&self) -> Value {
        let (reason, message) = self.classify();
        failure(reason, &message)
    }
}

/// Build the canonical failure shape.
pub fn failure(reason: &str, message: &str) -> Value {
    json!({"success": false, "reason": reason, "message": message})
}

/// The failure shape for a non-RomM error (disk, I/O): `reason: "unknown"`.
pub fn unknown_failure(err: &anyhow::Error) -> Value {
    failure(reason::UNKNOWN, &err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_slugs_and_distinct_messages() {
        let auth = RommError::Auth("x".into()).classify();
        let forbidden = RommError::Forbidden("x".into()).classify();
        assert_eq!(auth.0, "auth_failed");
        assert_eq!(forbidden.0, "auth_failed");
        assert_ne!(auth.1, forbidden.1);
        assert!(forbidden.1.contains("Access denied"));
        for e in [
            RommError::Ssl("x".into()),
            RommError::Timeout("x".into()),
            RommError::Connection("x".into()),
            RommError::Server {
                status: 502,
                message: "x".into(),
            },
            RommError::Api("boom".into()),
        ] {
            assert_eq!(e.classify().0, "server_unreachable", "{e:?}");
        }
        assert!(
            RommError::Ssl("x".into())
                .classify()
                .1
                .contains("SSL certificate error")
        );
        assert!(RommError::Timeout("x".into()).classify().1.contains("timed out"));
        assert!(
            RommError::Connection("x".into())
                .classify()
                .1
                .contains("Server unreachable")
        );
        assert_eq!(
            RommError::Server {
                status: 502,
                message: "x".into()
            }
            .classify()
            .1,
            "Server error (502) \u{2014} check your RomM server logs"
        );
        assert_eq!(RommError::Api("boom".into()).classify().1, "boom");
        assert_eq!(
            RommError::NotFound {
                message: "x".into(),
                detail: None
            }
            .classify()
            .0,
            "not_found"
        );
        let mismatch = RommError::TokenHostMismatch.classify();
        assert_eq!(mismatch.0, "config_error");
        assert!(mismatch.1.contains("Sign in again"));
    }

    #[test]
    fn retryable() {
        assert!(
            RommError::Server {
                status: 500,
                message: String::new()
            }
            .is_retryable()
        );
        assert!(RommError::Connection(String::new()).is_retryable());
        assert!(RommError::Timeout(String::new()).is_retryable());
        assert!(!RommError::TokenHostMismatch.is_retryable());
        assert!(!RommError::Auth(String::new()).is_retryable());
        assert!(!RommError::Ssl(String::new()).is_retryable());
        assert!(!RommError::Api(String::new()).is_retryable());
    }

    #[test]
    fn failure_shape() {
        let v = RommError::Auth("x".into()).to_failure();
        assert_eq!(v["success"], false);
        assert_eq!(v["reason"], "auth_failed");
        assert!(v["message"].is_string());
        assert_eq!(v.as_object().unwrap().len(), 3);
        let u = unknown_failure(&anyhow::anyhow!("disk full"));
        assert_eq!(
            u,
            json!({"success": false, "reason": "unknown", "message": "disk full"})
        );
    }
}
