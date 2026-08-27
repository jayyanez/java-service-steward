// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::{Error, Result};

const OUTPUT_CAPACITY: usize = 256;
const MAX_OUTPUT_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    ConsoleBreak,
    Jcmd,
}

pub fn method(config: &Config, arguments: &[OsString]) -> Result<Method> {
    match config
        .get_or("jss.threaddump.method", "AUTO")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "AUTO" => Ok(if has_reduced_signal_usage(arguments) {
            Method::Jcmd
        } else {
            Method::ConsoleBreak
        }),
        "BREAK" | "CTRL_BREAK" | "CONSOLE_BREAK" if has_reduced_signal_usage(arguments) => {
            Err(Error::DiagnosticUnavailable(
                "the JVM was launched with -Xrs, which disables Windows CTRL_BREAK thread dumps; \
                 remove -Xrs or use AUTO/JCMD with the matching JDK diagnostic tool"
                    .into(),
            ))
        }
        "BREAK" | "CTRL_BREAK" | "CONSOLE_BREAK" => Ok(Method::ConsoleBreak),
        "JCMD" => Ok(Method::Jcmd),
        value => Err(Error::Config(format!(
            "jss.threaddump.method must be AUTO, BREAK, or JCMD; found {value}"
        ))),
    }
}

pub fn capture_with_jcmd(
    java_program: &Path,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    working_directory: &Path,
    pid: u32,
    timeout: Duration,
    mut record: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    crate::diagnostics::require_attach(arguments)?;
    let jcmd = crate::diagnostics::jcmd(java_program, environment, working_directory)?;

    let mut child = Command::new(&jcmd)
        .arg(pid.to_string())
        .arg("Thread.print")
        .arg("-l")
        .current_dir(working_directory)
        .envs(environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::Config(format!("could not execute {}: {error}", jcmd.display())))?;

    let (sender, receiver) = crossbeam_channel::bounded(OUTPUT_CAPACITY);
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, sender.clone());
    }
    drop(sender);

    let deadline = Instant::now() + timeout.max(Duration::from_secs(1));
    let mut output_disconnected = false;
    let status = loop {
        if output_disconnected {
            thread::sleep(Duration::from_millis(25));
        } else {
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(line) => record(&line)?,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    output_disconnected = true;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
        }
        if let Some(status) = child.try_wait()? {
            while let Ok(line) = receiver.recv_timeout(Duration::from_millis(25)) {
                record(&line)?;
            }
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Config(format!(
                "{} did not finish the thread dump within {} seconds",
                jcmd.display(),
                timeout.as_secs().max(1)
            )));
        }
    };

    if status.success() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "{} exited with {status} while requesting a thread dump",
            jcmd.display()
        )))
    }
}

fn has_reduced_signal_usage(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| argument.to_string_lossy().eq_ignore_ascii_case("-Xrs"))
}

fn spawn_reader(
    reader: impl std::io::Read + Send + 'static,
    sender: crossbeam_channel::Sender<Vec<u8>>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        while let Ok(buffer) = reader.fill_buf() {
            if buffer.is_empty() {
                break;
            }
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(buffer.len(), |index| index + 1);
            let mut line = buffer[..consumed.min(MAX_OUTPUT_LINE)].to_vec();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            reader.consume(consumed);
            if sender.send(line).is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Method, method};
    use crate::config::Config;
    use std::ffi::OsString;
    use std::fs;

    #[test]
    fn auto_uses_jcmd_only_when_reduced_signal_usage_is_enabled() {
        let directory =
            std::env::temp_dir().join(format!("jss-thread-method-test-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.java.command=java\n").expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(
            method(&config, &[]).expect("default method"),
            Method::ConsoleBreak
        );
        assert_eq!(
            method(&config, &[OsString::from("-Xrs")]).expect("Xrs method"),
            Method::Jcmd
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn configured_method_can_force_either_backend() {
        let directory = std::env::temp_dir().join(format!(
            "jss-thread-method-override-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.java.command=java\njss.threaddump.method=JCMD\n",
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(method(&config, &[]).expect("forced method"), Method::Jcmd);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn forced_break_rejects_xrs_with_an_actionable_capability_error() {
        let directory =
            std::env::temp_dir().join(format!("jss-thread-break-xrs-test-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.java.command=java\njss.threaddump.method=BREAK\n",
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let error =
            method(&config, &[OsString::from("-Xrs")]).expect_err("BREAK cannot work with -Xrs");
        assert!(error.to_string().contains("disables Windows CTRL_BREAK"));
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
