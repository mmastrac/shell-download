use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
pub(crate) struct WgetDriver;

impl Driver for WgetDriver {
    fn start(
        &self,
        req: RequestBuilder,
        out_path: std::path::PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let mut cmd = Command::new("wget");
        cmd.arg("-O")
            .arg(&out_path)
            .arg("--server-response")
            .arg(&req.url);
        if !req.follow_redirects {
            cmd.arg("--max-redirect=0");
        }
        for (k, v) in util::add_common_headers(&req) {
            cmd.arg("--header").arg(format!("{k}: {v}"));
        }

        let child = util::spawn_child_for_output(cmd, "wget")?;

        Ok(util::spawn_download_thread(
            req,
            out_path,
            cancel,
            move |req, _out, cancel| {
                let output = util::wait_child_with_output(child, cancel, "wget", req.quiet)?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut last_code: Option<u16> = None;
                for line in stderr.lines() {
                    let line = line.trim();
                    if let Some(rest) = line.strip_prefix("HTTP/") {
                        let parts: Vec<&str> = rest.split_whitespace().collect();
                        if parts.len() >= 2 {
                            if let Ok(code) = parts[1].parse::<u16>() {
                                last_code = Some(code);
                            }
                        }
                    }
                }
                Ok((last_code.unwrap_or(200), false))
            },
        ))
    }
}
