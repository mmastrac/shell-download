use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CurlDriver;

impl Driver for CurlDriver {
    fn start(
        &self,
        req: RequestBuilder,
        out_path: std::path::PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let mut cmd = Command::new("curl");
        cmd.arg("-sS")
            .arg("--compressed")
            .arg("-o")
            .arg(&out_path)
            .arg("-w")
            .arg("%{http_code}")
            .arg(&req.url);

        if req.follow_redirects {
            cmd.arg("-L");
        }

        for (k, v) in util::add_common_headers(&req) {
            cmd.arg("-H").arg(format!("{k}: {v}"));
        }

        let child = util::spawn_child_for_output(cmd, "curl")?;

        Ok(util::spawn_download_thread(
            req,
            out_path,
            cancel,
            move |req, _out, cancel| {
                let output = util::wait_child_with_output(child, cancel, "curl", req.quiet)?;
                let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let code: u16 = code_str
                    .parse()
                    .map_err(|_| ResponseError::BadStatusCode(code_str))?;
                Ok((code, false))
            },
        ))
    }
}
