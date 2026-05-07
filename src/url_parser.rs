pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

impl Url {
    pub fn new(url: &str) -> Result<Self, Error> {
        parse_url(url).map_err(Error::Parse)
    }

    pub fn path_and_query(&self) -> String {
        match &self.query {
            Some(q) if !q.is_empty() => format!("{}?{}", self.path, q),
            _ => self.path.clone(),
        }
    }

    pub fn authority(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.host, p),
            None => self.host.clone(),
        }
    }
}

#[allow(dead_code)]
impl Url {
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
}

#[derive(Debug)]
pub enum Error {
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
fn parse_url(input: &str) -> Result<Url, String> {
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

#[cfg(not(feature = "url"))]
fn parse_url(input: &str) -> Result<Url, String> {
    // Very small, best-effort parser:
    // scheme://host[:port]/path?query#fragment
    let input = input.trim();
    let (scheme, rest) = input
        .split_once("://")
        .ok_or_else(|| "missing scheme (expected '://')".to_string())?;
    if scheme.is_empty() {
        return Err("empty scheme".to_string());
    }

    let mut rest = rest;

    // fragment
    let (rest2, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (rest, None),
    };
    rest = rest2;

    // query
    let (rest3, query) = match rest.split_once('?') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (rest, None),
    };
    rest = rest3;

    // host[:port] + path
    let (hostport, path) = match rest.split_once('/') {
        Some((hp, p)) => (hp, format!("/{}", p)),
        None => (rest, "/".to_string()),
    };
    if hostport.is_empty() {
        return Err("missing host".to_string());
    }

    let (host, port) = if let Some((h, p)) = hostport.rsplit_once(':') {
        if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            let port: u16 = p.parse().map_err(|_| "invalid port".to_string())?;
            (h.to_string(), Some(port))
        } else {
            (hostport.to_string(), None)
        }
    } else {
        (hostport.to_string(), None)
    };

    Ok(Url {
        scheme: scheme.to_ascii_lowercase(),
        host,
        port,
        path,
        query,
        fragment,
    })
}
