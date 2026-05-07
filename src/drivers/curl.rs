use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
/// `curl` backend.
pub(crate) struct CurlDriver;

impl Driver for CurlDriver {
    /// Start a download using `curl`.
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let mut cmd = Command::new("curl");
        cmd.arg("-sS")
            .arg("--compressed")
            .arg("-o")
            .arg("-")
            .arg("-w")
            .arg("%{stderr}%{http_code}")
            .arg(&req.url);

        if req.follow_redirects {
            cmd.arg("-L");
        }

        for (k, v) in util::add_common_headers(&req) {
            cmd.arg("-H").arg(format!("{k}: {v}"));
        }

        util::spawn_download_cmd_thread(
            cmd,
            "curl",
            req,
            sink,
            cancel,
            move |output, _req| {
                let code_str = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let code: u16 = code_str
                    .parse()
                    .map_err(|_| ResponseError::BadStatusCode(code_str))?;
                Ok((code, None))
            },
        )
    }
}
