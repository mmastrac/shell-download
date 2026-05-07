mod drivers;
mod url_parser;
mod util;

use std::io;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Downloader {
    Curl,
    Wget,
    PowerShell,
    OpenSsl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// Never be quiet: always forward child stdout/stderr to the parent process.
    Never,
    /// Always be quiet: never forward child stdout/stderr.
    Always,
    /// Only be quiet on success: forward output if the command fails.
    OnSuccess,
}

#[derive(Debug, Clone)]
pub struct RequestBuilder {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) preferred: Vec<Downloader>,
    pub(crate) follow_redirects: bool,
    pub(crate) quiet: Quiet,
}

impl RequestBuilder {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            preferred: Vec::new(),
            follow_redirects: true,
            quiet: Quiet::Always,
        }
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn preferred_downloader(mut self, preferred: Downloader) -> Self {
        self.preferred.push(preferred);
        self
    }

    pub fn follow_redirects(mut self, follow_redirects: bool) -> Self {
        self.follow_redirects = follow_redirects;
        self
    }

    pub fn quiet(mut self, quiet: Quiet) -> Self {
        self.quiet = quiet;
        self
    }

    pub fn start(self, target_path: impl AsRef<Path>) -> Result<RequestHandle, StartError> {
        let target_path = target_path.as_ref().to_path_buf();

        if let Some(parent) = target_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(StartError::IoError)?;
            }
        }

        let _ = std::fs::remove_file(&target_path);

        // URL preflight: fail early with a message useful to callers.
        url_parser::Url::new(&self.url).map_err(|e| StartError::Url(e.to_string()))?;

        let cancel = Arc::new(AtomicBool::new(false));
        let mut saw_non_not_found: Option<io::Error> = None;
        let mut saw_any_not_found = false;

        for d in candidate_downloaders(&self.preferred) {
            match d
                .driver()
                .start(self.clone(), target_path.clone(), Arc::clone(&cancel))
            {
                Ok(join) => {
                    return Ok(RequestHandle {
                        cancel,
                        join: Some(join),
                    });
                }
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
                Err(StartError::Url(msg)) => return Err(StartError::Url(msg)),
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
        static CURL: drivers::curl::CurlDriver = drivers::curl::CurlDriver;
        static WGET: drivers::wget::WgetDriver = drivers::wget::WgetDriver;
        static POWERSHELL: drivers::powershell::PowerShellDriver =
            drivers::powershell::PowerShellDriver;
        static OPENSSL: drivers::openssl::OpenSslDriver = drivers::openssl::OpenSslDriver;

        match self {
            Downloader::Curl => &CURL,
            Downloader::Wget => &WGET,
            Downloader::PowerShell => &POWERSHELL,
            Downloader::OpenSsl => &OPENSSL,
        }
    }
}

#[derive(Debug)]
pub struct RequestHandle {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<Response, ResponseError>>>,
}

impl RequestHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn join(mut self) -> Result<Response, ResponseError> {
        match self.join.take().expect("join called once").join() {
            Ok(r) => r,
            Err(_) => Err(ResponseError::ThreadPanicked),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status_code: u16,
}

#[derive(Debug)]
pub enum StartError {
    NoDriverFound,
    IoError(io::Error),
    Url(String),
}

impl From<io::Error> for StartError {
    fn from(value: io::Error) -> Self {
        Self::IoError(value)
    }
}

#[derive(Debug)]
pub enum ResponseError {
    Io(io::Error),
    InvalidUrl,
    UnsupportedScheme,
    Cancelled,
    ThreadPanicked,
    CommandFailed {
        program: &'static str,
        exit_code: Option<i32>,
        stderr: String,
    },
    BadStatusCode(String),
    GzipFailed {
        exit_code: Option<i32>,
        stderr: String,
    },
    Start(StartError),
}

impl From<io::Error> for ResponseError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

fn candidate_downloaders(preferred: &[Downloader]) -> Vec<Downloader> {
    if !preferred.is_empty() {
        return preferred.to_vec();
    }
    vec![
        Downloader::Curl,
        Downloader::Wget,
        Downloader::PowerShell,
        Downloader::OpenSsl,
    ]
}
