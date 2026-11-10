//! Pure rules for transferring a RomM multipart ROM as a cached ZIP.
//!
//! RomM serves a multipart ROM two ways: without `Range` it streams a ZIP
//! generated on the fly; with `Range` it builds (or reuses) one complete cached
//! ZIP and serves that as a static file. The two are not byte-identical, so a
//! partial may only be appended to when it provably belongs to one cached
//! representation. That identity is the strong `ETag` (plus length and
//! `Last-Modified`) the cached response supplied. Everything here is decision
//! logic over header values; the HTTP adapter does the I/O.

/// What a cached archive response revealed about the representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveIdentity {
    /// The exact `ETag` header value (quotes included).
    pub etag: String,
    pub last_modified: Option<String>,
    /// Complete representation length in bytes.
    pub length: u64,
}

/// A strong entity tag: quoted, opaque, and without the weak `W/` prefix.
/// A weak tag does not identify bytes, so it must never be sent in `If-Range`.
pub fn is_strong_etag(value: &str) -> bool {
    let v = value.trim();
    v.len() >= 2 && v.starts_with('"') && v.ends_with('"') && !v[1..v.len() - 1].contains('"')
}

/// How the next request for a multipart ROM must be shaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumePlan {
    /// Ask for the whole cached archive (`Range: bytes=0-`); any existing
    /// partial is replaced once a valid response is accepted.
    Fresh,
    /// Continue the partial: `Range: bytes=<local size>-` + `If-Range: <etag>`.
    Append(ArchiveIdentity),
    /// A partial of unknown origin (made before validators were stored). It is
    /// kept untouched until the user chooses to restart.
    NeedsRecovery,
}

/// Decide how to continue a multipart transfer.
///
/// * `partial_len` is the actual size of the file on disk, never a stored counter.
/// * `restart` is an explicit user choice to replace the partial.
/// * Without a usable strong validator no append is attempted: the
///   conservative path is a fresh transfer.
pub fn plan_resume(
    partial_len: u64,
    identity: Option<&ArchiveIdentity>,
    needs_recovery: bool,
    restart: bool,
) -> ResumePlan {
    if partial_len == 0 || restart {
        return ResumePlan::Fresh;
    }
    if needs_recovery {
        return ResumePlan::NeedsRecovery;
    }
    match identity {
        Some(i) if is_strong_etag(&i.etag) && i.length > 0 && partial_len <= i.length => {
            ResumePlan::Append(i.clone())
        }
        _ => ResumePlan::Fresh,
    }
}

/// The response headers the decision needs.
#[derive(Debug, Clone, Default)]
pub struct ResponseHead<'a> {
    pub status: u16,
    /// `(start, total)` of a parsed `Content-Range`.
    pub content_range: Option<(u64, u64)>,
    pub content_length: Option<u64>,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
    pub content_encoding: Option<&'a str>,
    pub content_type: Option<&'a str>,
}

/// Why a response is refused (never written to the archive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    ContentEncoding(String),
    NotAnArchive(String),
    UnexpectedStatus(u16),
    MissingContentRange,
    WrongOffset { got: u64, local: u64 },
    LengthMismatch { got: u64, expected: u64 },
    ValidatorMismatch,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContentEncoding(e) => write!(f, "unexpected content encoding {e:?} (byte offsets would not match)"),
            Self::NotAnArchive(t) => write!(f, "response is not an archive (content type {t:?})"),
            Self::UnexpectedStatus(s) => write!(f, "unexpected HTTP status {s} for a download"),
            Self::MissingContentRange => write!(f, "partial response without a usable Content-Range"),
            Self::WrongOffset { got, local } => {
                write!(f, "partial response starts at byte {got}, local file is {local} bytes")
            }
            Self::LengthMismatch { got, expected } => {
                write!(f, "archive length changed: server says {got}, saved {expected}")
            }
            Self::ValidatorMismatch => write!(f, "response validator contradicts the saved one"),
        }
    }
}

fn check_encoding(head: &ResponseHead<'_>) -> Result<(), Rejection> {
    match head.content_encoding.map(str::trim) {
        None | Some("") => Ok(()),
        Some(e) if e.eq_ignore_ascii_case("identity") => Ok(()),
        Some(e) => Err(Rejection::ContentEncoding(e.to_string())),
    }
}

fn check_not_error_page(head: &ResponseHead<'_>) -> Result<(), Rejection> {
    if let Some(t) = head.content_type {
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("text/") || lower.contains("json") {
            return Err(Rejection::NotAnArchive(t.to_string()));
        }
    }
    Ok(())
}

/// What a fresh (`Range: bytes=0-`) response means for the transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshAccepted {
    /// Complete length (0 when the server did not say).
    pub total: u64,
    /// Present only when the response carries a strong `ETag` and a length:
    /// the partial may then be resumed against it.
    pub identity: Option<ArchiveIdentity>,
    /// A cached (`206`) response rather than a streamed `200`.
    pub cached: bool,
}

/// Judge the response to the first request for a multipart ROM.
pub fn judge_fresh(head: &ResponseHead<'_>) -> Result<FreshAccepted, Rejection> {
    check_encoding(head)?;
    check_not_error_page(head)?;
    let (total, cached) = match head.status {
        206 => {
            let (start, total) = head.content_range.ok_or(Rejection::MissingContentRange)?;
            if start != 0 {
                return Err(Rejection::WrongOffset { got: start, local: 0 });
            }
            (total, true)
        }
        200 => (head.content_length.unwrap_or(0), false),
        other => return Err(Rejection::UnexpectedStatus(other)),
    };
    let identity = match head.etag {
        Some(etag) if is_strong_etag(etag) && total > 0 => Some(ArchiveIdentity {
            etag: etag.to_string(),
            last_modified: head.last_modified.map(str::to_string),
            length: total,
        }),
        _ => None,
    };
    Ok(FreshAccepted { total, identity, cached })
}

/// What a resume (`Range` + `If-Range`) response means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeVerdict {
    /// A consistent `206` at the local offset: append.
    Append,
    /// A complete `200` (validator changed or `Range` ignored): never append;
    /// the partial is replaced once this response's own identity is saved.
    Replace(FreshAccepted),
    /// `416`: the local file may be the whole archive; the caller decides.
    NotSatisfiable,
}

/// Judge the response to a conditional resume request.
pub fn judge_resume(
    head: &ResponseHead<'_>,
    local_len: u64,
    expected: &ArchiveIdentity,
) -> Result<ResumeVerdict, Rejection> {
    if head.status == 416 {
        return Ok(ResumeVerdict::NotSatisfiable);
    }
    check_encoding(head)?;
    check_not_error_page(head)?;
    match head.status {
        206 => {
            let (start, total) = head.content_range.ok_or(Rejection::MissingContentRange)?;
            if start != local_len {
                return Err(Rejection::WrongOffset { got: start, local: local_len });
            }
            if total != expected.length {
                return Err(Rejection::LengthMismatch { got: total, expected: expected.length });
            }
            if head.etag.is_some_and(|e| e != expected.etag) {
                return Err(Rejection::ValidatorMismatch);
            }
            if let (Some(got), Some(saved)) = (head.last_modified, expected.last_modified.as_deref())
                && got != saved
            {
                return Err(Rejection::ValidatorMismatch);
            }
            Ok(ResumeVerdict::Append)
        }
        200 => judge_fresh(head).map(ResumeVerdict::Replace),
        other => Err(Rejection::UnexpectedStatus(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ArchiveIdentity {
        ArchiveIdentity {
            etag: "\"abc123\"".into(),
            last_modified: Some("Mon, 01 Jan 2026 00:00:00 GMT".into()),
            length: 1000,
        }
    }

    #[test]
    fn strong_etags_are_quoted_opaque_tags() {
        assert!(is_strong_etag("\"abc\""));
        assert!(is_strong_etag("\"\""));
        assert!(!is_strong_etag("W/\"abc\""));
        assert!(!is_strong_etag("abc"));
        assert!(!is_strong_etag("\"a\"b\""));
        assert!(!is_strong_etag(""));
        assert!(!is_strong_etag("\""));
    }

    #[test]
    fn plan_resume_follows_the_conservative_rules() {
        let id = identity();
        assert_eq!(plan_resume(0, Some(&id), false, false), ResumePlan::Fresh);
        assert_eq!(plan_resume(500, Some(&id), false, false), ResumePlan::Append(id.clone()));
        assert_eq!(plan_resume(1000, Some(&id), false, false), ResumePlan::Append(id.clone()));
        // the actual local size wins: a partial longer than the archive is stale
        assert_eq!(plan_resume(1001, Some(&id), false, false), ResumePlan::Fresh);
        // no validator, or only a weak one: never append
        assert_eq!(plan_resume(500, None, false, false), ResumePlan::Fresh);
        let weak = ArchiveIdentity { etag: "W/\"abc\"".into(), ..id.clone() };
        assert_eq!(plan_resume(500, Some(&weak), false, false), ResumePlan::Fresh);
        let unsized_id = ArchiveIdentity { length: 0, ..id.clone() };
        assert_eq!(plan_resume(500, Some(&unsized_id), false, false), ResumePlan::Fresh);
        // a legacy partial is preserved until the user restarts it
        assert_eq!(plan_resume(500, None, true, false), ResumePlan::NeedsRecovery);
        assert_eq!(plan_resume(500, Some(&id), true, false), ResumePlan::NeedsRecovery);
        assert_eq!(plan_resume(500, None, true, true), ResumePlan::Fresh);
        assert_eq!(plan_resume(0, None, true, false), ResumePlan::Fresh);
    }

    fn head(status: u16) -> ResponseHead<'static> {
        ResponseHead { status, ..ResponseHead::default() }
    }

    #[test]
    fn fresh_cached_response_yields_an_identity() {
        let h = ResponseHead {
            content_range: Some((0, 1000)),
            etag: Some("\"abc123\""),
            last_modified: Some("Mon, 01 Jan 2026 00:00:00 GMT"),
            ..head(206)
        };
        let ok = judge_fresh(&h).unwrap();
        assert_eq!(ok, FreshAccepted { total: 1000, identity: Some(identity()), cached: true });
    }

    #[test]
    fn fresh_streamed_response_has_no_identity() {
        let h = ResponseHead { content_length: Some(900), ..head(200) };
        assert_eq!(judge_fresh(&h).unwrap(), FreshAccepted { total: 900, identity: None, cached: false });
        // a weak tag does not identify bytes
        let weak = ResponseHead { content_length: Some(900), etag: Some("W/\"x\""), ..head(200) };
        assert_eq!(judge_fresh(&weak).unwrap().identity, None);
        // a strong tag without a length cannot be validated later
        let unsized_resp = ResponseHead { etag: Some("\"x\""), ..head(200) };
        assert_eq!(judge_fresh(&unsized_resp).unwrap().identity, None);
    }

    #[test]
    fn fresh_rejects_what_is_not_the_archive() {
        let ranged = |start| ResponseHead { content_range: Some((start, 10)), ..head(206) };
        assert_eq!(judge_fresh(&ranged(5)).unwrap_err(), Rejection::WrongOffset { got: 5, local: 0 });
        assert_eq!(judge_fresh(&head(206)).unwrap_err(), Rejection::MissingContentRange);
        for status in [202, 204, 301, 416] {
            assert_eq!(judge_fresh(&head(status)).unwrap_err(), Rejection::UnexpectedStatus(status));
        }
        let gz = ResponseHead { content_encoding: Some("gzip"), content_length: Some(5), ..head(200) };
        assert!(matches!(judge_fresh(&gz).unwrap_err(), Rejection::ContentEncoding(_)));
        let ident = ResponseHead { content_encoding: Some("identity"), content_length: Some(5), ..head(200) };
        assert!(judge_fresh(&ident).is_ok());
        for ty in ["application/json", "text/html; charset=utf-8"] {
            let page = ResponseHead { content_type: Some(ty), content_length: Some(5), ..head(200) };
            assert!(matches!(judge_fresh(&page).unwrap_err(), Rejection::NotAnArchive(_)));
        }
        let zip = ResponseHead { content_type: Some("application/zip"), content_length: Some(5), ..head(200) };
        assert!(judge_fresh(&zip).is_ok());
    }

    fn partial(start: u64, total: u64) -> ResponseHead<'static> {
        ResponseHead {
            content_range: Some((start, total)),
            etag: Some("\"abc123\""),
            last_modified: Some("Mon, 01 Jan 2026 00:00:00 GMT"),
            ..head(206)
        }
    }

    #[test]
    fn resume_appends_only_a_consistent_206() {
        assert_eq!(judge_resume(&partial(400, 1000), 400, &identity()).unwrap(), ResumeVerdict::Append);
        // validators are optional on a 206, but must not contradict
        let bare = ResponseHead { content_range: Some((400, 1000)), ..head(206) };
        assert_eq!(judge_resume(&bare, 400, &identity()).unwrap(), ResumeVerdict::Append);
    }

    #[test]
    fn resume_rejects_inconsistent_206() {
        let id = identity();
        assert_eq!(
            judge_resume(&partial(399, 1000), 400, &id).unwrap_err(),
            Rejection::WrongOffset { got: 399, local: 400 }
        );
        assert_eq!(
            judge_resume(&partial(400, 1001), 400, &id).unwrap_err(),
            Rejection::LengthMismatch { got: 1001, expected: 1000 }
        );
        assert_eq!(judge_resume(&head(206), 400, &id).unwrap_err(), Rejection::MissingContentRange);
        let other_tag = ResponseHead { etag: Some("\"zzz\""), ..partial(400, 1000) };
        assert_eq!(judge_resume(&other_tag, 400, &id).unwrap_err(), Rejection::ValidatorMismatch);
        let other_date = ResponseHead { last_modified: Some("Tue, 02 Jan 2026 00:00:00 GMT"), ..partial(400, 1000) };
        assert_eq!(judge_resume(&other_date, 400, &id).unwrap_err(), Rejection::ValidatorMismatch);
        let gz = ResponseHead { content_encoding: Some("gzip"), ..partial(400, 1000) };
        assert!(matches!(judge_resume(&gz, 400, &id).unwrap_err(), Rejection::ContentEncoding(_)));
    }

    #[test]
    fn resume_200_is_never_an_append() {
        let h = ResponseHead {
            content_length: Some(2000),
            etag: Some("\"new\""),
            ..head(200)
        };
        match judge_resume(&h, 400, &identity()).unwrap() {
            ResumeVerdict::Replace(f) => {
                assert_eq!(f.total, 2000);
                assert_eq!(f.identity.unwrap().etag, "\"new\"");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
        let page = ResponseHead { content_type: Some("text/html"), ..h.clone() };
        assert!(judge_resume(&page, 400, &identity()).is_err());
    }

    #[test]
    fn resume_416_and_other_statuses() {
        assert_eq!(judge_resume(&head(416), 1000, &identity()).unwrap(), ResumeVerdict::NotSatisfiable);
        for status in [202, 204, 302] {
            assert_eq!(
                judge_resume(&head(status), 400, &identity()).unwrap_err(),
                Rejection::UnexpectedStatus(status)
            );
        }
    }
}
