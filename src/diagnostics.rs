// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const ATTACH_DISABLED: &str = "-XX:+DisableAttachMechanism";
const ATTACH_ENABLED: &str = "-XX:-DisableAttachMechanism";

pub fn jcmd(
    java_program: &Path,
    environment: &[(OsString, OsString)],
    working_directory: &Path,
) -> Result<PathBuf> {
    let executable = if cfg!(windows) { "jcmd.exe" } else { "jcmd" };
    if java_program.components().count() > 1 {
        let candidate = java_program.with_file_name(executable);
        return candidate.is_file().then_some(candidate).ok_or_else(|| {
            Error::DiagnosticUnavailable(format!(
                "{executable} was not found next to the configured Java runtime: {}; \
                 core service supervision remains available, but JCMD thread dumps and \
                 on-demand heap dumps require the matching JDK diagnostic tool",
                java_program.display()
            ))
        });
    }

    let path = environment
        .iter()
        .rev()
        .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value);
    if let Some(path) = path {
        for directory in std::env::split_paths(path) {
            let candidate = if directory.as_os_str().is_empty() {
                working_directory.join(executable)
            } else if directory.is_absolute() {
                directory.join(executable)
            } else {
                working_directory.join(directory).join(executable)
            };
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(Error::DiagnosticUnavailable(format!(
        "{executable} was not found in the PATH used by the configured Java runtime; \
         core service supervision remains available, but JCMD thread dumps and \
         on-demand heap dumps require the matching JDK diagnostic tool"
    )))
}

pub fn require_attach(arguments: &[OsString]) -> Result<()> {
    let disabled = arguments.iter().rev().find_map(|argument| {
        let argument = argument.to_string_lossy();
        if argument == ATTACH_DISABLED {
            Some(true)
        } else if argument == ATTACH_ENABLED {
            Some(false)
        } else {
            None
        }
    });
    if disabled == Some(true) {
        Err(Error::DiagnosticUnavailable(format!(
            "the JVM was launched with {ATTACH_DISABLED}; core service supervision remains \
             available, but JCMD thread dumps and on-demand heap dumps cannot attach"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{jcmd, require_attach};
    use crate::error::Error;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "jss-diagnostics-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[test]
    fn resolves_jcmd_beside_an_explicit_java_runtime() {
        let directory = test_directory("explicit");
        fs::create_dir_all(directory.join("runtime/bin")).expect("create runtime bin");
        let executable = if cfg!(windows) { "jcmd.exe" } else { "jcmd" };
        let java = directory.join("runtime/bin/java.exe");
        let expected = directory.join("runtime/bin").join(executable);
        fs::write(&expected, b"fixture").expect("write diagnostic fixture");
        assert_eq!(
            jcmd(&java, &[], &directory).expect("resolve adjacent jcmd"),
            expected
        );
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn resolves_bare_jcmd_from_the_configured_path() {
        let directory = test_directory("path");
        let tools = directory.join("tools");
        fs::create_dir_all(&tools).expect("create tools directory");
        let executable = if cfg!(windows) { "jcmd.exe" } else { "jcmd" };
        let expected = tools.join(executable);
        fs::write(&expected, b"fixture").expect("write diagnostic fixture");
        let path = std::env::join_paths([&tools]).expect("compose test PATH");
        let environment = vec![(OsString::from("PATH"), path)];
        assert_eq!(
            jcmd(
                PathBuf::from("java.exe").as_path(),
                &environment,
                &directory
            )
            .expect("resolve PATH jcmd"),
            expected
        );
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn missing_jcmd_is_a_capability_error_not_a_runtime_error() {
        let directory = test_directory("missing");
        fs::create_dir_all(&directory).expect("create fixture");
        let error = jcmd(&directory.join("runtime/bin/java.exe"), &[], &directory)
            .expect_err("missing jcmd must be reported");
        assert!(matches!(error, Error::DiagnosticUnavailable(_)));
        assert!(
            error
                .to_string()
                .contains("core service supervision remains available")
        );
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn the_last_attach_option_wins() {
        require_attach(&[]).expect("attach defaults to enabled");
        let disabled = [OsString::from("-XX:+DisableAttachMechanism")];
        assert!(matches!(
            require_attach(&disabled),
            Err(Error::DiagnosticUnavailable(_))
        ));
        require_attach(&[
            OsString::from("-XX:+DisableAttachMechanism"),
            OsString::from("-XX:-DisableAttachMechanism"),
        ])
        .expect("last option enables attach");
    }
}
