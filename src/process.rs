//! Subprocess helpers with piped stdio.

use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

fn missing_pipe(name: &'static str) -> io::Error {
    io::Error::other(format!("missing child {name}"))
}

/// Spawns `cmd` with stdin set to [`Stdio::null`], stdout and stderr as pipes.
///
/// Overwrites any prior `stdin` / `stdout` / `stderr` on `cmd`.
///
/// Used by backends that only capture stdout/stderr; see also [`spawn_stdin_stdout_stderr`].
#[allow(dead_code)]
pub(crate) fn spawn_stdout_stderr(
    cmd: &mut Command,
) -> io::Result<(Child, ChildStdout, ChildStderr)> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| missing_pipe("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing_pipe("stderr"))?;
    Ok((child, stdout, stderr))
}

/// Spawns `cmd` with stdin, stdout, and stderr as pipes.
///
/// Overwrites any prior `stdin` / `stdout` / `stderr` on `cmd`.
pub(crate) fn spawn_stdin_stdout_stderr(
    cmd: &mut Command,
) -> io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().ok_or_else(|| missing_pipe("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing_pipe("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing_pipe("stderr"))?;
    Ok((child, stdin, stdout, stderr))
}
