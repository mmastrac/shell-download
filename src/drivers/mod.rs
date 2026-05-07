use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, RequestBuilder, ResponseError, StartError};

pub(crate) trait Driver {
    fn start(
        &self,
        req: RequestBuilder,
        out_path: PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError>;
}

pub(crate) mod curl;
pub(crate) mod openssl;
pub(crate) mod powershell;
pub(crate) mod wget;
