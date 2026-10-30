//! Which web origins may drive the sidecar.
//!
//! The server listens on loopback with no token, so without this any website a
//! cashier opens could `fetch("http://127.0.0.1:9787/print/raw")` and print, or
//! list the terminal's printers. CORS alone is not enough: it only stops the
//! page from READING the answer, the request still arrives. So the server checks
//! the `Origin` header itself and refuses anything not on the allowlist.
//!
//! Allowed:
//!  - `http(s)://localhost`, `127.0.0.1`, `[::1]` on any port (dev);
//!  - `https://neogeniux.com` and any `https://*.neogeniux.com` subdomain;
//!  - exact origins listed in `NEOPRINT_ALLOWED_ORIGINS` (comma separated),
//!    for preview deploys and white-label domains.
//! A request with NO `Origin` header (curl, native tools on the machine) is
//! inside the loopback trust boundary and is allowed by the caller.

use std::sync::OnceLock;

const APEX_DOMAIN: &str = "neogeniux.com";
const ENV_EXTRA: &str = "NEOPRINT_ALLOWED_ORIGINS";

fn normalize(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn extra_origins() -> &'static Vec<String> {
    static EXTRA: OnceLock<Vec<String>> = OnceLock::new();
    EXTRA.get_or_init(|| {
        std::env::var(ENV_EXTRA)
            .unwrap_or_default()
            .split(',')
            .map(normalize)
            .filter(|s| !s.is_empty())
            .collect()
    })
}

/// Allowlist check using the built-in rules plus `NEOPRINT_ALLOWED_ORIGINS`.
pub fn is_allowed(origin: &str) -> bool {
    is_allowed_with(origin, extra_origins())
}

pub fn is_allowed_with(origin: &str, extra: &[String]) -> bool {
    let origin = normalize(origin);
    if extra.iter().any(|e| *e == origin) {
        return true;
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false; // "null", garbage
    };
    if scheme != "http" && scheme != "https" {
        return false;
    }
    // `rest` must be exactly host[:port]: no path, query, or userinfo.
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return false;
    }
    let (host, port) = split_host_port(rest);
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    // Production: https on the default port only, apex or a real subdomain.
    scheme == "https"
        && port.is_none()
        && (host == APEX_DOMAIN || host.ends_with(&format!(".{APEX_DOMAIN}")))
}

fn split_host_port(authority: &str) -> (&str, Option<&str>) {
    // IPv6 literal: "[::1]:4200"
    if let Some(end) = authority.find(']') {
        let host = &authority[..=end];
        let port = authority[end + 1..].strip_prefix(':');
        return (host, port);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &[String] = &[];

    #[test]
    fn allows_loopback_dev_origins_on_any_port() {
        for o in [
            "http://localhost:4200",
            "http://localhost",
            "http://127.0.0.1:4200",
            "https://localhost:3000",
            "http://[::1]:4200",
        ] {
            assert!(is_allowed_with(o, NONE), "{o}");
        }
    }

    #[test]
    fn allows_production_apex_and_subdomains_over_https() {
        for o in [
            "https://neogeniux.com",
            "https://app.neogeniux.com",
            "https://crm.neogeniux.com/",
            "HTTPS://App.NeoGeniux.com",
        ] {
            assert!(is_allowed_with(o, NONE), "{o}");
        }
    }

    #[test]
    fn rejects_foreign_lookalike_and_malformed_origins() {
        for o in [
            "https://evil.example",
            "https://neogeniux.com.evil.example",
            "https://evilneogeniux.com",
            "https://neogeniux.com@evil.example",
            "https://neogeniux.com:8443", // production is default port only
            "http://neogeniux.com",       // plain http is dev-loopback only
            "https://app.neogeniux.com/path",
            "http://localhost.evil.example",
            "null",
            "",
            "file://",
            "ftp://localhost",
        ] {
            assert!(!is_allowed_with(o, NONE), "{o:?} must be rejected");
        }
    }

    #[test]
    fn extra_origins_are_exact_match_only() {
        let extra = vec!["https://preview-123.example.dev".to_string()];
        assert!(is_allowed_with("https://preview-123.example.dev/", &extra));
        assert!(!is_allowed_with("https://other.example.dev", &extra));
        assert!(!is_allowed_with("https://x.preview-123.example.dev", &extra));
    }
}
