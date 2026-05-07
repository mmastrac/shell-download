use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::AtomicBool,
    Arc,
};

use crate::{drivers::Driver, util, Error, RequestBuilder};

#[derive(Debug, Clone, Copy)]
pub(crate) struct PwshDriver;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PowerShellDriver;

impl Driver for PwshDriver {
    fn download(
        &self,
        req: &RequestBuilder,
        out: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(u16, bool), Error> {
        download_inner(req, out, cancel, true)
    }
}

impl Driver for PowerShellDriver {
    fn download(
        &self,
        req: &RequestBuilder,
        out: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(u16, bool), Error> {
        download_inner(req, out, cancel, false)
    }
}

fn download_inner(
    req: &RequestBuilder,
    out: &Path,
    cancel: &Arc<AtomicBool>,
    use_pwsh: bool,
) -> Result<(u16, bool), Error> {
    let program: &'static str = if use_pwsh { "pwsh" } else { "powershell" };

    let mut ps_headers = String::new();
    for (k, v) in util::add_common_headers(req) {
        ps_headers.push_str(&format!("'{}'='{}';", escape_ps(&k), escape_ps(&v)));
    }
    let headers_expr = format!("@{{{ps_headers}}}");
    let url = escape_ps(&req.url);
    let out_str = escape_ps(&out.to_string_lossy());
    let max_redir = if req.follow_redirects { 10 } else { 0 };

    let script = format!(
        "$ProgressPreference='SilentlyContinue';\
         $h={headers_expr};\
         try {{\
           $r=Invoke-WebRequest -Uri '{url}' -Headers $h -OutFile '{out_str}' -MaximumRedirection {max_redir} -ErrorAction Stop {basic};\
           $sc=$r.StatusCode;\
           if ($null -eq $sc) {{ $sc=0 }};\
           if ($sc -is [int]) {{ [Console]::Out.Write($sc) }} else {{ [Console]::Out.Write($sc.value__) }};\
           exit 0;\
         }} catch {{\
           Write-Error $_;\
           exit 1;\
         }}",
        basic = if use_pwsh { "" } else { "-UseBasicParsing" }
    );

    let mut cmd = Command::new(program);
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(script);

    let output = util::run_cancellable_command(cmd, cancel, program)?;
    let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let code: u16 = code_str.parse().map_err(|_| Error::BadStatusCode(code_str))?;
    Ok((code, false))
}

fn escape_ps(s: &str) -> String {
    s.replace('\'', "''")
}

