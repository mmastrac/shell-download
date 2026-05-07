use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError};

/// Backend driver interface.
pub(crate) trait Driver {
    /// Start a download and return a join handle for its result.
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError>;
}

pub(crate) mod curl;
pub(crate) mod http11;
pub(crate) mod openssl;
pub(crate) mod powershell;
pub(crate) mod python3;
pub(crate) mod tcp;
pub(crate) mod wget;
