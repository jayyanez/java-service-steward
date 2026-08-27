// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{Local, Timelike};

use crate::config::Config;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSource {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Debug = 1,
    Info = 2,
    Status = 3,
    Warn = 4,
    Error = 5,
    Fatal = 6,
    Advice = 7,
    Notice = 8,
    None = 9,
}

impl LogLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG ",
            Self::Info => "INFO  ",
            Self::Status => "STATUS",
            Self::Warn => "WARN  ",
            Self::Error => "ERROR ",
            Self::Fatal => "FATAL ",
            Self::Advice => "ADVICE",
            Self::Notice => "NOTICE",
            Self::None => "NONE  ",
        }
    }

    #[must_use]
    pub fn from_protocol_code(code: u8) -> Option<Self> {
        match code {
            117 => Some(Self::Debug),
            118 => Some(Self::Info),
            119 => Some(Self::Status),
            120 => Some(Self::Warn),
            121 => Some(Self::Error),
            122 => Some(Self::Fatal),
            123 => Some(Self::Advice),
            124 => Some(Self::Notice),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    Wrapper,
    Protocol,
    Jvm(u32),
}

impl LogSource {
    fn label(self) -> String {
        match self {
            Self::Wrapper => "wrapper ".into(),
            Self::Protocol => "wrapperp".into(),
            Self::Jvm(id) => format!("jvm {id:<4}"),
        }
    }

    fn thread_label(self) -> &'static str {
        match self {
            Self::Jvm(_) => "javaio ",
            Self::Wrapper | Self::Protocol => "main   ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    None,
    Debug,
    Dump,
    Gc,
    Restart,
    Shutdown,
    Pause,
    Resume,
}

#[derive(Debug, Clone)]
pub struct FilterMatch {
    pub index: u32,
    pub actions: Vec<FilterAction>,
    pub message: String,
}

#[derive(Debug, Clone)]
struct Filter {
    index: u32,
    trigger: String,
    allow_wildcards: bool,
    actions: Vec<FilterAction>,
    message: String,
}

#[derive(Debug, Clone)]
pub struct Filters {
    entries: Vec<Filter>,
}

impl Filters {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let entries = config
            .numbered("wrapper.filter.trigger")
            .into_iter()
            .filter_map(|(index, trigger)| {
                let action_name = format!("wrapper.filter.action.{index}");
                let mut actions: Vec<FilterAction> = config
                    .get_or(&action_name, "RESTART")
                    .split([',', ' ', '\t'])
                    .filter_map(parse_filter_action)
                    .take(16)
                    .collect();
                if actions.is_empty() {
                    actions.push(FilterAction::Restart);
                }
                (!trigger.is_empty()).then(|| Filter {
                    index,
                    trigger: trigger.into(),
                    allow_wildcards: config
                        .get_bool(&format!("wrapper.filter.allow_wildcards.{index}"), false),
                    actions,
                    message: config
                        .get_or(
                            &format!("wrapper.filter.message.{index}"),
                            "Filter trigger matched.",
                        )
                        .into(),
                })
            })
            .collect();
        Self { entries }
    }

    #[must_use]
    pub fn inspect(&self, line: &str) -> Vec<FilterMatch> {
        self.inspect_bytes(line.as_bytes())
    }

    #[must_use]
    pub fn inspect_bytes(&self, line: &[u8]) -> Vec<FilterMatch> {
        self.entries
            .iter()
            .find(|filter| trigger_matches(line, &filter.trigger, filter.allow_wildcards))
            .map(|filter| FilterMatch {
                index: filter.index,
                actions: filter.actions.clone(),
                message: filter.message.clone(),
            })
            .into_iter()
            .collect()
    }
}

pub struct LogWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    current_size: u64,
    max_size: u64,
    max_files: u32,
    roll_on_jvm_restart: bool,
    format: String,
    threshold: LogLevel,
    console_enabled: bool,
    console_format: String,
    console_threshold: LogLevel,
    console_flush: bool,
    started: Instant,
}

impl LogWriter {
    pub fn from_config(config: &Config) -> Result<Self> {
        Self::from_config_with_console(config, false)
    }

    pub fn from_config_with_console(config: &Config, console_enabled: bool) -> Result<Self> {
        let path = config.resolve_path(config.get_or("wrapper.logfile", "wrapper.log"));
        let max_files = config
            .get_u64("wrapper.logfile.maxfiles", 0)
            .try_into()
            .unwrap_or(u32::MAX);
        let roll_mode = config
            .get_or("wrapper.logfile.rollmode", "SIZE")
            .to_ascii_uppercase();
        let max_size = if matches!(
            roll_mode.as_str(),
            "SIZE" | "SIZE_OR_WRAPPER" | "SIZE_OR_JVM"
        ) {
            parse_size(config.get_or("wrapper.logfile.maxsize", "0")).unwrap_or(0)
        } else {
            0
        };
        if matches!(
            roll_mode.as_str(),
            "WRAPPER" | "JVM" | "SIZE_OR_WRAPPER" | "SIZE_OR_JVM"
        ) && path.exists()
        {
            rotate(&path, max_files)?;
        }
        let file = open_append_shared(&path)?;
        let current_size = file.metadata()?.len();
        Ok(Self {
            path,
            max_size,
            max_files,
            roll_on_jvm_restart: matches!(roll_mode.as_str(), "JVM" | "SIZE_OR_JVM"),
            writer: BufWriter::new(file),
            current_size,
            format: validated_format(config.get_or("wrapper.logfile.format", "LPTM")),
            threshold: if config.get_bool("wrapper.debug", false) {
                LogLevel::Debug
            } else {
                configured_log_level(config, "wrapper.logfile.loglevel", LogLevel::Info)
            },
            console_enabled,
            console_format: validated_format(config.get_or("wrapper.console.format", "PM")),
            console_threshold: if config.get_bool("wrapper.debug", false) {
                LogLevel::Debug
            } else {
                configured_log_level(config, "wrapper.console.loglevel", LogLevel::Info)
            },
            console_flush: config.get_bool("wrapper.console.flush", false),
            started: Instant::now(),
        })
    }

    pub fn write(&mut self, level: LogLevel, source: LogSource, message: &str) -> Result<()> {
        self.write_bytes(level, source, message.as_bytes())
    }

    pub fn roll_for_jvm_restart(&mut self) -> Result<()> {
        if !self.roll_on_jvm_restart {
            return Ok(());
        }
        self.writer.flush()?;
        rotate(&self.path, self.max_files)?;
        let replacement = open_truncated_shared(&self.path)?;
        self.writer = BufWriter::new(replacement);
        self.current_size = 0;
        Ok(())
    }

    pub fn write_bytes(
        &mut self,
        level: LogLevel,
        source: LogSource,
        message: &[u8],
    ) -> Result<()> {
        let write_file = level >= self.threshold && self.threshold != LogLevel::None;
        let write_console = self.console_enabled
            && level >= self.console_threshold
            && self.console_threshold != LogLevel::None;
        if !write_file && !write_console {
            return Ok(());
        }

        if write_console {
            let rendered = self.render_bytes(&self.console_format, level, source, message);
            let mut stdout = io::stdout().lock();
            stdout.write_all(&rendered)?;
            stdout.write_all(b"\r\n")?;
            if self.console_flush {
                stdout.flush()?;
            }
        }

        if write_file {
            if self.max_size > 0 && self.current_size >= self.max_size {
                self.writer.flush()?;
                rotate(&self.path, self.max_files)?;
                let replacement = open_truncated_shared(&self.path)?;
                self.writer = BufWriter::new(replacement);
                self.current_size = 0;
            }
            let rendered = self.render_bytes(&self.format, level, source, message);
            let line_size = rendered.len().saturating_add(2) as u64;
            self.writer.write_all(&rendered)?;
            self.writer.write_all(b"\r\n")?;
            self.writer.flush()?;
            self.current_size = self.current_size.saturating_add(line_size);
        }
        Ok(())
    }

    fn render_bytes(
        &self,
        format: &str,
        level: LogLevel,
        source: LogSource,
        message: &[u8],
    ) -> Vec<u8> {
        let now = Local::now();
        let columns: Vec<Vec<u8>> = format
            .chars()
            .filter_map(|token| match token.to_ascii_uppercase() {
                'L' => Some(level.label().as_bytes().to_vec()),
                'P' => Some(source.label().into_bytes()),
                'D' => Some(source.thread_label().as_bytes().to_vec()),
                'Q' => Some(b" ".to_vec()),
                'T' => Some(now.format("%Y/%m/%d %H:%M:%S").to_string().into_bytes()),
                'Z' => Some(
                    format!(
                        "{}.{:03}",
                        now.format("%Y/%m/%d %H:%M:%S"),
                        now.nanosecond() / 1_000_000
                    )
                    .into_bytes(),
                ),
                'U' => Some(format!("{:8}", self.started.elapsed().as_secs()).into_bytes()),
                'G' => Some(format!("{:8}", 0).into_bytes()),
                'M' => Some(message.to_vec()),
                _ => None,
            })
            .collect();
        let capacity =
            columns.iter().map(Vec::len).sum::<usize>() + columns.len().saturating_sub(1) * 3;
        let mut rendered = Vec::with_capacity(capacity);
        for (index, column) in columns.into_iter().enumerate() {
            if index > 0 {
                rendered.extend_from_slice(b" | ");
            }
            rendered.extend_from_slice(&column);
        }
        rendered
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn parse_filter_action(value: &str) -> Option<FilterAction> {
    match value.trim().to_ascii_uppercase().as_str() {
        "NONE" => Some(FilterAction::None),
        "DEBUG" => Some(FilterAction::Debug),
        "DUMP" => Some(FilterAction::Dump),
        "GC" => Some(FilterAction::Gc),
        "RESTART" => Some(FilterAction::Restart),
        "SHUTDOWN" => Some(FilterAction::Shutdown),
        "PAUSE" => Some(FilterAction::Pause),
        "RESUME" => Some(FilterAction::Resume),
        _ => None,
    }
}

fn trigger_matches(line: &[u8], trigger: &str, allow_wildcards: bool) -> bool {
    if trigger.is_empty() {
        return false;
    }
    if allow_wildcards {
        if trigger.is_ascii() {
            return wildcard_contains(line, trigger.as_bytes());
        }
        if let Ok(text) = std::str::from_utf8(line) {
            return wildcard_contains(text.as_bytes(), trigger.as_bytes());
        }
        let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(line);
        return wildcard_contains(decoded.as_bytes(), trigger.as_bytes());
    }
    if trigger.is_ascii() {
        return line
            .windows(trigger.len())
            .any(|candidate| candidate == trigger.as_bytes());
    }
    if let Ok(text) = std::str::from_utf8(line) {
        return text.contains(trigger);
    }
    let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(line);
    decoded.contains(trigger)
}

fn wildcard_contains(text: &[u8], pattern: &[u8]) -> bool {
    (0..=text.len()).any(|start| wildcard_matches_prefix(&text[start..], pattern))
}

fn wildcard_matches_prefix(text: &[u8], pattern: &[u8]) -> bool {
    let (mut text_index, mut pattern_index) = (0, 0);
    let (mut last_star, mut star_text_index) = (None, 0);

    loop {
        if pattern_index == pattern.len() {
            return true;
        }
        if pattern[pattern_index] == b'*' {
            last_star = Some(pattern_index);
            pattern_index += 1;
            star_text_index = text_index;
            continue;
        }
        if text_index < text.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
            continue;
        }
        let Some(star_index) = last_star else {
            return false;
        };
        if star_text_index == text.len() {
            return false;
        }
        star_text_index += 1;
        text_index = star_text_index;
        pattern_index = star_index + 1;
    }
}

#[must_use]
pub fn low_log_level(config: &Config) -> u8 {
    if config.get_bool("wrapper.debug", false) {
        return LogLevel::Debug as u8;
    }
    [
        configured_log_level(config, "wrapper.console.loglevel", LogLevel::Info),
        configured_log_level(config, "wrapper.logfile.loglevel", LogLevel::Info),
        configured_log_level(config, "wrapper.syslog.loglevel", LogLevel::None),
    ]
    .into_iter()
    .min()
    .unwrap_or(LogLevel::Info) as u8
}

#[must_use]
pub fn java_command_log_level(config: &Config) -> LogLevel {
    configured_log_level(config, "wrapper.java.command.loglevel", LogLevel::Debug)
}

fn configured_log_level(config: &Config, property: &str, default: LogLevel) -> LogLevel {
    match config
        .get(property)
        .map(str::trim)
        .map(str::to_ascii_uppercase)
    {
        Some(value) if value == "DEBUG" => LogLevel::Debug,
        Some(value) if value == "INFO" => LogLevel::Info,
        Some(value) if value == "STATUS" => LogLevel::Status,
        Some(value) if value == "WARN" => LogLevel::Warn,
        Some(value) if value == "ERROR" => LogLevel::Error,
        Some(value) if value == "FATAL" => LogLevel::Fatal,
        Some(value) if value == "ADVICE" => LogLevel::Advice,
        Some(value) if value == "NOTICE" => LogLevel::Notice,
        Some(value) if value == "NONE" => LogLevel::None,
        _ => default,
    }
}

fn validated_format(format: &str) -> String {
    if format.chars().any(|token| {
        matches!(
            token.to_ascii_uppercase(),
            'L' | 'P' | 'D' | 'Q' | 'T' | 'Z' | 'U' | 'G' | 'M'
        )
    }) {
        format.into()
    } else {
        "LPTM".into()
    }
}

fn rotate(path: &Path, max_files: u32) -> Result<()> {
    let rotation_limit = if max_files == 0 {
        highest_rolled_index(path)?
            .checked_add(1)
            .ok_or_else(|| crate::error::Error::Config("too many rotated log files".into()))?
    } else {
        let oldest = rolled_path(path, max_files);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        max_files
    };
    for index in (1..rotation_limit).rev() {
        let source = rolled_path(path, index);
        if source.exists() {
            fs::rename(source, rolled_path(path, index + 1))?;
        }
    }
    if path.exists() {
        fs::rename(path, rolled_path(path, 1))?;
    }
    Ok(())
}

fn highest_rolled_index(path: &Path) -> Result<u32> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let Some(file_name) = path.file_name() else {
        return Ok(0);
    };
    let prefix = format!("{}.", file_name.to_string_lossy());
    let mut highest = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(index) = name
            .strip_prefix(&prefix)
            .and_then(|suffix| suffix.parse::<u32>().ok())
        {
            highest = highest.max(index);
        }
    }
    Ok(highest)
}

fn rolled_path(path: &Path, index: u32) -> PathBuf {
    PathBuf::from(format!("{}.{index}", path.to_string_lossy()))
}

fn open_append_shared(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    configure_windows_log_sharing(&mut options);
    options.open(path)
}

fn open_truncated_shared(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    configure_windows_log_sharing(&mut options);
    options.open(path)
}

#[cfg(windows)]
fn configure_windows_log_sharing(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
}

#[cfg(not(windows))]
fn configure_windows_log_sharing(_options: &mut OpenOptions) {}

#[must_use]
pub fn parse_size(value: &str) -> Option<u64> {
    let normalized = value.trim().to_ascii_lowercase();
    let split = normalized
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(normalized.len());
    let number = normalized[..split].parse::<u64>().ok()?;
    let multiplier = match normalized[split..].trim() {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return None,
    };
    number.checked_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::{
        FilterAction, Filters, LogLevel, LogSource, LogWriter, low_log_level, parse_size,
        rolled_path, rotate,
    };
    use crate::config::Config;
    use std::fs;

    #[test]
    fn parses_legacy_sizes() {
        assert_eq!(parse_size("50m"), Some(50 * 1024 * 1024));
        assert_eq!(parse_size("2 GB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("invalid"), None);
    }

    #[test]
    fn zero_maxfiles_keeps_an_unbounded_rotation_history() {
        let directory = test_directory("unbounded-rotation");
        let active = directory.join("wrapper.log");
        for value in [b"first".as_slice(), b"second", b"third"] {
            fs::write(&active, value).expect("write active log");
            rotate(&active, 0).expect("rotate without archive limit");
        }
        assert_eq!(
            fs::read(directory.join("wrapper.log.1")).expect("read newest archive"),
            b"third"
        );
        assert_eq!(
            fs::read(directory.join("wrapper.log.2")).expect("read middle archive"),
            b"second"
        );
        assert_eq!(
            fs::read(directory.join("wrapper.log.3")).expect("read oldest archive"),
            b"first"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn detects_configured_restart_filter() {
        let directory = test_directory("filter");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.filter.trigger.1=FATAL SYNTHETIC\nwrapper.filter.action.1=RESTART\n",
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let matches = Filters::from_config(&config).inspect("prefix FATAL SYNTHETIC suffix");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].index, 1);
        assert_eq!(matches[0].actions, [FilterAction::Restart]);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn filter_supports_wildcards_messages_chained_actions_and_first_match_only() {
        let directory = test_directory("advanced-filter");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.filter.trigger.1=Head*Tail-?\n\
             wrapper.filter.allow_wildcards.1=TRUE\n\
             wrapper.filter.action.1=DUMP, GC RESTART\n\
             wrapper.filter.message.1=Synthetic recovery.\n\
             wrapper.filter.trigger.2=Tail\n\
             wrapper.filter.action.2=SHUTDOWN\n",
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let matches = Filters::from_config(&config).inspect("prefix Head middle Tail-7 suffix");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].index, 1);
        assert_eq!(
            matches[0].actions,
            [FilterAction::Dump, FilterAction::Gc, FilterAction::Restart]
        );
        assert_eq!(matches[0].message, "Synthetic recovery.");
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn omitted_filter_action_defaults_to_restart_and_none_blocks_later_filters() {
        let directory = test_directory("filter-default-and-none");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.filter.trigger.1=IgnoreError\n\
             wrapper.filter.action.1=NONE\n\
             wrapper.filter.trigger.2=Error\n\
             wrapper.filter.action.2=SHUTDOWN\n\
             wrapper.filter.trigger.3=DefaultAction\n",
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let filters = Filters::from_config(&config);
        assert_eq!(
            filters.inspect("IgnoreError")[0].actions,
            [FilterAction::None]
        );
        assert_eq!(
            filters.inspect("DefaultAction")[0].actions,
            [FilterAction::Restart]
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn renders_the_legacy_lptm_shape_with_crlf() {
        let directory = test_directory("format");
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.logfile=wrapper.log\n").expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("create logger");
        logger
            .write(LogLevel::Info, LogSource::Jvm(1), "synthetic message")
            .expect("write log");
        drop(logger);
        let bytes = fs::read(directory.join("wrapper.log")).expect("read log");
        let text = String::from_utf8(bytes.clone()).expect("utf8 log");
        let columns: Vec<&str> = text.trim_end().split(" | ").collect();
        assert_eq!(columns.len(), 4);
        assert_eq!(columns[0], "INFO  ");
        assert_eq!(columns[1], "jvm 1   ");
        assert_eq!(columns[2].len(), 19);
        assert_eq!(columns[3], "synthetic message");
        assert!(bytes.ends_with(b"\r\n"));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn renders_the_legacy_console_pm_shape() {
        let directory = test_directory("console-format");
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.logfile=wrapper.log\n").expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let logger = LogWriter::from_config(&config).expect("create logger");
        assert_eq!(
            logger.render_bytes("PM", LogLevel::Status, LogSource::Wrapper, b"message"),
            b"wrapper  | message"
        );
        drop(logger);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn console_flush_honors_false_and_command_line_true() {
        let directory = test_directory("console-flush");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.logfile=wrapper.log\n\
             wrapper.console.flush=false\n\
             wrapper.internal.namedpipe=1992730144\n",
        )
        .expect("write config");

        let config = Config::load(&path, &directory, &[]).expect("load false config");
        let logger = LogWriter::from_config(&config).expect("create false logger");
        assert!(!logger.console_flush);
        assert!(config.warnings().is_empty());
        drop(logger);

        let config = Config::load(&path, &directory, &["wrapper.console.flush=true".into()])
            .expect("load true override");
        let logger = LogWriter::from_config(&config).expect("create true logger");
        assert!(logger.console_flush);
        assert!(config.warnings().is_empty());
        drop(logger);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn jvm_rollmode_rotates_on_wrapper_start_and_each_jvm_restart() {
        let directory = test_directory("jvm-rollmode");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.logfile=wrapper.log\nwrapper.logfile.rollmode=JVM\nwrapper.logfile.maxfiles=3\n",
        )
        .expect("write config");
        fs::write(directory.join("wrapper.log"), b"before wrapper start\r\n")
            .expect("write existing log");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("create logger");
        assert_eq!(
            fs::read(directory.join("wrapper.log.1")).expect("read wrapper-start archive"),
            b"before wrapper start\r\n"
        );
        logger
            .write(LogLevel::Info, LogSource::Jvm(1), "first JVM")
            .expect("write first JVM record");
        logger
            .roll_for_jvm_restart()
            .expect("roll before second JVM");
        logger
            .write(LogLevel::Info, LogSource::Jvm(2), "second JVM")
            .expect("write second JVM record");
        drop(logger);

        let first_archive =
            fs::read_to_string(directory.join("wrapper.log.1")).expect("read first JVM archive");
        assert!(first_archive.contains("first JVM"));
        assert_eq!(
            fs::read(directory.join("wrapper.log.2")).expect("read pre-start archive"),
            b"before wrapper start\r\n"
        );
        let active =
            fs::read_to_string(directory.join("wrapper.log")).expect("read active JVM log");
        assert!(active.contains("second JVM"));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn active_log_can_be_read_while_the_writer_is_open() {
        let directory = test_directory("shared-read");
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.logfile=wrapper.log\n").expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("create logger");
        logger
            .write(LogLevel::Info, LogSource::Jvm(1), "read while running")
            .expect("write log");

        let bytes = fs::read(directory.join("wrapper.log")).expect("open active log for reading");
        assert!(bytes.ends_with(b"read while running\r\n"));
        drop(logger);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn preserves_non_utf8_jvm_output_bytes() {
        let directory = test_directory("raw-jvm-bytes");
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.logfile=wrapper.log\n").expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("create logger");
        logger
            .write_bytes(LogLevel::Info, LogSource::Jvm(1), b"Windows-1252: \xe1")
            .expect("write raw log bytes");
        drop(logger);
        let bytes = fs::read(directory.join("wrapper.log")).expect("read log");
        assert!(bytes.ends_with(b"Windows-1252: \xe1\r\n"));
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn sends_info_as_the_default_low_log_level() {
        let directory = test_directory("low-level");
        let path = directory.join("wrapper.conf");
        fs::write(&path, "wrapper.logfile=wrapper.log\n").expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        assert_eq!(low_log_level(&config), LogLevel::Info as u8);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn resolves_wildfly_logfile_from_the_wrapper_executable_directory() {
        let directory = test_directory("wildfly-path");
        let binary_directory = directory.join("bin");
        let log_directory = directory.join("standalone").join("log");
        fs::create_dir_all(&binary_directory).expect("create bin directory");
        fs::create_dir_all(&log_directory).expect("create log directory");
        let path = binary_directory.join("wrapper.conf");
        fs::write(&path, "wrapper.logfile=../standalone/log/wrapper.log\n").expect("write config");
        let config = Config::load(&path, &binary_directory, &[]).expect("load config");
        let logger = LogWriter::from_config(&config).expect("create logger");
        assert_eq!(
            logger.path().canonicalize().expect("canonical log path"),
            log_directory
                .join("wrapper.log")
                .canonicalize()
                .expect("canonical expected path")
        );
        drop(logger);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn rotation_keeps_exactly_archives_one_through_fifty() {
        let directory = test_directory("fifty-archives");
        let active = directory.join("wrapper.log");
        fs::write(&active, "active").expect("write active log");
        for index in 1..=50 {
            fs::write(rolled_path(&active, index), index.to_string()).expect("write rolled log");
        }
        rotate(&active, 50).expect("rotate logs");
        assert_eq!(
            fs::read_to_string(rolled_path(&active, 1)).expect("read first archive"),
            "active"
        );
        assert_eq!(
            fs::read_to_string(rolled_path(&active, 50)).expect("read last archive"),
            "49"
        );
        assert!(!rolled_path(&active, 51).exists());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn rotates_at_the_configured_fifty_mebibyte_boundary() {
        let directory = test_directory("fifty-mebibyte-boundary");
        let active = directory.join("wrapper.log");
        fs::File::create(&active)
            .expect("create sparse active log")
            .set_len(50 * 1024 * 1024)
            .expect("extend active log to 50 MiB");
        for index in 1..=50 {
            fs::write(rolled_path(&active, index), index.to_string()).expect("write rolled log");
        }
        let configuration_path = directory.join("wrapper.conf");
        fs::write(
            &configuration_path,
            "wrapper.logfile=wrapper.log\n\
             wrapper.logfile.rollmode=SIZE\n\
             wrapper.logfile.maxfiles=50\n\
             wrapper.logfile.maxsize=50m\n",
        )
        .expect("write config");
        let config = Config::load(&configuration_path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("create logger");
        logger
            .write(LogLevel::Info, LogSource::Wrapper, "after boundary")
            .expect("write first line after boundary");
        drop(logger);

        assert_eq!(fs::metadata(&active).expect("active metadata").len(), 58);
        assert_eq!(
            fs::metadata(rolled_path(&active, 1))
                .expect("first archive metadata")
                .len(),
            50 * 1024 * 1024
        );
        assert_eq!(
            fs::read_to_string(rolled_path(&active, 50)).expect("read last archive"),
            "49"
        );
        assert!(!rolled_path(&active, 51).exists());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn never_falls_back_when_the_configured_log_directory_is_missing() {
        let directory = test_directory("missing-log-directory");
        let configuration_path = directory.join("wrapper.conf");
        fs::write(&configuration_path, "wrapper.logfile=missing/wrapper.log\n")
            .expect("write config");
        let config = Config::load(&configuration_path, &directory, &[]).expect("load config");
        assert!(LogWriter::from_config(&config).is_err());
        assert!(!directory.join("wrapper.log").exists());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn recovers_from_interruption_during_archive_shifting() {
        let directory = test_directory("interrupted-archive-shift");
        let active = directory.join("wrapper.log");
        fs::File::create(&active)
            .expect("create sparse active log")
            .set_len(50 * 1024 * 1024)
            .expect("extend active log to 50 MiB");
        for index in 1..=50 {
            fs::write(rolled_path(&active, index), index.to_string()).expect("write rolled log");
        }

        fs::remove_file(rolled_path(&active, 50)).expect("simulate oldest removal");
        fs::rename(rolled_path(&active, 49), rolled_path(&active, 50))
            .expect("simulate first completed shift");
        fs::rename(rolled_path(&active, 48), rolled_path(&active, 49))
            .expect("simulate second completed shift");

        let configuration_path = directory.join("wrapper.conf");
        fs::write(
            &configuration_path,
            "wrapper.logfile=wrapper.log\n\
             wrapper.logfile.rollmode=SIZE\n\
             wrapper.logfile.maxfiles=50\n\
             wrapper.logfile.maxsize=50m\n",
        )
        .expect("write config");
        let config = Config::load(&configuration_path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("recover logger");
        logger
            .write(LogLevel::Info, LogSource::Wrapper, "after recovery")
            .expect("write after recovery");
        drop(logger);

        assert!(active.exists());
        assert_eq!(
            fs::metadata(rolled_path(&active, 1))
                .expect("first archive metadata")
                .len(),
            50 * 1024 * 1024
        );
        assert!(rolled_path(&active, 50).exists());
        assert!(!rolled_path(&active, 51).exists());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn recovers_when_interrupted_after_moving_the_active_log() {
        let directory = test_directory("interrupted-after-active-move");
        let active = directory.join("wrapper.log");
        let first_archive = rolled_path(&active, 1);
        fs::write(&first_archive, "previous active").expect("simulate completed active move");
        let configuration_path = directory.join("wrapper.conf");
        fs::write(
            &configuration_path,
            "wrapper.logfile=wrapper.log\n\
             wrapper.logfile.rollmode=SIZE\n\
             wrapper.logfile.maxfiles=50\n\
             wrapper.logfile.maxsize=50m\n",
        )
        .expect("write config");
        let config = Config::load(&configuration_path, &directory, &[]).expect("load config");
        let mut logger = LogWriter::from_config(&config).expect("reopen active log");
        logger
            .write(LogLevel::Info, LogSource::Wrapper, "after recovery")
            .expect("write new active log");
        drop(logger);

        assert!(active.exists());
        assert_eq!(
            fs::read_to_string(first_archive).expect("read preserved archive"),
            "previous active"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    fn test_directory(label: &str) -> std::path::PathBuf {
        crate::test_support::unique_directory(label)
    }
}
