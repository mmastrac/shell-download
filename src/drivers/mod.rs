use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

use crate::{Error, RequestBuilder};

pub(crate) trait Driver {
    fn download(
        &self,
        req: &RequestBuilder,
        out: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(u16, bool), Error>;
}

pub(crate) mod curl;
pub(crate) mod fetch;
pub(crate) mod openssl;
pub(crate) mod powershell;
pub(crate) mod wget;
