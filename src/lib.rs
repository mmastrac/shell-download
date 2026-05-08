#![doc = include_str!("../README.md")]

mod drivers;
mod process;
mod sink;
mod url_parser;
mod util;

pub use sink::DownloadSink;

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::drivers::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A supported download backend.
pub enum Downloader {
    /// Use `curl`.
    Curl,
    /// Use `wget`.
    Wget,
    /// Use PowerShell (`pwsh`/`powershell`).
    PowerShell,
    /// Use Python `urllib`.
    Python3,
    /// Minimal HTTP/HTTPS tunnel: TCP for HTTP, OpenSSL (`openssl s_client`) for HTTPS.
    Tunnel,
    /// Plain HTTP/1.1 over a TCP socket only (no TLS).
    Tcp,
    /// HTTPS via OpenSSL only (`openssl s_client`).
    OpenSSL,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Controls forwarding of child stdout/stderr.
pub enum Quiet {
    /// Never be quiet: always forward child stdout/stderr to the parent process.
    Never,
    /// Always be quiet: never forward child stdout/stderr.
    Always,
    /// Only be quiet on success: forward output if the command fails.
    OnSuccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Response body content encoding (if known).
pub enum ContentEncoding {
    /// Gzip-compressed content.
    Gzip,
}

#[derive(Debug, Clone)]
/// Low-level download result prior to finalizing the output file.
pub struct DownloadResult {
    /// HTTP status code (best-effort).
    pub status_code: u16,
    /// Response content encoding, if known.
    pub content_encoding: Option<ContentEncoding>,
}

#[derive(Debug, Clone)]
/// Builder for a single download request.
pub struct RequestBuilder {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) preferred: Vec<Downloader>,
    pub(crate) follow_redirects: bool,
    pub(crate) quiet: Quiet,
}

impl RequestBuilder {
    /// Create a new request builder.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            preferred: Vec::new(),
            follow_redirects: true,
            quiet: Quiet::Always,
        }
    }

    /// Add an HTTP header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Prefer a specific downloader backend.
    pub fn preferred_downloader(mut self, preferred: Downloader) -> Self {
        self.preferred.push(preferred);
        self
    }

    /// Enable or disable HTTP redirect following.
    pub fn follow_redirects(mut self, follow_redirects: bool) -> Self {
        self.follow_redirects = follow_redirects;
        self
    }

    /// Control forwarding of child output.
    pub fn quiet(mut self, quiet: Quiet) -> Self {
        self.quiet = quiet;
        self
    }

    /// Fetch the response body as a String, blocking until the download is
    /// complete.
    pub fn fetch_string(self) -> Result<String, ResponseError> {
        String::from_utf8(self.fetch_bytes()?)
            .map_err(|e| ResponseError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    /// Fetch the response body into memory, blocking until the download is
    /// complete.
    pub fn fetch_bytes(self) -> Result<Vec<u8>, ResponseError> {
        url_parser::Url::new(&self.url)
            .map_err(|e| ResponseError::Start(StartError::Url(e.to_string())))?;

        let cancel = Arc::new(AtomicBool::new(false));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let memory_root = DownloadSink::buffer(buffer.clone());
        let join = self
            .start_first_backend(Arc::clone(&cancel), memory_root.clone())
            .map_err(ResponseError::Start)?;

        join.join().map_err(|_| ResponseError::ThreadPanicked)??;

        Ok(std::mem::take(&mut *buffer.lock().unwrap()))
    }

    /// Start the download in a background thread.
    pub fn start(self, target_path: impl AsRef<Path>) -> Result<RequestHandle, StartError> {
        // URL preflight: fail early with a message useful to callers.
        let _ = url_parser::Url::new(&self.url).map_err(|e| StartError::Url(e.to_string()))?;

        let target_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&target_path)?;

        let cancel = Arc::new(AtomicBool::new(false));
        let sink = DownloadSink::file(target_file);
        let join = self.start_first_backend(cancel.clone(), sink)?;

        Ok(RequestHandle {
            cancel,
            join: Some(join),
        })
    }

    /// Run [`candidate_downloaders`] once; `next_sink` prepares the body sink for each attempt.
    fn start_first_backend(
        &self,
        cancel: Arc<AtomicBool>,
        sink: DownloadSink,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let mut saw_non_not_found: Option<io::Error> = None;
        let mut saw_any_not_found = false;

        let request = Request {
            url: url_parser::Url::new(&self.url).map_err(|e| StartError::Url(e.to_string()))?,
            headers: self.headers.clone(),
            follow_redirects: self.follow_redirects,
            quiet: self.quiet,
        };

        for d in candidate_downloaders(&self.preferred) {
            match d
                .driver()
                .start(request.clone(), sink.clone(), Arc::clone(&cancel))
            {
                Ok(join) => return Ok(join),
                Err(StartError::Url(msg)) => return Err(StartError::Url(msg)),
                Err(StartError::NoDriverFound) => {
                    saw_any_not_found = true;
                    continue;
                }
                Err(StartError::IoError(e)) => {
                    if saw_non_not_found.is_none() {
                        saw_non_not_found = Some(e);
                    }
                    continue;
                }
            }
        }

        if let Some(e) = saw_non_not_found {
            return Err(StartError::IoError(e));
        }
        if saw_any_not_found {
            return Err(StartError::NoDriverFound);
        }
        Err(StartError::NoDriverFound)
    }
}

impl Downloader {
    pub(crate) fn driver(self) -> &'static dyn drivers::Driver {
        match self {
            Downloader::Curl => &drivers::curl::CurlDriver,
            Downloader::Wget => &drivers::wget::WgetDriver,
            Downloader::PowerShell => &drivers::powershell::PowerShellDriver,
            Downloader::Python3 => &drivers::python3::Python3Driver,
            Downloader::Tunnel => &drivers::tunnel::TunnelDriver,
            Downloader::Tcp => &drivers::tunnel::TcpDriver,
            Downloader::OpenSSL => &drivers::tunnel::OpenSslDriver,
        }
    }
}

#[derive(Debug)]
/// Handle for a running download.
pub struct RequestHandle {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<DownloadResult, ResponseError>>>,
}

impl RequestHandle {
    /// Request cancellation (best-effort).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Wait for completion and move the temp download to the target path.
    pub fn join(mut self) -> Result<Response, ResponseError> {
        let res = match self.join.take().expect("join called once").join() {
            Ok(r) => r,
            Err(_) => Err(ResponseError::ThreadPanicked),
        }?;

        Ok(Response {
            status_code: res.status_code,
        })
    }
}

impl Drop for RequestHandle {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.cancel.store(true, Ordering::SeqCst);
            // `tmp_path` will clean itself up via `Drop`.
        }
    }
}

#[derive(Debug, Clone)]
/// Final response metadata for a completed download.
pub struct Response {
    /// HTTP status code (best-effort).
    pub status_code: u16,
}

#[derive(Debug)]
/// Errors that can occur while starting a download.
pub enum StartError {
    /// No usable backend executable was found.
    NoDriverFound,
    /// A local I/O error occurred.
    IoError(io::Error),
    /// URL validation failed.
    Url(String),
}

impl From<io::Error> for StartError {
    fn from(value: io::Error) -> Self {
        Self::IoError(value)
    }
}

#[derive(Debug)]
/// Errors that can occur while running a request.
pub enum ResponseError {
    /// A local I/O error occurred.
    Io(io::Error),
    /// The URL could not be parsed.
    InvalidUrl,
    /// The URL scheme is unsupported.
    UnsupportedScheme,
    /// The request was cancelled.
    Cancelled,
    /// The worker thread panicked.
    ThreadPanicked,
    /// The backend command failed.
    CommandFailed {
        /// Backend program label.
        program: &'static str,
        /// Process exit code, if available.
        exit_code: Option<i32>,
        /// Captured stderr (best-effort).
        stderr: String,
    },
    /// The backend returned a non-numeric status code.
    BadStatusCode(String),
    /// Gzip decoding failed.
    GzipFailed {
        /// Process exit code, if available.
        exit_code: Option<i32>,
        /// Captured stderr (best-effort).
        stderr: String,
    },
    /// Download start failed.
    Start(StartError),
}

impl From<io::Error> for ResponseError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Choose downloaders in priority order.
fn candidate_downloaders(preferred: &[Downloader]) -> Vec<Downloader> {
    if !preferred.is_empty() {
        return preferred.to_vec();
    }
    vec![
        Downloader::Curl,
        Downloader::Wget,
        Downloader::PowerShell,
        Downloader::Python3,
        Downloader::Tunnel,
    ]
}
