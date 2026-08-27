// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyCommand {
    Console,
    Service,
    Start,
    Pause,
    Resume,
    Stop,
    Install,
    InstallStart,
    Remove,
    ControlCode(u32),
    Dump,
    HeapDump,
    Query { silent: bool },
    Version,
    Help,
}

impl LegacyCommand {
    #[must_use]
    pub fn needs_configuration(&self) -> bool {
        !matches!(self, Self::Version | Self::Help)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub command: LegacyCommand,
    pub configuration: PathBuf,
    pub overrides: Vec<String>,
    pub application_args: Vec<OsString>,
}

impl Cli {
    pub fn parse_env() -> Result<Self> {
        Self::parse(std::env::args_os().skip(1))
    }

    pub fn parse<I, S>(arguments: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
        let separator = arguments.iter().position(|argument| argument == "--");
        let (wrapper_args, application_args) = match separator {
            Some(index) => (&arguments[..index], arguments[index + 1..].to_vec()),
            None => (arguments.as_slice(), Vec::new()),
        };

        let mut cursor = 0;
        let mut explicit_command = false;
        let command = if let Some(first) = wrapper_args.first() {
            match parse_command(first)? {
                Some(command) => {
                    cursor = 1;
                    explicit_command = true;
                    command
                }
                None if first.to_string_lossy().starts_with('-') => {
                    return Err(Error::Cli(format!(
                        "unknown command: {}",
                        first.to_string_lossy()
                    )));
                }
                None => LegacyCommand::Console,
            }
        } else {
            LegacyCommand::Console
        };

        let mut configuration = PathBuf::from("wrapper.conf");
        if command.needs_configuration() && cursor < wrapper_args.len() {
            let candidate = &wrapper_args[cursor];
            if !looks_like_override(candidate) {
                configuration = PathBuf::from(candidate);
                cursor += 1;
            } else if !explicit_command {
                return Err(Error::Cli("missing configuration file".into()));
            }
        }

        let mut overrides = Vec::new();
        for argument in &wrapper_args[cursor..] {
            let text = argument.to_str().ok_or_else(|| {
                Error::Cli("configuration property overrides must be valid Unicode".into())
            })?;
            if !text.contains('=') {
                return Err(Error::Cli(format!(
                    "expected a name=value property, found: {text}"
                )));
            }
            overrides.push(text.to_owned());
        }

        Ok(Self {
            command,
            configuration,
            overrides,
            application_args,
        })
    }

    #[must_use]
    pub fn resolve_configuration(&self, executable_directory: &Path) -> PathBuf {
        if self.configuration.is_absolute() {
            self.configuration.clone()
        } else {
            executable_directory.join(&self.configuration)
        }
    }
}

fn looks_like_override(value: &OsStr) -> bool {
    value.to_string_lossy().contains('=')
}

fn parse_command(argument: &OsStr) -> Result<Option<LegacyCommand>> {
    let text = argument.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    let command = match lower.as_str() {
        "-c" | "--console" => Some(LegacyCommand::Console),
        "-s" | "--service" => Some(LegacyCommand::Service),
        "-t" | "--start" => Some(LegacyCommand::Start),
        "-a" | "--pause" => Some(LegacyCommand::Pause),
        "-e" | "--resume" => Some(LegacyCommand::Resume),
        "-p" | "--stop" => Some(LegacyCommand::Stop),
        "-i" | "--install" => Some(LegacyCommand::Install),
        "-it" | "--installstart" => Some(LegacyCommand::InstallStart),
        "-r" | "--remove" => Some(LegacyCommand::Remove),
        "-d" | "--dump" => Some(LegacyCommand::Dump),
        "--heapdump" => Some(LegacyCommand::HeapDump),
        "-q" | "--query" => Some(LegacyCommand::Query { silent: false }),
        "-qs" | "--querysilent" => Some(LegacyCommand::Query { silent: true }),
        "-v" | "--version" => Some(LegacyCommand::Version),
        "-?" | "--help" | "-h" => Some(LegacyCommand::Help),
        _ => {
            if let Some(code) = lower
                .strip_prefix("-l=")
                .or_else(|| lower.strip_prefix("--controlcode="))
            {
                let code = code
                    .parse::<u32>()
                    .map_err(|_| Error::Cli(format!("invalid control code: {code}")))?;
                Some(LegacyCommand::ControlCode(code))
            } else {
                None
            }
        }
    };
    Ok(command)
}

pub const HELP: &str = include_str!("help.txt");

#[must_use]
pub fn help_text() -> String {
    format!("{}\n{HELP}", crate::version_text())
}

#[cfg(test)]
mod tests {
    use super::{Cli, HELP, LegacyCommand, help_text};
    use crate::config::{
        SUPPORTED_JSS_PROPERTIES, SUPPORTED_NUMBERED_WRAPPER_PROPERTY_PREFIXES,
        SUPPORTED_WRAPPER_PROPERTIES,
    };
    use std::ffi::OsString;
    use std::path::Path;

    #[test]
    fn defaults_to_console_and_wrapper_conf() {
        let cli = Cli::parse(Vec::<OsString>::new()).expect("valid CLI");
        assert_eq!(cli.command, LegacyCommand::Console);
        assert_eq!(cli.configuration, Path::new("wrapper.conf"));
    }

    #[test]
    fn accepts_implicit_console_configuration() {
        let cli = Cli::parse(["conf/app.conf", "wrapper.debug=true"])
            .expect("valid implicit console CLI");
        assert_eq!(cli.command, LegacyCommand::Console);
        assert_eq!(cli.configuration, Path::new("conf/app.conf"));
        assert_eq!(cli.overrides, ["wrapper.debug=true"]);
    }

    #[test]
    fn preserves_service_command_and_pass_through_arguments() {
        let cli = Cli::parse([
            "-s",
            "wrapper.conf",
            "wrapper.debug=true",
            "--",
            "--server-config=x.xml",
            "value with spaces",
        ])
        .expect("valid service CLI");
        assert_eq!(cli.command, LegacyCommand::Service);
        assert_eq!(cli.overrides, ["wrapper.debug=true"]);
        assert_eq!(
            cli.application_args,
            ["--server-config=x.xml", "value with spaces"]
        );
    }

    #[test]
    fn parses_control_code() {
        let cli = Cli::parse(["-l=201"]).expect("valid control code");
        assert_eq!(cli.command, LegacyCommand::ControlCode(201));
    }

    #[test]
    fn parses_heap_dump_extension() {
        let cli = Cli::parse(["--heapdump", "wrapper.conf"]).expect("valid heap dump command");
        assert_eq!(cli.command, LegacyCommand::HeapDump);
        assert_eq!(cli.configuration, Path::new("wrapper.conf"));
    }

    #[test]
    fn every_help_alias_prints_without_needing_configuration() {
        for alias in ["-?", "-h", "--help"] {
            let cli = Cli::parse([alias]).expect("parse help alias");
            assert_eq!(cli.command, LegacyCommand::Help);
            assert!(!cli.command.needs_configuration());
        }
    }

    #[test]
    fn help_starts_with_the_version_text_and_names_no_third_party() {
        let help = help_text();
        assert!(help.starts_with(&crate::version_text()));
        for forbidden in crate::FOREIGN_NAME_MARKERS.iter().chain(&["Copyright"]) {
            assert!(
                !help.contains(forbidden),
                "help must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn embedded_help_mentions_every_recognized_legacy_and_own_property() {
        for property in SUPPORTED_WRAPPER_PROPERTIES {
            assert!(
                HELP.contains(property),
                "embedded help is missing supported property {property}"
            );
        }
        for prefix in SUPPORTED_NUMBERED_WRAPPER_PROPERTY_PREFIXES {
            assert!(
                HELP.contains(prefix),
                "embedded help is missing supported property family {prefix}<n>"
            );
        }
        for property in SUPPORTED_JSS_PROPERTIES {
            assert!(
                HELP.contains(property),
                "embedded help is missing own property {property}"
            );
        }
        for integration_method in ["SimpleApp", "StartStopApp", "JarApp", "ServiceListener"] {
            assert!(
                HELP.contains(integration_method),
                "embedded help is missing integration method {integration_method}"
            );
        }
    }
}
