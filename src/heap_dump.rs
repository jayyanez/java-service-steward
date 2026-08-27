// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Local;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::jvm::JvmCommand;

const MAX_DIAGNOSTIC_OUTPUT: usize = 4096;
const MAX_NAME_ATTEMPTS: u32 = 1000;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

#[derive(Debug)]
pub enum Completion {
    Created { path: PathBuf, bytes: u64 },
    Failed { path: PathBuf, message: String },
}

pub enum Poll {
    Pending,
    Complete(Completion),
}

pub struct Task {
    path: PathBuf,
    timeout: Duration,
    receiver: crossbeam_channel::Receiver<Completion>,
}

impl Task {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Upper bound on how long the worker may run before it gives up.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn poll(&self) -> Poll {
        match self.receiver.try_recv() {
            Ok(completion) => Poll::Complete(completion),
            Err(crossbeam_channel::TryRecvError::Empty) => Poll::Pending,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Poll::Complete(Completion::Failed {
                    path: self.path.clone(),
                    message: "the heap-dump worker ended without a result".into(),
                })
            }
        }
    }
}

#[must_use]
pub fn timeout(config: &Config) -> Duration {
    Duration::from_secs(
        config
            .get_u64("jss.heapdump.timeout", DEFAULT_TIMEOUT_SECONDS)
            .max(1),
    )
}

pub fn start(
    config: &Config,
    command: &JvmCommand,
    logfile: &Path,
    pid: u32,
    jvm_id: u32,
) -> Result<Task> {
    crate::diagnostics::require_attach(&command.arguments)?;
    let jcmd = crate::diagnostics::jcmd(
        &command.program,
        &command.environment,
        &command.working_directory,
    )?;
    let directory = config.get("jss.heapdump.directory").map_or_else(
        || default_directory(config, logfile),
        |path| config.resolve_path(path),
    );
    fs::create_dir_all(&directory)?;
    let path = unique_path(&directory, pid, jvm_id)?;
    let timeout = timeout(config);

    let (sender, receiver) = crossbeam_channel::bounded(1);
    let environment = command.environment.clone();
    let worker_path = path.clone();
    let worker_directory = command.working_directory.clone();
    thread::Builder::new()
        .name("jss-heap-dump".into())
        .spawn(move || {
            let completion = run_jcmd(
                &jcmd,
                &environment,
                worker_directory,
                pid,
                worker_path,
                timeout,
            );
            let _ = sender.send(completion);
        })?;
    Ok(Task {
        path,
        timeout,
        receiver,
    })
}

fn default_directory(config: &Config, logfile: &Path) -> PathBuf {
    logfile
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || config.executable_directory().to_path_buf(),
            Path::to_path_buf,
        )
}

fn unique_path(directory: &Path, pid: u32, jvm_id: u32) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let path = directory.join(format!(
            "heap-{timestamp}-jvm{jvm_id}-pid{pid}{suffix}.hprof"
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(Error::Config(format!(
        "could not allocate a unique heap-dump name in {}",
        directory.display()
    )))
}

fn run_jcmd(
    jcmd: &Path,
    environment: &[(OsString, OsString)],
    working_directory: PathBuf,
    pid: u32,
    path: PathBuf,
    timeout: Duration,
) -> Completion {
    let spawned = Command::new(jcmd)
        .arg(pid.to_string())
        .arg("GC.heap_dump")
        .arg(&path)
        .current_dir(working_directory)
        .envs(environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            return failed(
                path,
                format!("could not execute {}: {error}", jcmd.display()),
            );
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                return failed(path, format!("could not wait for jcmd: {error}"));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return failed(
                path,
                format!(
                    "{} did not finish the heap dump within {} seconds",
                    jcmd.display(),
                    timeout.as_secs()
                ),
            );
        }
        thread::sleep(Duration::from_millis(100));
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if status.success() {
        match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > 0 => Completion::Created {
                path,
                bytes: metadata.len(),
            },
            Ok(_) => failed(
                path,
                "jcmd reported success but created an empty heap dump".into(),
            ),
            Err(error) => failed(
                path,
                format!("jcmd reported success but no heap dump was found: {error}"),
            ),
        }
    } else {
        failed(
            path,
            format!(
                "jcmd exited with {status}: {}",
                diagnostic_output(&stdout, &stderr)
            ),
        )
    }
}

fn read_all(stream: Option<impl Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        let _ = stream.read_to_end(&mut bytes);
    }
    bytes
}

fn failed(path: PathBuf, message: String) -> Completion {
    let _ = fs::remove_file(&path);
    Completion::Failed { path, message }
}

fn diagnostic_output(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = if stderr.is_empty() { stdout } else { stderr };
    let text = String::from_utf8_lossy(combined);
    let mut output: String = text.chars().take(MAX_DIAGNOSTIC_OUTPUT).collect();
    output = output.trim().replace(['\r', '\n'], " ");
    if output.is_empty() {
        "no diagnostic output".into()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::unique_path;
    use std::fs;

    #[test]
    fn heap_dump_names_never_overwrite_an_existing_file() {
        let directory = crate::test_support::unique_directory("heap-name");
        let first = unique_path(&directory, 123, 4).expect("allocate first path");
        fs::write(&first, b"existing").expect("reserve first path");
        let second = unique_path(&directory, 123, 4).expect("allocate second path");
        assert_ne!(first, second);
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
