//! Canonical server-origin derivation and comparison for server-identity binding.
//!
//! A stored Client API Token is minted against one RomM server and must never be
//! replayed against another. The comparison key is the full origin — scheme,
//! host, and (non-default) port. `https://h` and `http://h` are distinct origins:
//! a downgrade to plaintext is a different (and hostile) destination.
//!
//! The parsing follows Python's `urllib.parse.urlsplit` (which the Python backend
//! used), so both backends agree on every stored `romm_api_token_origin`.

use std::net::Ipv6Addr;

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

/// Return the canonical `scheme://host[:port]` origin of `url`, or `None`.
///
/// Scheme and host are lowercased; the scheme's default port is folded out; any
/// path, query, fragment, userinfo or trailing slash is dropped. An IPv6 literal
/// keeps its brackets and a trailing FQDN root dot is removed, so the result
/// re-parses unchanged. `None` when `url` has no http(s) scheme or no host.
pub fn normalize_origin(url: &str) -> Option<String> {
    // urlsplit strips leading C0-control/space characters and removes tab/CR/LF.
    let cleaned: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();

    let colon = cleaned.find(':')?;
    let raw_scheme = &cleaned[..colon];
    let first = raw_scheme.chars().next()?;
    if !first.is_ascii_alphabetic()
        || !raw_scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let scheme = raw_scheme.to_ascii_lowercase();
    let default = default_port(&scheme)?;
    let rest = &cleaned[colon + 1..];
    let after_slashes = rest.strip_prefix("//")?;
    let netloc_end = after_slashes.find(['/', '?', '#']).unwrap_or(after_slashes.len());
    let netloc = &after_slashes[..netloc_end];
    let hostinfo = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);

    let (host, port_str) = if let Some((_, bracketed)) = hostinfo.split_once('[') {
        let (inner, after) = bracketed.split_once(']')?;
        // Python 3.11+ rejects a bracketed host that is not a valid IPv6 address.
        inner.parse::<Ipv6Addr>().ok()?;
        let port = after.split_once(':').map_or("", |(_, p)| p);
        (inner.to_ascii_lowercase(), port)
    } else {
        if hostinfo.contains(']') {
            return None;
        }
        let (h, p) = hostinfo.split_once(':').unwrap_or((hostinfo, ""));
        (h.to_ascii_lowercase(), p)
    };
    if host.is_empty() {
        return None;
    }
    let port = if port_str.is_empty() {
        None
    } else if port_str.chars().all(|c| c.is_ascii_digit()) {
        let value: u32 = port_str.parse().ok()?;
        if value > 65535 {
            return None;
        }
        Some(value as u16)
    } else {
        return None;
    };

    let trimmed = host.trim_end_matches('.');
    let host = if trimmed.is_empty() { host.as_str() } else { trimmed };
    let host_part = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match port {
        Some(p) if p != default => Some(format!("{scheme}://{host_part}:{p}")),
        _ => Some(format!("{scheme}://{host_part}")),
    }
}

/// True iff `a` and `b` normalize to the same origin. An unknown or
/// unparseable origin on either side is never equal to anything (fail closed).
pub fn same_origin(a: Option<&str>, b: Option<&str>) -> bool {
    match (a.and_then(normalize_origin), b.and_then(normalize_origin)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// True iff `url`, after trimming surrounding whitespace, names an http(s) origin.
pub fn is_valid_server_url(url: &str) -> bool {
    normalize_origin(url.trim()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(url: &str) -> Option<String> {
        normalize_origin(url)
    }

    #[test]
    fn normalize_basic_and_default_ports() {
        assert_eq!(n("https://romm.local").as_deref(), Some("https://romm.local"));
        assert_eq!(n("http://romm.local").as_deref(), Some("http://romm.local"));
        assert_eq!(n("https://romm.local:443").as_deref(), Some("https://romm.local"));
        assert_eq!(n("http://romm.local:80").as_deref(), Some("http://romm.local"));
        assert_eq!(n("https://romm.local:8443").as_deref(), Some("https://romm.local:8443"));
    }

    #[test]
    fn normalize_case_path_query_fragment() {
        assert_eq!(n("HTTPS://romm.local").as_deref(), Some("https://romm.local"));
        assert_eq!(n("https://RomM.Local").as_deref(), Some("https://romm.local"));
        assert_eq!(n("https://romm.local/").as_deref(), Some("https://romm.local"));
        assert_eq!(n("https://romm.local/romm/").as_deref(), Some("https://romm.local"));
        assert_eq!(
            n("https://romm.local/x?y=1#frag").as_deref(),
            Some("https://romm.local")
        );
        assert_eq!(
            n("https://user:pw@romm.local:8080/x").as_deref(),
            Some("https://romm.local:8080")
        );
    }

    #[test]
    fn normalize_scheme_and_port_sensitivity() {
        assert_ne!(n("https://romm.local"), n("http://romm.local"));
        assert_ne!(n("https://romm.local:8080"), n("https://romm.local:9090"));
        assert_eq!(n("https://romm.local/romm/"), n("https://romm.local"));
    }

    #[test]
    fn normalize_ipv6() {
        assert_eq!(n("https://[::1]:8080").as_deref(), Some("https://[::1]:8080"));
        let once = n("https://[::1]:8080").unwrap();
        assert_eq!(n(&once).as_deref(), Some(once.as_str()));
        assert_eq!(n("https://[::1]").as_deref(), Some("https://[::1]"));
        assert_eq!(n("https://[::1]:443").as_deref(), Some("https://[::1]"));
        assert_eq!(n("https://[::1]:8080/romm").as_deref(), Some("https://[::1]:8080"));
        assert_eq!(
            n("https://[2001:DB8::1]:8080").as_deref(),
            Some("https://[2001:db8::1]:8080")
        );
        assert_eq!(n("https://[nonsense]:8080"), None);
        assert_eq!(n("https://[::1:8080"), None);
    }

    #[test]
    fn normalize_trailing_dot() {
        assert_eq!(n("https://host.com.").as_deref(), Some("https://host.com"));
        assert_eq!(n("https://host.com.:8443").as_deref(), Some("https://host.com:8443"));
    }

    #[test]
    fn normalize_rejects() {
        for bad in [
            "romm.local",
            "https://",
            "ftp://romm.local",
            "",
            "   ",
            "https://romm.local:notaport",
            "https://romm.local:99999",
            "://nohost",
            "localhost:8080",
        ] {
            assert_eq!(n(bad), None, "{bad}");
        }
        // An empty port is "no port", as in urlsplit.
        assert_eq!(n("https://romm.local:").as_deref(), Some("https://romm.local"));
    }

    #[test]
    fn same_origin_cases() {
        let s = |a: &str, b: &str| same_origin(Some(a), Some(b));
        assert!(s("https://romm.local", "https://romm.local"));
        assert!(s("https://romm.local:443", "https://romm.local"));
        assert!(s("https://romm.local/romm/", "https://romm.local"));
        assert!(!s("https://a.local", "https://b.local"));
        assert!(!s("https://romm.local", "http://romm.local"));
        assert!(!s("https://romm.local:8080", "https://romm.local:9090"));
        assert!(!same_origin(None, Some("https://romm.local")));
        assert!(!same_origin(Some("https://romm.local"), None));
        assert!(!same_origin(None, None));
        assert!(!s("not-a-url", "https://romm.local"));
        assert!(!s("nope", "also-nope"));
        assert!(s("https://[::1]:8080", "https://[::1]:8080"));
        assert!(s("https://[::1]:8080/romm", "https://[::1]:8080"));
        assert!(!s("https://[::1]", "https://[::1]:8080"));
        assert!(s("https://[::1]:443", "https://[::1]"));
        assert!(!s("https://[::1]:8080", "https://::1:8080"));
        assert!(s("https://host.com.", "https://host.com"));
    }

    #[test]
    fn valid_server_url() {
        for ok in [
            "http://romm.local",
            "https://romm.local",
            "https://romm.local:8443/romm",
            "  http://romm.local  ",
            "\t https://romm.local \n",
        ] {
            assert!(is_valid_server_url(ok), "{ok}");
        }
        for bad in ["romm.local", "ftp://romm.local", "", "   ", "https://", "://nohost"] {
            assert!(!is_valid_server_url(bad), "{bad}");
        }
    }
}
