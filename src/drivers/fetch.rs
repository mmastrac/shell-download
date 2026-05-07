use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::AtomicBool,
    Arc,
};

use crate::{drivers::Driver, util, Error, RequestBuilder};

#[derive(Debug, Clone, Copy)]
pub(crate) struct FetchDriver;

impl Driver for FetchDriver {
    fn download(
        &self,
        req: &RequestBuilder,
        out: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(u16, bool), Error> {
        let mut cmd = Command::new("fetch");
        cmd.arg("-q").arg("-o").arg(out).arg(&req.url);
        let _ = util::add_common_headers(req);
        let _output = util::run_cancellable_command(cmd, cancel, "fetch", req.quiet)?;
        Ok((200, false))
    }
}

