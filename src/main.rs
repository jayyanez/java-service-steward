// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::process::ExitCode;

use java_service_steward::cli::{Cli, LegacyCommand, help_text};
use java_service_steward::config::Config;
use java_service_steward::error::Result;
use java_service_steward::supervisor::{Supervisor, console_control_channel};
use java_service_steward::telemetry::EventPublisher;
use java_service_steward::{service, version_text};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("Java Service Steward: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse_env()?;
    match cli.command {
        LegacyCommand::Help => {
            print!("{}", help_text());
            return Ok(0);
        }
        LegacyCommand::Version => {
            print!("{}", version_text());
            return Ok(0);
        }
        _ => {}
    }

    let executable = std::env::current_exe()?;
    let executable_directory = executable.parent().ok_or_else(|| {
        java_service_steward::error::Error::Config("the executable has no parent directory".into())
    })?;
    let configuration_path = cli.resolve_configuration(executable_directory);
    let config = Config::load(&configuration_path, executable_directory, &cli.overrides)?;

    match cli.command {
        LegacyCommand::Console => {
            // The supervisor writes configuration warnings to the log, which
            // also reaches the console in this mode.
            let (_control_sender, controls) = console_control_channel()?;
            let (events, _event_receiver) = EventPublisher::bounded(512);
            Supervisor::new(&config, &cli.application_args, false, controls, events).run()
        }
        LegacyCommand::Service => service::run_dispatcher(&cli, &config),
        _ => {
            for warning in config.warnings() {
                eprintln!("Java Service Steward: warning: {}", warning.message);
            }
            service::execute_command(&cli, &config)
        }
    }
}
