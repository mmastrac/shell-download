//! Shared HTTP/1.1 helpers for minimal TCP and OpenSSL `s_client` backends.

use std::io::{self, Read};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    ContentEncoding, DownloadSink, RequestBuilder, ResponseError, url_parser::Url, util,
};

pub(crate) type HttpResponseParts = (u16, Vec<(String, String)>, Vec<u8>);

/// Build a `GET` request line and headers for `url` and `req`.
pub(crate) fn build_get_request(url: &Url, req: &RequestBuilder) -> String {
    let path = url.path_and_query();
    let mut request = String::new();
    request.push_str(&format!("GET {path} HTTP/1.1\r\n"));
    request.push_str(&format!("Host: {}\r\n", url.authority()));
    request.push_str("Connection: close\r\n");
    for (k, v) in util::add_common_headers(req) {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    request
}

/// Read until EOF, polling `cancel`.
pub(crate) fn read_to_vec_cancelled(
    reader: &mut impl Read,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<u8>, ResponseError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(ResponseError::Cancelled);
        }
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ResponseError::Io(e)),
        }
    }
    Ok(buf)
}

/// Follow redirects and write the final body to `sink`. `fetch` performs one HTTP exchange.
pub(crate) fn redirect_download(
    req: &RequestBuilder,
    sink: &DownloadSink,
    cancel: &Arc<AtomicBool>,
    mut fetch: impl FnMut(
        &Url,
        &RequestBuilder,
        &Arc<AtomicBool>,
    ) -> Result<HttpResponseParts, ResponseError>,
) -> Result<(u16, Option<ContentEncoding>), ResponseError> {
    let mut current_url = req.url.clone();
    let mut redirects_left = if req.follow_redirects {
        10usize
    } else {
        0usize
    };

    loop {
        let url = Url::new(&current_url).map_err(|_| ResponseError::InvalidUrl)?;
        let (status_code, headers, body) = fetch(&url, req, cancel)?;

        if is_redirect(status_code) && redirects_left > 0 {
            if let Some(loc) = header_value(&headers, "location") {
                redirects_left -= 1;
                current_url = resolve_location(&current_url, loc);
                continue;
            }
        }

        let content_encoding = header_value(&headers, "content-encoding")
            .map(|v| v.to_ascii_lowercase().contains("gzip"))
            .unwrap_or(false)
            .then_some(ContentEncoding::Gzip);

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

        sink.write_all_body(&body).map_err(ResponseError::Io)?;
        return Ok((status_code, content_encoding));
    }
}

/// Parse a raw byte stream that may include a TLS or tool prelude before the HTTP message.
pub(crate) fn parse_http_response(all: &[u8]) -> Result<HttpResponseParts, ResponseError> {
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
    if let Ok(parsed) = Url::new(current_url) {
        return format!("{}://{}/{}", parsed.scheme, parsed.authority(), location);
    }
    location.to_string()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_value<'a>(
    headers: &'a [(String, String)],
    key: &str,
) -> Option<&'a str> {
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
