/// Parsed URL components used by the built-in downloader.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    /// URL scheme (normalized to lowercase).
    pub scheme: String,
    /// Host name.
    pub host: String,
    /// Explicit port, if provided.
    pub port: Option<u16>,
    /// Path (always starts with `/`).
    pub path: String,
    /// Raw query string (without `?`).
    pub query: Option<String>,
    /// Raw fragment string (without `#`).
    pub fragment: Option<String>,
}

impl Url {
    /// Parse a URL string.
    pub fn new(url: &str) -> Result<Self, Error> {
        #[cfg(feature = "url")]
        {
            parse_url_crate(url).map_err(Error::Parse)
        }
        #[cfg(not(feature = "url"))]
        {
            parse_url_builtin(url).map_err(Error::Parse)
        }
    }

    /// Return the path plus query, if present.
    pub fn path_and_query(&self) -> String {
        match &self.query {
            Some(q) if !q.is_empty() => format!("{}?{}", self.path, q),
            _ => self.path.clone(),
        }
    }

    /// Return `host[:port]`.
    pub fn authority(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.host, p),
            None => self.host.clone(),
        }
    }

    /// Reconstruct the URL string (for CLI arguments and redirect chains).
    pub fn to_url_string(&self) -> String {
        let mut out = format!("{}://{}", self.scheme, self.authority());
        out.push_str(&self.path);
        if let Some(q) = &self.query {
            if !q.is_empty() {
                out.push('?');
                out.push_str(q);
            }
        }
        if let Some(f) = &self.fragment {
            out.push('#');
            out.push_str(f);
        }
        out
    }
}

#[allow(dead_code)]
impl Url {
    /// Return the fragment, if present.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
}

#[derive(Debug)]
/// URL parse error.
pub enum Error {
    /// Parse failure message.
    Parse(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Parse(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(feature = "url")]
fn parse_url_crate(input: &str) -> Result<Url, String> {
    let u = url::Url::parse(input).map_err(|e| e.to_string())?;
    let scheme = u.scheme().to_string();
    let host = u
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();
    let port = u.port(); // keep explicit port only (best-effort)

    let path = {
        let p = u.path();
        if p.is_empty() {
            "/".to_string()
        } else {
            p.to_string()
        }
    };
    let query = u.query().map(|s| s.to_string());
    let fragment = u.fragment().map(|s| s.to_string());

    Ok(Url {
        scheme,
        host,
        port,
        path,
        query,
        fragment,
    })
}

/// Built-in parser (always compiled; used by [`Url::new`] unless the `url` feature is on).
#[allow(dead_code)]
fn parse_url_builtin(input: &str) -> Result<Url, String> {
    // scheme://[userinfo@]host[:port]/path?query#fragment
    let input = input.trim();
    let (scheme, rest) = input
        .split_once("://")
        .ok_or_else(|| "missing scheme (expected '://')".to_string())?;
    if scheme.is_empty() {
        return Err("empty scheme".to_string());
    }

    let (rest, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (rest, None),
    };

    let (rest, query) = match rest.split_once('?') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (rest, None),
    };

    let split = find_authority_path_split(rest)?;
    let authority_raw = &rest[..split];
    let mut path = if split < rest.len() {
        rest[split..].to_string()
    } else {
        "/".to_string()
    };
    if path.is_empty() {
        path = "/".to_string();
    }

    let authority = strip_userinfo(authority_raw);
    if authority.is_empty() {
        return Err("missing host".to_string());
    }

    let (host, port) = parse_host_port(authority)?;

    Ok(Url {
        scheme: scheme.to_ascii_lowercase(),
        host,
        port,
        path,
        query,
        fragment,
    })
}

fn strip_userinfo(authority: &str) -> &str {
    authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority)
}

fn find_authority_path_split(s: &str) -> Result<usize, String> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    if bytes.first() == Some(&b'[') {
        while i < bytes.len() && bytes[i] != b']' {
            i += 1;
        }
        if i >= bytes.len() {
            return Err("unclosed '[' in host".to_string());
        }
        i += 1; // past ']'
        if i < bytes.len() && bytes[i] == b':' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i >= bytes.len() {
            return Ok(s.len());
        }
        if bytes[i] != b'/' {
            return Err("expected '/' or end of authority after IPv6 host".to_string());
        }
        return Ok(i);
    }

    Ok(s.find('/').unwrap_or(s.len()))
}

fn parse_host_port(authority: &str) -> Result<(String, Option<u16>), String> {
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or_else(|| "unclosed '[' in host".to_string())?;
        let host = authority[..=close].to_string();
        let after = &authority[close + 1..];
        if after.is_empty() {
            return Ok((host, None));
        }
        let port_str = after.strip_prefix(':').ok_or_else(|| {
            "invalid text after IPv6 host (expected optional ':port')".to_string()
        })?;
        if port_str.is_empty() {
            return Err("empty port after IPv6 host".to_string());
        }
        let port: u16 = port_str
            .parse()
            .map_err(|_| "invalid port after IPv6 host".to_string())?;
        return Ok((host, Some(port)));
    }

    if let Some((h, p)) = authority.rsplit_once(':') {
        if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && p.len() <= 5 {
            let port: u16 = p.parse().map_err(|_| "invalid port".to_string())?;
            return Ok((h.to_string(), Some(port)));
        }
    }

    Ok((authority.to_string(), None))
}

#[cfg(test)]
mod builtin_tests {
    use super::{Url, parse_url_builtin};

    fn b(s: &str) -> Url {
        parse_url_builtin(s).unwrap()
    }

    #[test]
    fn simple_host_port_path() {
        let u = b("http://127.0.0.1:8080/anything/x");
        assert_eq!(u.scheme, "http");
        assert_eq!(u.host, "127.0.0.1");
        assert_eq!(u.port, Some(8080));
        assert_eq!(u.path, "/anything/x");
        assert_eq!(u.authority(), "127.0.0.1:8080");
    }

    #[test]
    fn ipv6_brackets_and_port() {
        let u = b("http://[::1]:9999/foo");
        assert_eq!(u.host, "[::1]");
        assert_eq!(u.port, Some(9999));
        assert_eq!(u.path, "/foo");
    }

    #[test]
    fn ipv6_no_port() {
        let u = b("http://[::1]/bar");
        assert_eq!(u.host, "[::1]");
        assert_eq!(u.port, None);
        assert_eq!(u.path, "/bar");
    }

    #[test]
    fn userinfo_stripped() {
        let u = b("http://user:pass@example.com:7/p");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, Some(7));
        assert_eq!(u.path, "/p");
    }

    #[test]
    fn tough_path_query() {
        let u = b("http://127.0.0.1:8080/anything/foo$%25?!&1");
        assert_eq!(u.path, "/anything/foo$%25");
        assert_eq!(u.query.as_deref(), Some("!&1"));
    }

    #[test]
    fn host_only_default_path() {
        let u = b("https://example.com");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.path, "/");
        assert_eq!(u.port, None);
    }
}
