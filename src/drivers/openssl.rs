use std::io::{self, Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    RequestBuilder, Response, ResponseError, StartError, drivers::Driver, url_parser::Url, util,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenSslDriver;

impl Driver for OpenSslDriver {
    fn start(
        &self,
        req: RequestBuilder,
        target_path: std::path::PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<std::thread::JoinHandle<Result<Response, ResponseError>>, StartError> {
        let tmp_path = util::tmp_path_for_target(&target_path);

        // If we can determine upfront that this request begins with https://, try to spawn
        // `openssl` now so that "command not found" becomes a StartError.
        if let Ok(parsed) = Url::new(&req.url) {
            if parsed.scheme == "https" {
                let mut cmd = Command::new("openssl");
                cmd.arg("version")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());

                match cmd.spawn() {
                    Ok(mut child) => {
                        let _ = child.wait();
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return Err(StartError::NoDriverFound);
                    }
                    Err(e) => return Err(StartError::IoError(e)),
                }
            }
        }

        Ok(util::spawn_request_thread(
            req,
            target_path,
            tmp_path,
            cancel,
            move |req, out, cancel| download_inner(req, out, cancel),
        ))
    }
}

fn download_inner(
    req: &RequestBuilder,
    out: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<(u16, bool), ResponseError> {
    let mut current_url = req.url.clone();
    let mut redirects_left = if req.follow_redirects {
        10usize
    } else {
        0usize
    };

    loop {
        let url = Url::new(&current_url).map_err(|_| ResponseError::InvalidUrl)?;
        let (status_code, headers, body) = match url.scheme.as_str() {
            "https" => get_https_via_openssl(&url, req, cancel)?,
            "http" => get_http_via_tcp(&url, req, cancel)?,
            _ => return Err(ResponseError::UnsupportedScheme),
        };

        if is_redirect(status_code) && redirects_left > 0 {
            if let Some(loc) = header_value(&headers, "location") {
                redirects_left -= 1;
                current_url = resolve_location(&current_url, loc);
                continue;
            }
        }

        let content_encoding_gzip = header_value(&headers, "content-encoding")
            .map(|v| v.to_ascii_lowercase().contains("gzip"))
            .unwrap_or(false);

        let body = if header_value(&headers, "transfer-encoding")
            .map(|v| v.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false)
        {
            decode_chunked(&body)?
        } else if let Some(cl) = header_value(&headers, "content-length") {
            if let Ok(n) = cl.trim().parse::<usize>() {
                body.into_iter().take(n).collect::<Vec<u8>>()
            } else {
                body
            }
        } else {
            body
        };

        std::fs::write(out, &body).map_err(ResponseError::Io)?;
        return Ok((status_code, content_encoding_gzip));
    }
}

fn get_http_via_tcp(
    url: &Url,
    req: &RequestBuilder,
    cancel: &Arc<AtomicBool>,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), ResponseError> {
    let host = &url.host;
    let port = url.port.unwrap_or(80);
    let path = url.path_and_query();

    let mut request = String::new();
    request.push_str(&format!("GET {path} HTTP/1.1\r\n"));
    request.push_str(&format!("Host: {host}\r\n"));
    request.push_str("Connection: close\r\n");
    for (k, v) in util::add_common_headers(req) {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");

    if cancel.load(Ordering::SeqCst) {
        return Err(ResponseError::Cancelled);
    }

    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(ResponseError::Io)?;
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(ResponseError::Cancelled);
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ResponseError::Io(e)),
        }
    }

    parse_http_response_from_openssl_output(&buf)
}

fn get_https_via_openssl(
    url: &Url,
    req: &RequestBuilder,
    cancel: &Arc<AtomicBool>,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), ResponseError> {
    let host = url.host.clone();
    let port = url.port.unwrap_or(443);
    let path = url.path_and_query();

    let mut request = String::new();
    request.push_str(&format!("GET {path} HTTP/1.1\r\n"));
    request.push_str(&format!("Host: {host}\r\n"));
    request.push_str("Connection: close\r\n");
    for (k, v) in util::add_common_headers(req) {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");

    let mut cmd = Command::new("openssl");
    cmd.arg("s_client")
        .arg("-connect")
        .arg(format!("{host}:{port}"))
        .arg("-servername")
        .arg(&host)
        .arg("-quiet")
        .arg("-ign_eof")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(ResponseError::Io)?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            ResponseError::Io(io::Error::new(
                io::ErrorKind::Other,
                "missing openssl stdin",
            ))
        })?;
        stdin.write_all(request.as_bytes())?;
    }

    let mut stdout = child.stdout.take().ok_or_else(|| {
        ResponseError::Io(io::Error::new(
            io::ErrorKind::Other,
            "missing openssl stdout",
        ))
    })?;

    let mut buf = Vec::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ResponseError::Cancelled);
        }

        let mut chunk = [0u8; 16 * 1024];
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ResponseError::Io(e)),
        }
    }

    let _ = child.wait();
    parse_http_response_from_openssl_output(&buf)
}

fn is_redirect(code: u16) -> bool {
    matches!(code, 301 | 302 | 303 | 307 | 308)
}

fn resolve_location(current_url: &str, location: &str) -> String {
    let location = location.trim();
    if location.contains("://") {
        return location.to_string();
    }
    if location.starts_with('/') {
        if let Ok(parsed) = Url::new(current_url) {
            return format!("{}://{}{}", parsed.scheme, parsed.authority(), location);
        }
    }
    // Best-effort: treat as relative-to-root if we can't safely resolve.
    if let Ok(parsed) = Url::new(current_url) {
        return format!("{}://{}/{}", parsed.scheme, parsed.authority(), location);
    }
    location.to_string()
}

fn parse_http_response_from_openssl_output(
    all: &[u8],
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), ResponseError> {
    let needle = b"HTTP/";
    let start = all
        .windows(needle.len())
        .position(|w| w == needle)
        .ok_or(ResponseError::InvalidUrl)?;
    let http = &all[start..];
    let header_end = find_subslice(http, b"\r\n\r\n").ok_or(ResponseError::InvalidUrl)?;
    let header_bytes = &http[..header_end + 4];
    let body_bytes = &http[header_end + 4..];

    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n").filter(|l| !l.is_empty());
    let status_line = lines.next().ok_or(ResponseError::InvalidUrl)?;
    let mut status_parts = status_line.split_whitespace();
    let _httpver = status_parts.next().ok_or(ResponseError::InvalidUrl)?;
    let code_str = status_parts.next().ok_or(ResponseError::InvalidUrl)?;
    let code: u16 = code_str
        .parse()
        .map_err(|_| ResponseError::BadStatusCode(code_str.into()))?;

    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    Ok((code, headers, body_bytes.to_vec()))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_value<'a>(headers: &'a [(String, String)], key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>, ResponseError> {
    let mut out = Vec::new();
    loop {
        let line_end = find_subslice(body, b"\r\n").ok_or(ResponseError::InvalidUrl)?;
        let line = &body[..line_end];
        let line_str = String::from_utf8_lossy(line);
        let size_hex = line_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| ResponseError::InvalidUrl)?;
        body = &body[line_end + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size + 2 {
            return Err(ResponseError::InvalidUrl);
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
    Ok(out)
}
