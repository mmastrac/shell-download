use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, RequestBuilder, ResponseError, StartError, DownloadSink};

/// Backend driver interface.
pub(crate) trait Driver {
    /// Start a download and return a join handle for its result.
    /// On failure, returns the body [`DownloadSink`] unchanged so callers can retry or discard.
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<
        JoinHandle<Result<DownloadResult, ResponseError>>,
        (StartError, DownloadSink),
    >;
}

pub(crate) mod curl;
pub(crate) mod openssl;
pub(crate) mod powershell;
pub(crate) mod python3;
pub(crate) mod wget;
