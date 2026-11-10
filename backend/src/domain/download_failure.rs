//! Deciding whether a failed download is resumable (`interrupted`) or final
//! (`failed`), and the timestamps the queue stamps on its rows.

use crate::domain::download_job::{
    INTERRUPT_AUTH_FAILED, INTERRUPT_DISK_FULL, INTERRUPT_NEEDS_RECOVERY, INTERRUPT_SERVER_UNREACHABLE,
    INTERRUPT_TIMEOUT,
};
use crate::domain::errors::RommError;

/// The terminal verdict of a failed transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Keep the partial; the reason slug says why.
    Interrupted(&'static str),
    /// Can never succeed as-is; delete the partial.
    Failed,
}

/// Marker error: the transfer was stopped by a cancel, pause, or shutdown.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("transfer cancelled")]
pub struct TransferCancelled;

/// Marker error: the partial on disk cannot be trusted to continue (the server
/// answered a resume with an inconsistent representation, or a `416` that does
/// not match the saved length). The partial is kept untouched and the user is
/// offered a restart.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("the partial download cannot be resumed safely: {0}")]
pub struct ArchiveRecoveryNeeded(pub String);

/// Classify a download failure.
///
/// `interrupted` keeps the partial: a full disk (`ENOSPC` / `EDQUOT` anywhere
/// in the cause chain), an auth failure (401/403/token bound to another
/// server), or any transport / server failure (unreachable, timeout, 5xx,
/// 408/429, a stream cut short). `failed` is for what can never succeed as-is:
/// a 404 or other 4xx, a transfer that overshot its declared size or produced
/// nothing, or any local invariant / path / archive error.
pub fn classify_download_failure(err: &anyhow::Error) -> Outcome {
    if err.chain().any(|c| c.downcast_ref::<ArchiveRecoveryNeeded>().is_some()) {
        return Outcome::Interrupted(INTERRUPT_NEEDS_RECOVERY);
    }
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>()
            && matches!(io.raw_os_error(), Some(code) if code == libc::ENOSPC || code == libc::EDQUOT)
        {
            return Outcome::Interrupted(INTERRUPT_DISK_FULL);
        }
    }
    let Some(romm) = err.downcast_ref::<RommError>() else {
        return Outcome::Failed;
    };
    match romm {
        RommError::Auth(_) | RommError::Forbidden(_) | RommError::TokenHostMismatch => {
            Outcome::Interrupted(INTERRUPT_AUTH_FAILED)
        }
        RommError::NotFound { .. } => Outcome::Failed,
        RommError::Server { .. } => Outcome::Interrupted(INTERRUPT_SERVER_UNREACHABLE),
        // Connected, but the server did not answer (or went quiet) in time.
        RommError::Timeout(_) => Outcome::Interrupted(INTERRUPT_TIMEOUT),
        other => {
            if let Some(status) = leading_http_status(&other.to_string())
                && (400..500).contains(&status)
                && status != 408
                && status != 429
            {
                return Outcome::Failed;
            }
            for cause in err.chain() {
                let text = cause.to_string();
                if text.contains("Download produced 0 bytes") {
                    return Outcome::Failed;
                }
                if let Some((got, expected)) = incomplete_counts(&text)
                    && got > expected
                {
                    return Outcome::Failed;
                }
            }
            Outcome::Interrupted(INTERRUPT_SERVER_UNREACHABLE)
        }
    }
}

/// `HTTP <3 digits>` at the start of `text`, followed by a non-word character.
fn leading_http_status(text: &str) -> Option<u16> {
    let rest = text.strip_prefix("HTTP ")?;
    let digits = rest.get(..3)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if rest[3..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    digits.parse().ok()
}

/// `(got, expected)` from "Download incomplete: got N bytes, expected M".
fn incomplete_counts(text: &str) -> Option<(u64, u64)> {
    const NEEDLE: &str = "Download incomplete: got ";
    let start = text.find(NEEDLE)? + NEEDLE.len();
    let rest = &text[start..];
    let got_end = rest.find(|c: char| !c.is_ascii_digit())?;
    let got: u64 = rest[..got_end].parse().ok()?;
    let rest = rest[got_end..].strip_prefix(" bytes, expected ")?;
    let exp_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let expected: u64 = rest[..exp_end].parse().ok()?;
    Some((got, expected))
}

/// ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS+00:00`) for epoch seconds.
pub fn iso_from_epoch(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn conn(msg: &str) -> anyhow::Error {
        anyhow!(RommError::Connection(msg.into()))
    }

    #[test]
    fn transport_and_server_errors_interrupt() {
        assert_eq!(
            classify_download_failure(&conn("down")),
            Outcome::Interrupted("server_unreachable")
        );
        assert_eq!(
            classify_download_failure(&anyhow!(RommError::Timeout("slow".into()))),
            Outcome::Interrupted("timeout")
        );
        for status in [503, 429] {
            let e = anyhow!(RommError::Server {
                status,
                message: "x".into()
            });
            assert_eq!(classify_download_failure(&e), Outcome::Interrupted("server_unreachable"));
        }
        let e = anyhow!(RommError::Api("HTTP 408: Request Timeout (GET /x)".into()));
        assert_eq!(classify_download_failure(&e), Outcome::Interrupted("server_unreachable"));
    }

    #[test]
    fn an_untrusted_partial_needs_recovery_not_failure() {
        let e = anyhow::Error::new(ArchiveRecoveryNeeded("length changed".into()));
        assert_eq!(classify_download_failure(&e), Outcome::Interrupted("needs_recovery"));
        let wrapped = e.context("resuming");
        assert_eq!(classify_download_failure(&wrapped), Outcome::Interrupted("needs_recovery"));
    }

    #[test]
    fn disk_full_anywhere_in_chain() {
        let enospc = std::io::Error::from_raw_os_error(libc::ENOSPC);
        let wrapped = anyhow::Error::new(enospc).context(RommError::Connection("transfer failed".into()));
        assert_eq!(classify_download_failure(&wrapped), Outcome::Interrupted("disk_full"));
        let bare = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EDQUOT));
        assert_eq!(classify_download_failure(&bare), Outcome::Interrupted("disk_full"));
        let deep = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOSPC))
            .context("writing")
            .context("extracting");
        assert_eq!(classify_download_failure(&deep), Outcome::Interrupted("disk_full"));
    }

    #[test]
    fn auth_failures_interrupt() {
        for e in [
            RommError::Auth("401".into()),
            RommError::Forbidden("403".into()),
            RommError::TokenHostMismatch,
        ] {
            assert_eq!(classify_download_failure(&anyhow!(e)), Outcome::Interrupted("auth_failed"));
        }
    }

    #[test]
    fn not_found_and_other_4xx_fail() {
        let nf = anyhow!(RommError::NotFound {
            message: "404".into(),
            detail: None
        });
        assert_eq!(classify_download_failure(&nf), Outcome::Failed);
        let e = anyhow!(RommError::Api("HTTP 416: Range Not Satisfiable (GET /x)".into()));
        assert_eq!(classify_download_failure(&e), Outcome::Failed);
    }

    #[test]
    fn short_stream_interrupts_but_overshoot_and_empty_fail() {
        assert_eq!(
            classify_download_failure(&conn("Download incomplete: got 50 bytes, expected 100")),
            Outcome::Interrupted("server_unreachable")
        );
        assert_eq!(
            classify_download_failure(&conn("Download incomplete: got 150 bytes, expected 100")),
            Outcome::Failed
        );
        assert_eq!(
            classify_download_failure(&conn(
                "Download produced 0 bytes (no Content-Length header and no data received)"
            )),
            Outcome::Failed
        );
    }

    #[test]
    fn local_errors_fail() {
        assert_eq!(classify_download_failure(&anyhow!("bad install")), Outcome::Failed);
        let perm = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EACCES));
        assert_eq!(classify_download_failure(&perm), Outcome::Failed);
        assert_eq!(classify_download_failure(&anyhow!(TransferCancelled)), Outcome::Failed);
    }

    #[test]
    fn helpers() {
        assert_eq!(leading_http_status("HTTP 404: x"), Some(404));
        assert_eq!(leading_http_status("HTTP 4041"), None);
        assert_eq!(leading_http_status("HTTP 40"), None);
        assert_eq!(leading_http_status("xHTTP 404"), None);
        assert_eq!(leading_http_status("HTTP 404"), Some(404));
        assert_eq!(incomplete_counts("Download incomplete: got 1 bytes, expected 2"), Some((1, 2)));
        assert_eq!(incomplete_counts("Download incomplete: got x"), None);
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(iso_from_epoch(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso_from_epoch(1_767_225_600), "2026-01-01T00:00:00+00:00");
        assert_eq!(iso_from_epoch(951_825_661), "2000-02-29T12:01:01+00:00");
    }
}
