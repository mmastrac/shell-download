mod drivers;
mod util;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Downloader {
    Curl,
    Wget,
    PowerShell,
    Pwsh,
    Fetch,
    OpenSsl,
}

#[derive(Debug, Clone)]
pub struct RequestBuilder {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) preferred: Option<Downloader>,
    pub(crate) follow_redirects: bool,
}

impl RequestBuilder {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            preferred: None,
            follow_redirects: true,
        }
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn preferred_downloader(mut self, preferred: Downloader) -> Self {
        self.preferred = Some(preferred);
        self
    }

    pub fn follow_redirects(mut self, follow_redirects: bool) -> Self {
        self.follow_redirects = follow_redirects;
        self
    }

    pub fn start(self, target_path: impl AsRef<Path>) -> Result<RequestHandle, Error> {
        let target_path = target_path.as_ref().to_path_buf();

        if let Some(parent) = target_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }
        }

        let _ = std::fs::remove_file(&target_path);

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel2 = Arc::clone(&cancel);

        let join = thread::spawn(move || run_request(self, target_path, cancel2));

        Ok(RequestHandle {
            cancel,
            join: Some(join),
        })
    }
}

impl Downloader {
    pub(crate) fn driver(self) -> &'static dyn drivers::Driver {
        static CURL: drivers::curl::CurlDriver = drivers::curl::CurlDriver;
        static WGET: drivers::wget::WgetDriver = drivers::wget::WgetDriver;
        static PWSH: drivers::powershell::PwshDriver = drivers::powershell::PwshDriver;
        static POWERSHELL: drivers::powershell::PowerShellDriver = drivers::powershell::PowerShellDriver;
        static FETCH: drivers::fetch::FetchDriver = drivers::fetch::FetchDriver;
        static OPENSSL: drivers::openssl::OpenSslDriver = drivers::openssl::OpenSslDriver;

        match self {
            Downloader::Curl => &CURL,
            Downloader::Wget => &WGET,
            Downloader::Pwsh => &PWSH,
            Downloader::PowerShell => &POWERSHELL,
            Downloader::Fetch => &FETCH,
            Downloader::OpenSsl => &OPENSSL,
        }
    }
}

#[derive(Debug)]
pub struct RequestHandle {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<Response, Error>>>,
}

impl RequestHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn join(mut self) -> Result<Response, Error> {
        match self.join.take().expect("join called once").join() {
            Ok(r) => r,
            Err(_) => Err(Error::ThreadPanicked),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status_code: u16,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidUrl,
    UnsupportedScheme,
    NoDownloader,
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
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

fn run_request(
    req: RequestBuilder,
    target_path: PathBuf,
    cancel: Arc<AtomicBool>,
) -> Result<Response, Error> {
    let downloader = pick_downloader(req.preferred)?;
    let driver = downloader.driver();

    let mut tmp = target_path.clone();
    tmp.set_extension(format!(
        "{}.tmp",
        util::unique_suffix().unwrap_or_else(|| "download".into())
    ));
    let _ = std::fs::remove_file(&tmp);

    let (status_code, content_encoding_gzip) = driver.download(&req, &tmp, &cancel)?;

    if cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Cancelled);
    }

    let needs_gunzip = content_encoding_gzip || util::file_looks_gzipped(&tmp).unwrap_or(false);
    if needs_gunzip {
        util::gunzip_to_target(&tmp, &target_path)?;
        let _ = std::fs::remove_file(&tmp);
    } else {
        let _ = std::fs::remove_file(&target_path);
        std::fs::rename(&tmp, &target_path).map_err(Error::Io)?;
    }

    Ok(Response { status_code })
}

fn pick_downloader(preferred: Option<Downloader>) -> Result<Downloader, Error> {
    if let Some(p) = preferred {
        // Preferred downloader is intended for deterministic behavior (e.g. tests).
        // If it's not available, the request should fail rather than silently falling back.
        return Ok(p);
    }

    for d in [
        Downloader::Curl,
        Downloader::Wget,
        Downloader::Pwsh,
        Downloader::PowerShell,
        Downloader::Fetch,
        Downloader::OpenSsl,
    ] {
        if downloader_available(d) {
            return Ok(d);
        }
    }

    Err(Error::NoDownloader)
}

fn downloader_available(d: Downloader) -> bool {
    match d {
        Downloader::Curl => util::downloader_available("curl"),
        Downloader::Wget => util::downloader_available("wget"),
        Downloader::PowerShell => util::downloader_available("powershell"),
        Downloader::Pwsh => util::downloader_available("pwsh"),
        Downloader::Fetch => util::downloader_available("fetch"),
        Downloader::OpenSsl => util::downloader_available("openssl"),
    }
}
