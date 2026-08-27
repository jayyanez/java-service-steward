// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Optional checks against a local directory of reference `wrapper.conf` and
//! `wrapper.log` files that are not part of the repository. Set
//! `JSS_REFERENCE_DIR` to enable them; they skip cleanly otherwise.

use std::fs;
use std::path::PathBuf;

use java_service_steward::config::{Config, WarningLevel};

fn reference_directory() -> Option<PathBuf> {
    let value = std::env::var_os("JSS_REFERENCE_DIR")?;
    let path = PathBuf::from(value);
    if path.is_dir() {
        Some(path)
    } else {
        eprintln!(
            "skipping reference checks because JSS_REFERENCE_DIR is not a directory: {}",
            path.display()
        );
        None
    }
}

#[test]
fn every_reference_configuration_parses_without_unsupported_properties() {
    let Some(directory) = reference_directory() else {
        return;
    };
    let entries = fs::read_dir(&directory).expect("read reference directory");
    let mut checked = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "conf") {
            continue;
        }
        let config = Config::load(&path, &directory, &[])
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
        let unsupported: Vec<_> = config
            .warnings()
            .iter()
            .filter(|warning| warning.level == WarningLevel::Warn)
            .collect();
        assert!(
            unsupported.is_empty(),
            "every wrapper.* property in {} must be recognized: {unsupported:?}",
            path.display()
        );
        assert!(!config.required("wrapper.java.command").unwrap().is_empty());
        assert!(
            !config
                .required("wrapper.java.mainclass")
                .unwrap()
                .is_empty()
        );
        checked += 1;
    }
    eprintln!("checked {checked} reference configuration files");
}

#[test]
fn every_reference_log_follows_the_lptm_and_crlf_contract() {
    let Some(directory) = reference_directory() else {
        return;
    };
    let entries = fs::read_dir(&directory).expect("read reference directory");
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.contains(".log") {
            continue;
        }
        let bytes = fs::read(&path).expect("read reference log");
        assert!(
            bytes.ends_with(b"\r\n"),
            "{} must end with CRLF",
            path.display()
        );
        let text = String::from_utf8_lossy(&bytes);
        for line in text.split("\r\n").filter(|line| !line.is_empty()) {
            assert_lptm(line, &path);
        }
    }
}

fn assert_lptm(line: &str, path: &std::path::Path) {
    let columns: Vec<&str> = line.splitn(4, " | ").collect();
    assert_eq!(
        columns.len(),
        4,
        "{}: expected four LPTM columns",
        path.display()
    );
    assert_eq!(columns[0].len(), 6, "{}: level width", path.display());
    assert!(
        matches!(
            columns[0],
            "DEBUG " | "INFO  " | "STATUS" | "WARN  " | "ERROR " | "FATAL " | "ADVICE" | "NOTICE"
        ),
        "{}: known level",
        path.display()
    );
    assert_eq!(columns[1].len(), 8, "{}: source width", path.display());
    assert_eq!(columns[2].len(), 19, "{}: timestamp width", path.display());
    let timestamp = columns[2].as_bytes();
    for separator in [4, 7] {
        assert_eq!(timestamp[separator], b'/');
    }
    assert_eq!(timestamp[10], b' ');
    for separator in [13, 16] {
        assert_eq!(timestamp[separator], b':');
    }
}
