use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::AtomicBool,
    Arc,
};

use crate::{drivers::Driver, util, Error, RequestBuilder};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CurlDriver;

impl Driver for CurlDriver {
    fn download(
        &self,
        req: &RequestBuilder,
        out: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(u16, bool), Error> {
        let mut cmd = Command::new("curl");
        cmd.arg("-sS")
            .arg("--compressed")
            .arg("-o")
            .arg(out)
            .arg("-w")
            .arg("%{http_code}")
            .arg(&req.url);

        if req.follow_redirects {
            cmd.arg("-L");
        }

        for (k, v) in util::add_common_headers(req) {
            cmd.arg("-H").arg(format!("{k}: {v}"));
        }

        let output = util::run_cancellable_command(cmd, cancel, "curl", req.quiet)?;
        let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let code: u16 = code_str.parse().map_err(|_| Error::BadStatusCode(code_str))?;
        Ok((code, false))
    }
}

