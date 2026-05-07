use std::io::{self, Read as _, Write as _};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use crate::{
    ContentEncoding, DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError,
    drivers::{Driver, http11},
    url_parser::Url,
    util,
};

#[derive(Debug, Clone, Copy)]
/// HTTPS via `openssl s_client`.
pub(crate) struct OpenSslDriver;

impl Driver for OpenSslDriver {
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let url = match Url::new(&req.url) {
            Ok(u) => u,
            Err(_) => return Err(StartError::NoDriverFound),
        };
        if url.scheme != "https" {
            return Err(StartError::NoDriverFound);
        }

        Self::start_https(url, req, sink, cancel)
    }
}

impl OpenSslDriver {
    /// Spawn `openssl s_client` with pipes for the initial URL, then hand the [`Child`] to the
    /// worker (first hop). Later redirect hops spawn a new client in the worker.
    fn start_https(
        initial: Url,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let mut cmd = openssl_s_client_command(&initial);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(StartError::NoDriverFound);
            }
            Err(e) => return Err(StartError::IoError(e)),
        };

        Ok(util::spawn_download_thread(req, sink, cancel, move |req, sink, cancel| {
            download_https_with_first_child(child, req, sink, cancel)
        }))
    }
}

/// `openssl s_client` with `-connect` / `-servername` for `url` (HTTPS).
fn openssl_s_client_command(url: &Url) -> Command {
    let host = url.host.clone();
    let port = url.port.unwrap_or(443);
    let mut cmd = Command::new("openssl");
    cmd.arg("s_client")
        .arg("-connect")
        .arg(format!("{host}:{port}"))
        .arg("-servername")
        .arg(host)
        .arg("-quiet")
        .arg("-ign_eof");
    cmd
}

fn download_https_with_first_child(
    first_child: Child,
    req: &RequestBuilder,
    sink: &DownloadSink,
    cancel: &Arc<AtomicBool>,
) -> Result<(u16, Option<ContentEncoding>), ResponseError> {
    let mut primed = Some(first_child);
    http11::redirect_download(req, sink, cancel, |url, req, cancel| {
        if url.scheme != "https" {
            if let Some(mut c) = primed.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            return Err(ResponseError::UnsupportedScheme);
        }
        if let Some(child) = primed.take() {
            read_https_response_from_openssl_child(child, url, req, cancel)
        } else {
            fetch_https_spawn_child(url, req, cancel)
        }
    })
}

fn fetch_https_spawn_child(
    url: &Url,
    req: &RequestBuilder,
    cancel: &Arc<AtomicBool>,
) -> Result<http11::HttpResponseParts, ResponseError> {
    let mut cmd = openssl_s_client_command(url);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = cmd.spawn().map_err(ResponseError::Io)?;
    read_https_response_from_openssl_child(child, url, req, cancel)
}

fn read_https_response_from_openssl_child(
    mut child: Child,
    url: &Url,
    req: &RequestBuilder,
    cancel: &Arc<AtomicBool>,
) -> Result<http11::HttpResponseParts, ResponseError> {
    let request = http11::build_get_request(url, req);
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ResponseError::Io(io::Error::other("missing openssl stdin")))?;
        stdin.write_all(request.as_bytes())?;
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing openssl stdout")))?;

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
    http11::parse_http_response(&buf)
}
