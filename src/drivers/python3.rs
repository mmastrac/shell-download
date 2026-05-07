use std::path::Path;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
/// `python3` backend using `urllib`.
pub(crate) struct Python3Driver;

impl Driver for Python3Driver {
    /// Start a download using Python `urllib`.
    fn start(
        &self,
        req: RequestBuilder,
        out_path: &Path,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        // Prefer python3, then python.
        let candidates: [(&str, &str); 2] = [("python3", "python3"), ("python", "python")];
        let mut exe: Option<&'static str> = None;
        for (label, program) in candidates {
            if !util::find_program_in_path(program).is_empty() {
                // SAFETY: label is a 'static string from the array above.
                exe = Some(match label {
                    "python3" => "python3",
                    _ => "python",
                });
                break;
            }
        }
        let exe = exe.ok_or(StartError::NoDriverFound)?;

        // If we fall back to `python`, ensure it's Python 3.x.
        if exe == "python" {
            let out = Command::new("python")
                .arg("--version")
                .output()
                .map_err(StartError::IoError)?;
            let mut s = String::new();
            s.push_str(&String::from_utf8_lossy(&out.stdout));
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            if !s.trim_start().starts_with("Python 3") {
                return Err(StartError::NoDriverFound);
            }
        }

        // Keep the script conservative for broad Python 3.x compatibility (close to 3.0).
        //
        // argv:
        //   1: url
        //   2: out_path
        //   3: follow_redirects ("1" or "0")
        //   4..: headers as "Key: Value"
        //
        // stdout: status code (as digits)
        let script = r#"
import sys
try:
    import urllib2 as _u
except ImportError:
    import urllib.request as _u
    import urllib.error as _ue

def _main(argv):
    url = argv[1]
    out_path = argv[2]
    follow = argv[3] == "1"
    headers = argv[4:]

    class NoRedirect(_u.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, hdrs, newurl):
            return None

    opener = _u.build_opener() if follow else _u.build_opener(NoRedirect)
    req = _u.Request(url)
    for hv in headers:
        try:
            k, v = hv.split(":", 1)
            req.add_header(k.strip(), v.strip())
        except Exception:
            pass

    try:
        resp = opener.open(req)
        code = getattr(resp, "getcode", lambda: 200)()
        body = resp.read()
    except Exception as e:
        # Try to surface HTTP status codes for errors that carry a response.
        code = None
        r = getattr(e, "read", None)
        if r is not None:
            try:
                body = e.read()
            except Exception:
                body = b""
            code = getattr(e, "code", None)
        else:
            body = b""
        if code is None:
            sys.stderr.write(str(e) + "\n")
            return 1

    f = open(out_path, "wb")
    try:
        f.write(body)
    finally:
        f.close()

    sys.stdout.write(str(int(code)))
    return 0

sys.exit(_main(sys.argv))
"#;

        let mut cmd = Command::new(exe);
        cmd.arg("-c")
            .arg(script)
            .arg(&req.url)
            .arg(&out_path)
            .arg(if req.follow_redirects { "1" } else { "0" });

        for (k, v) in util::add_common_headers(&req) {
            cmd.arg(format!("{k}: {v}"));
        }

        let child = util::spawn_child_for_output(cmd, exe)?;

        Ok(util::spawn_download_thread(
            req,
            out_path,
            cancel,
            move |req, _out, cancel| {
                let output = util::wait_child_with_output(child, cancel, exe, req.quiet)?;
                let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let code: u16 = code_str
                    .parse()
                    .map_err(|_| ResponseError::BadStatusCode(code_str))?;
                Ok((code, None))
            },
        ))
    }
}
