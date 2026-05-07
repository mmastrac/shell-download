use std::io::{self, Read as _, Write as _};
use std::process::{ChildStderr, Command};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use crate::{
    ContentEncoding, DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError,
    drivers::Driver,
    process,
    url_parser::Url,
    util,
};

use super::http11;

#[derive(Debug, Clone, Copy)]
/// HTTPS via OpenSSL (`openssl s_client`).
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
    /// Spawn `openssl s_client` with pipes for the initial URL, then hand the handles to the
    /// worker (first hop). Later redirect hops spawn a new client in the worker.
    fn start_https(
        _initial: Url,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        Ok(util::spawn_download_thread(req, sink, cancel, move |req, sink, cancel| {
            download_https_with_first_child(req, sink, cancel)
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
    req: &RequestBuilder,
    sink: &DownloadSink,
    cancel: &Arc<AtomicBool>,
) -> Result<(u16, Option<ContentEncoding>), ResponseError> {
    http11::redirect_download(req, sink, cancel, |url, req, cancel| {
        if url.scheme != "https" {
            return Err(ResponseError::UnsupportedScheme);
        }
        fetch_https_spawn_child(url, req, cancel)
    })
}

fn fetch_https_spawn_child(
    url: &Url,
    req: &RequestBuilder,
    cancel: &Arc<AtomicBool>,
) -> Result<http11::HttpResponseParts, ResponseError> {
    let mut cmd = openssl_s_client_command(url);
    let (mut child, mut stdin, mut stdout, stderr) =
        process::spawn_stdin_stdout_stderr(&mut cmd).map_err(ResponseError::Io)?;
    let stderr_join = spawn_stderr_drain(stderr);

    let request = http11::build_get_request(url, req);
    stdin.write_all(request.as_bytes())?;
    drop(stdin);

    let mut buf = Vec::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_join.join();
            return Err(ResponseError::Cancelled);
        }

        let mut chunk = [0u8; 16 * 1024];
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_join.join();
                return Err(ResponseError::Io(e));
            }
        }
    }

    drop(stdout);
    let _ = child.wait();
    let _ = stderr_join.join();
    http11::parse_http_response(&buf)
}

fn spawn_stderr_drain(mut stderr: ChildStderr) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    })
}


