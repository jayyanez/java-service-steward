// SPDX-License-Identifier: Apache-2.0 OR MIT
//! `wrapper.conf` parsing: includes, encoding directive, environment
//! definitions and expansion, numbered sequences, command-line overrides and
//! warnings for unsupported properties.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};

use crate::error::{Error, Result};

const MAX_INCLUDE_DEPTH: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLevel {
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub level: WarningLevel,
    pub message: String,
}

impl Warning {
    fn warn(message: impl Into<String>) -> Self {
        Self {
            level: WarningLevel::Warn,
            message: message.into(),
        }
    }

    fn info(message: impl Into<String>) -> Self {
        Self {
            level: WarningLevel::Info,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    root_path: PathBuf,
    executable_directory: PathBuf,
    properties: BTreeMap<String, String>,
    environment: BTreeMap<String, String>,
    warnings: Vec<Warning>,
}

impl Config {
    pub fn load(
        root_path: impl AsRef<Path>,
        executable_directory: impl AsRef<Path>,
        overrides: &[String],
    ) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        let executable_directory = executable_directory.as_ref().to_path_buf();
        let mut loader = Loader::new(executable_directory.clone(), overrides)?;
        loader.load_file(&root_path, 0, true)?;
        loader.apply_overrides(overrides)?;
        loader.warn_about_unsupported_properties();

        Ok(Self {
            root_path,
            executable_directory,
            properties: loader.properties,
            environment: loader.environment,
            warnings: loader.warnings,
        })
    }

    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    #[must_use]
    pub fn executable_directory(&self) -> &Path {
        &self.executable_directory
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.properties.get(name).map(String::as_str)
    }

    #[must_use]
    pub fn get_or<'a>(&'a self, name: &str, default: &'a str) -> &'a str {
        self.get(name).unwrap_or(default)
    }

    pub fn required(&self, name: &str) -> Result<&str> {
        self.get(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::Config(format!("missing required property {name}")))
    }

    #[must_use]
    pub fn get_bool(&self, name: &str, default: bool) -> bool {
        self.get(name).map_or(default, |value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "1" | "on"
            )
        })
    }

    #[must_use]
    pub fn get_u64(&self, name: &str, default: u64) -> u64 {
        self.get(name)
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(default)
    }

    /// Returns a numbered sequence. The first missing index terminates the
    /// sequence unless `wrapper.ignore_sequence_gaps` is enabled; explicit
    /// empty entries are kept.
    #[must_use]
    pub fn numbered(&self, prefix: &str) -> Vec<(u32, &str)> {
        if self.get_bool("wrapper.ignore_sequence_gaps", false) {
            let numbered_prefix = format!("{prefix}.");
            let mut values: Vec<(u32, &str)> = self
                .properties
                .iter()
                .filter_map(|(name, value)| {
                    name.strip_prefix(&numbered_prefix)
                        .and_then(|suffix| suffix.parse::<u32>().ok())
                        .filter(|index| *index > 0)
                        .map(|index| (index, value.as_str()))
                })
                .collect();
            values.sort_unstable_by_key(|(index, _)| *index);
            return values;
        }
        let mut values = Vec::new();
        for index in 1.. {
            let name = format!("{prefix}.{index}");
            let Some(value) = self.get(&name) else {
                break;
            };
            values.push((index, value));
        }
        values
    }

    #[must_use]
    pub fn properties(&self) -> &BTreeMap<String, String> {
        &self.properties
    }

    #[must_use]
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }

    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    #[must_use]
    pub fn resolve_path(&self, value: &str) -> PathBuf {
        let value = remove_grouping_quotes(value.trim());
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            self.executable_directory.join(path)
        }
    }

    /// Encodes the properties for the JVM: `name=value` entries separated by
    /// tabs, with literal tabs doubled. Properties whose names look sensitive
    /// (for example `wrapper.ntservice.password`) are not sent.
    #[must_use]
    pub fn protocol_properties(&self) -> String {
        let mut output = String::new();
        for (name, value) in &self.properties {
            if is_sensitive_name(name) {
                continue;
            }
            if !output.is_empty() {
                output.push('\t');
            }
            output.push_str(&name.replace('\t', "\t\t"));
            output.push('=');
            output.push_str(&value.replace('\t', "\t\t"));
        }
        output
    }
}

struct Loader {
    executable_directory: PathBuf,
    properties: BTreeMap<String, String>,
    environment: BTreeMap<String, String>,
    locked_environment: BTreeSet<String>,
    include_stack: Vec<PathBuf>,
    warnings: Vec<Warning>,
}

impl Loader {
    fn new(executable_directory: PathBuf, overrides: &[String]) -> Result<Self> {
        let mut environment = BTreeMap::new();
        for (name, value) in std::env::vars() {
            environment.insert(normalize_environment_name(&name), value);
        }

        let mut locked_environment = BTreeSet::new();
        for override_value in overrides {
            let (name, value) = split_property(override_value).ok_or_else(|| {
                Error::Cli(format!("invalid property override: {override_value}"))
            })?;
            if let Some(environment_name) = name.strip_prefix("set.") {
                let normalized = normalize_environment_name(environment_name);
                environment.insert(normalized.clone(), value.to_owned());
                locked_environment.insert(normalized);
            }
        }

        Ok(Self {
            executable_directory,
            properties: BTreeMap::new(),
            environment,
            locked_environment,
            include_stack: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn load_file(&mut self, path: &Path, depth: usize, required: bool) -> Result<()> {
        if depth > MAX_INCLUDE_DEPTH {
            return Err(Error::Config(format!(
                "more than {MAX_INCLUDE_DEPTH} nested #include levels"
            )));
        }

        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.executable_directory.join(path)
        };
        let canonical = match resolved.canonicalize() {
            Ok(path) => path,
            Err(source) if !required && source.kind() == std::io::ErrorKind::NotFound => {
                self.warnings.push(Warning::warn(format!(
                    "optional #include not found: {}",
                    resolved.display()
                )));
                return Ok(());
            }
            Err(source) => {
                return Err(Error::ConfigRead {
                    path: resolved,
                    source,
                });
            }
        };
        if self.include_stack.contains(&canonical) {
            return Err(Error::Config(format!(
                "#include cycle detected at {}",
                canonical.display()
            )));
        }

        let bytes = fs::read(&canonical).map_err(|source| Error::ConfigRead {
            path: canonical.clone(),
            source,
        })?;
        let (text, decoding_warning) = decode_configuration(&bytes, &canonical)?;
        if let Some(message) = decoding_warning {
            self.warnings.push(Warning::warn(message));
        }

        self.include_stack.push(canonical.clone());
        for (line_index, logical_line) in logical_lines(&text).into_iter().enumerate() {
            self.parse_line(&canonical, line_index + 1, &logical_line, depth)?;
        }
        self.include_stack.pop();
        Ok(())
    }

    fn parse_line(&mut self, path: &Path, line: usize, raw: &str, depth: usize) -> Result<()> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('!') {
            return Ok(());
        }

        if let Some((directive, argument)) = parse_include(trimmed) {
            let expanded = expand_environment(argument, &self.environment);
            let include_path = PathBuf::from(remove_grouping_quotes(expanded.trim()));
            let required = directive == "#include.required";
            if directive == "#include.debug" {
                self.warnings.push(Warning::info(format!(
                    "#include.debug from {}:{} -> {}",
                    path.display(),
                    line,
                    include_path.display()
                )));
            }
            return self.load_file(&include_path, depth + 1, required);
        }

        if trimmed.starts_with('#') {
            return Ok(());
        }

        let Some((name, raw_value)) = split_property(trimmed) else {
            self.warnings.push(Warning::warn(format!(
                "ignored line without '=' at {}:{line}",
                path.display()
            )));
            return Ok(());
        };
        if name.is_empty() {
            return Err(Error::ConfigSyntax {
                path: path.to_path_buf(),
                line,
                message: "the property name is empty".into(),
            });
        }

        let uncommented = decode_hash_comments(raw_value);
        let value = expand_environment(uncommented.trim(), &self.environment);
        self.properties.insert(name.to_owned(), value.clone());
        self.apply_environment_property(name, &value);
        Ok(())
    }

    fn apply_environment_property(&mut self, name: &str, value: &str) {
        if let Some(environment_name) = name.strip_prefix("set.default.") {
            let normalized = normalize_environment_name(environment_name);
            if !self.locked_environment.contains(&normalized) {
                self.environment
                    .entry(normalized)
                    .or_insert_with(|| value.into());
            }
        } else if let Some(environment_name) = name.strip_prefix("set.") {
            let normalized = normalize_environment_name(environment_name);
            if !self.locked_environment.contains(&normalized) {
                self.environment.insert(normalized, value.into());
            }
        }
    }

    fn apply_overrides(&mut self, overrides: &[String]) -> Result<()> {
        for override_value in overrides {
            let (name, raw_value) = split_property(override_value).ok_or_else(|| {
                Error::Cli(format!("invalid property override: {override_value}"))
            })?;
            let value = expand_environment(raw_value, &self.environment);
            self.properties.insert(name.into(), value.clone());
            self.apply_environment_property(name, &value);
        }
        Ok(())
    }

    fn warn_about_unsupported_properties(&mut self) {
        let mut ignored_families: BTreeSet<&str> = BTreeSet::new();
        for name in self.properties.keys() {
            if name.starts_with("jss.") && !SUPPORTED_JSS_PROPERTIES.contains(&name.as_str()) {
                self.warnings.push(Warning::warn(format!(
                    "the property {name} is not a known Java Service Steward extension and will be ignored"
                )));
                continue;
            }
            if !name.starts_with("wrapper.") || is_supported_wrapper_property(name) {
                continue;
            }
            if let Some(family) = IGNORED_WRAPPER_PROPERTY_PREFIXES
                .iter()
                .find(|prefix| name.starts_with(*prefix))
            {
                ignored_families.insert(family);
                continue;
            }
            self.warnings.push(Warning::warn(format!(
                "the property {name} is not implemented and will be ignored"
            )));
        }
        for family in ignored_families {
            self.warnings.push(Warning::info(format!(
                "{family}* properties are accepted and ignored"
            )));
        }
    }
}

/// Property families that are accepted without a per-property warning because
/// they belong to features this project deliberately does not implement.
pub(crate) const IGNORED_WRAPPER_PROPERTY_PREFIXES: &[&str] = &["wrapper.license."];

/// Project extensions; every entry must also be documented in `help.txt`.
pub(crate) const SUPPORTED_JSS_PROPERTIES: &[&str] = &[
    "jss.threaddump.method",
    "jss.threaddump.timeout",
    "jss.heapdump.control_code",
    "jss.heapdump.directory",
    "jss.heapdump.timeout",
    "jss.java.job_object",
];

pub(crate) const SUPPORTED_NUMBERED_WRAPPER_PROPERTY_PREFIXES: &[&str] = &[
    "wrapper.java.additional.",
    "wrapper.java.classpath.",
    "wrapper.java.library.path.",
    "wrapper.app.parameter.",
    "wrapper.filter.trigger.",
    "wrapper.filter.action.",
    "wrapper.filter.allow_wildcards.",
    "wrapper.filter.message.",
    "wrapper.on_exit.",
    "wrapper.ntservice.dependency.",
];

pub(crate) const SUPPORTED_WRAPPER_PROPERTIES: &[&str] = &[
    "wrapper.java.command",
    "wrapper.java.mainclass",
    "wrapper.java.additional.auto_bits",
    "wrapper.java.initmemory",
    "wrapper.java.maxmemory",
    "wrapper.java.command.loglevel",
    "wrapper.debug",
    "wrapper.ignore_sequence_gaps",
    "wrapper.pidfile",
    "wrapper.pidfile.strict",
    "wrapper.java.pidfile",
    "wrapper.java.idfile",
    "wrapper.working.dir",
    "wrapper.port",
    "wrapper.port.min",
    "wrapper.port.max",
    "wrapper.jvm.port",
    "wrapper.jvm.port.min",
    "wrapper.jvm.port.max",
    "wrapper.native_library",
    "wrapper.disable_console_input",
    "wrapper.listener.force_stop",
    "wrapper.use_system_time",
    "wrapper.disable_shutdown_hook",
    "wrapper.cpu.timeout",
    "wrapper.startup.timeout",
    "wrapper.startup.delay",
    "wrapper.startup.delay.console",
    "wrapper.startup.delay.service",
    "wrapper.shutdown.timeout",
    "wrapper.request_thread_dump_on_failed_jvm_exit",
    "wrapper.request_thread_dump_on_failed_jvm_exit.delay",
    "wrapper.ping.interval",
    "wrapper.ping.timeout",
    "wrapper.restart.delay",
    "wrapper.disable_restarts",
    "wrapper.disable_restarts.automatic",
    "wrapper.max_failed_invocations",
    "wrapper.successful_invocation_time",
    "wrapper.logfile",
    "wrapper.logfile.format",
    "wrapper.logfile.loglevel",
    "wrapper.logfile.rollmode",
    "wrapper.logfile.maxfiles",
    "wrapper.logfile.maxsize",
    "wrapper.console.format",
    "wrapper.console.loglevel",
    "wrapper.console.flush",
    "wrapper.console.title",
    "wrapper.console.title.windows",
    "wrapper.internal.namedpipe",
    "wrapper.syslog.loglevel",
    "wrapper.thread_dump_control_code",
    "wrapper.pausable",
    "wrapper.pausable.stop_jvm",
    "wrapper.ntservice.generate_console",
    "wrapper.ntservice.name",
    "wrapper.ntservice.displayname",
    "wrapper.ntservice.description",
    "wrapper.ntservice.pausable",
    "wrapper.ntservice.pausable.stop_jvm",
    "wrapper.ntservice.starttype",
    "wrapper.ntservice.account",
    "wrapper.ntservice.password",
    "wrapper.ntservice.interactive",
    "wrapper.pause_on_startup",
];

fn is_supported_wrapper_property(name: &str) -> bool {
    if SUPPORTED_NUMBERED_WRAPPER_PROPERTY_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return true;
    }
    SUPPORTED_WRAPPER_PROPERTIES.contains(&name)
}

fn parse_include(line: &str) -> Option<(&str, &str)> {
    for directive in ["#include.required", "#include.debug", "#include"] {
        if let Some(rest) = line.strip_prefix(directive) {
            if rest.is_empty() {
                return Some((directive, rest));
            }
            if rest.starts_with(char::is_whitespace) || rest.starts_with('=') {
                return Some((directive, rest.trim_start_matches([' ', '\t', '='])));
            }
        }
    }
    None
}

fn split_property(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.split_once('=')?;
    Some((name.trim(), value))
}

fn decode_hash_comments(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '#' {
            output.push(character);
            continue;
        }
        if characters.peek() == Some(&'#') {
            characters.next();
            output.push('#');
        } else {
            break;
        }
    }
    output
}

fn logical_lines(text: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut pending = String::new();
    for physical in text.lines() {
        let without_cr = physical.trim_end_matches('\r');
        let slash_count = without_cr
            .chars()
            .rev()
            .take_while(|ch| *ch == '\\')
            .count();
        if slash_count % 2 == 1 {
            pending.push_str(&without_cr[..without_cr.len() - 1]);
        } else if pending.is_empty() {
            output.push(without_cr.to_owned());
        } else {
            pending.push_str(without_cr);
            output.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        output.push(pending);
    }
    output
}

fn decode_configuration(bytes: &[u8], path: &Path) -> Result<(String, Option<String>)> {
    let label = first_line_encoding_label(bytes);
    let encoding = match label.as_deref() {
        Some(label) => {
            Encoding::for_label(label.as_bytes()).ok_or_else(|| Error::ConfigSyntax {
                path: path.to_path_buf(),
                line: 1,
                message: format!("unknown encoding: {label}"),
            })?
        }
        None if std::str::from_utf8(bytes).is_ok() => UTF_8,
        None => WINDOWS_1252,
    };
    let (decoded, _, had_errors) = encoding.decode(bytes);
    let warning = if had_errors {
        Some(format!(
            "{} contains byte sequences that are invalid for {}",
            path.display(),
            encoding.name()
        ))
    } else if label.is_none() && encoding == WINDOWS_1252 {
        Some(format!(
            "{} is not UTF-8; it was read as windows-1252",
            path.display()
        ))
    } else {
        None
    };
    Ok((decoded.into_owned(), warning))
}

fn first_line_encoding_label(bytes: &[u8]) -> Option<String> {
    let first = bytes.split(|byte| *byte == b'\n').next()?;
    let first = std::str::from_utf8(first.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(first))
        .ok()?
        .trim();
    let value = first.strip_prefix("#encoding=")?.trim();
    Some(remove_grouping_quotes(value).to_owned())
}

#[must_use]
pub fn expand_environment(value: &str, environment: &BTreeMap<String, String>) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find('%') {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('%') else {
            output.push_str(&remaining[start..]);
            return output;
        };
        let name = &after_start[..end];
        if let Some(expansion) = environment.get(&normalize_environment_name(name)) {
            output.push_str(expansion);
        } else {
            output.push('%');
            output.push_str(name);
            output.push('%');
        }
        remaining = &after_start[end + 1..];
    }
    output.push_str(remaining);
    output
}

fn normalize_environment_name(name: &str) -> String {
    name.to_uppercase()
}

#[must_use]
pub fn remove_grouping_quotes(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[must_use]
pub fn is_sensitive_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    ["password", "secret", "token", "key", "credential", "vault"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

#[must_use]
pub fn redacted_value(name: &str, value: &str) -> String {
    if is_sensitive_name(name) {
        "<redacted>".into()
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Config, WarningLevel, decode_hash_comments, expand_environment, is_sensitive_name,
        logical_lines,
    };
    use std::collections::BTreeMap;
    use std::fs;

    fn unique_directory() -> std::path::PathBuf {
        crate::test_support::unique_directory("config")
    }

    #[test]
    fn expands_case_insensitive_environment_and_preserves_unknowns() {
        let environment = BTreeMap::from([("JAVA_HOME".into(), "C:/Java".into())]);
        assert_eq!(
            expand_environment("%java_home%/bin;%UNKNOWN%", &environment),
            "C:/Java/bin;%UNKNOWN%"
        );
    }

    #[test]
    fn joins_continuation_lines() {
        assert_eq!(
            logical_lines("a=one\\\ntwo\nb=three"),
            ["a=onetwo", "b=three"]
        );
    }

    #[test]
    fn strips_inline_hash_comments_and_decodes_doubled_hashes() {
        assert_eq!(decode_hash_comments("value # comment"), "value ");
        assert_eq!(decode_hash_comments("pa##ss##word"), "pa#ss#word");
        assert_eq!(decode_hash_comments("value###comment"), "value#");
    }

    #[test]
    fn loads_environment_numbered_values_and_overrides() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "set.APP_HOME=C:/Synthetic\nwrapper.java.command=%APP_HOME%/java.exe\nwrapper.java.additional.1=-Done=1\nwrapper.java.additional.2=\nwrapper.java.additional.3=-Dthree=3\n",
        )
        .expect("write fixture");

        let config = Config::load(
            &path,
            &directory,
            &[
                "set.APP_HOME=D:/Override".into(),
                "wrapper.debug=true".into(),
            ],
        )
        .expect("load config");
        assert_eq!(
            config.get("wrapper.java.command"),
            Some("D:/Override/java.exe")
        );
        assert_eq!(config.get("wrapper.debug"), Some("true"));
        assert_eq!(
            config.numbered("wrapper.java.additional"),
            [(1, "-Done=1"), (2, ""), (3, "-Dthree=3")]
        );

        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn identifies_sensitive_property_names() {
        assert!(is_sensitive_name("wrapper.java.additional.password"));
        assert!(is_sensitive_name("API_TOKEN"));
        assert!(!is_sensitive_name("wrapper.logfile"));
    }

    #[test]
    fn optionally_keeps_numbered_values_after_sequence_gaps() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.ignore_sequence_gaps=true\nwrapper.java.additional.1=-Done=1\nwrapper.java.additional.3=-Dthree=3\n",
        )
        .expect("write fixture");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(
            config.numbered("wrapper.java.additional"),
            [(1, "-Done=1"), (3, "-Dthree=3")]
        );
        assert!(config.warnings().is_empty());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn warns_about_unknown_wrapper_properties_without_echoing_values() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.future.option=synthetic-secret-value\n").expect("write fixture");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(config.warnings().len(), 1);
        assert_eq!(config.warnings()[0].level, WarningLevel::Warn);
        assert!(
            config.warnings()[0]
                .message
                .contains("wrapper.future.option")
        );
        assert!(
            !config.warnings()[0]
                .message
                .contains("synthetic-secret-value")
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn license_properties_produce_a_single_informational_notice() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.license.type=SYNTHETIC\nwrapper.license.id=1\nwrapper.license.key.1=abc\n",
        )
        .expect("write fixture");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(config.warnings().len(), 1);
        assert_eq!(config.warnings()[0].level, WarningLevel::Info);
        assert!(config.warnings()[0].message.contains("wrapper.license."));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn unknown_extension_properties_warn_like_unknown_wrapper_properties() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "jss.heapdump.timout=5
jss.heapdump.timeout=5
",
        )
        .expect("write fixture");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(config.warnings().len(), 1);
        assert_eq!(config.warnings()[0].level, WarningLevel::Warn);
        assert!(config.warnings()[0].message.contains("jss.heapdump.timout"));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn protocol_properties_exclude_sensitive_names() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.ntservice.name=synthetic\nwrapper.ntservice.password=do-not-send\nwrapper.java.additional.1=-Dkeep=1\n",
        )
        .expect("write fixture");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let encoded = config.protocol_properties();
        assert!(encoded.contains("wrapper.ntservice.name=synthetic"));
        assert!(encoded.contains("wrapper.java.additional.1=-Dkeep=1"));
        assert!(!encoded.contains("do-not-send"));
        assert!(!encoded.contains("wrapper.ntservice.password"));
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
