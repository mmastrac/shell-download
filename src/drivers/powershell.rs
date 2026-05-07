use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{Quiet, RequestBuilder, Response, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
pub(crate) struct PwshDriver;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PowerShellDriver;

impl Driver for PwshDriver {
    fn start(
        &self,
        req: RequestBuilder,
        target_path: std::path::PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<Response, ResponseError>>, StartError> {
        start_inner(req, target_path, cancel, true)
    }
}

impl Driver for PowerShellDriver {
    fn start(
        &self,
        req: RequestBuilder,
        target_path: std::path::PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<Response, ResponseError>>, StartError> {
        start_inner(req, target_path, cancel, false)
    }
}

fn start_inner(
    req: RequestBuilder,
    target_path: std::path::PathBuf,
    cancel: Arc<AtomicBool>,
    use_pwsh: bool,
) -> Result<JoinHandle<Result<Response, ResponseError>>, StartError> {
    let program: &'static str = if use_pwsh { "pwsh" } else { "powershell" };

    let tmp_path = util::tmp_path_for_target(&target_path);

    let mut ps_headers = String::new();
    for (k, v) in util::add_common_headers(&req) {
        ps_headers.push_str(&format!("'{}'='{}';", escape_ps(&k), escape_ps(&v)));
    }
    let headers_expr = format!("@{{{ps_headers}}}");
    let url = escape_ps(&req.url);
    let out_str = escape_ps(&tmp_path.to_string_lossy());
    let max_redir = if req.follow_redirects { 10 } else { 0 };

    let debug = match req.quiet {
        Quiet::Never => "",
        Quiet::Always | Quiet::OnSuccess => {
            "[Console]::Error.WriteLine(\"shell-download(powershell): starting request\");\
             [Console]::Error.WriteLine(\"  uri={0}\" -f $u);\
             [Console]::Error.WriteLine(\"  out={0}\" -f $o);\
             [Console]::Error.WriteLine(\"  max_redir={0}\" -f $mr);\
             [Console]::Error.WriteLine(\"  ps={0}\" -f $PSVersionTable.PSVersion);"
        }
    };

    let script = format!(
        "$ProgressPreference='SilentlyContinue';\
         $h={headers_expr};\
         $u='{url}';\
         $o='{out_str}';\
         $mr={max_redir};\
         {debug}\
         try {{\
           $r=Invoke-WebRequest -Uri $u -Headers $h -OutFile $o -PassThru -MaximumRedirection $mr -ErrorAction Stop {basic};\
           $sc=$r.StatusCode;\
           if ($null -eq $sc) {{ $sc=0 }};\
           if ($sc -is [int]) {{ [Console]::Out.Write($sc) }} else {{ [Console]::Out.Write($sc.value__) }};\
           exit 0;\
         }} catch {{\
           [Console]::Error.WriteLine(\"shell-download(powershell): request failed\");\
           [Console]::Error.WriteLine($_.ToString());\
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

    let child = util::spawn_child_for_output(cmd, program)?;

    Ok(util::spawn_request_thread(
        req,
        target_path,
        tmp_path,
        cancel,
        move |req, _out, cancel| {
            let output = util::wait_child_with_output(child, cancel, program, req.quiet)?;
            let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let code: u16 = code_str
                .parse()
                .map_err(|_| ResponseError::BadStatusCode(code_str))?;
            Ok((code, false))
        },
    ))
}

fn escape_ps(s: &str) -> String {
    s.replace('\'', "''")
}
