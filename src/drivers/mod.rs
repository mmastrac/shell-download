use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{RequestBuilder, Response, ResponseError, StartError};

pub(crate) trait Driver {
    fn start(
        &self,
        req: RequestBuilder,
        target_path: PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<Response, ResponseError>>, StartError>;
}

pub(crate) mod curl;
pub(crate) mod openssl;
pub(crate) mod powershell;
pub(crate) mod wget;
