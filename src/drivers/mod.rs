use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, DownloadSink, Quiet, ResponseError, StartError, url_parser::Url};

/// Backend driver interface.
pub(crate) trait Driver {
    /// Start a download and return a join handle for its result.
    fn start(
        &self,
        req: Request,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError>;
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub follow_redirects: bool,
    pub quiet: Quiet,
}

pub(crate) mod curl;
pub(crate) mod powershell;
pub(crate) mod python3;
pub(crate) mod tunnel;
pub(crate) mod wget;
