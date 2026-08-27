// SPDX-License-Identifier: Apache-2.0 OR MIT
use crate::cli::Cli;
use crate::config::Config;
use crate::error::{Error, Result};

#[must_use]
pub fn installed_launch_arguments(cli: &Cli) -> Vec<std::ffi::OsString> {
    let mut arguments = vec![
        std::ffi::OsString::from("-s"),
        cli.configuration.as_os_str().into(),
    ];
    arguments.extend(
        cli.overrides
            .iter()
            .filter(|value| !value.starts_with("wrapper.ntservice.password="))
            .map(std::ffi::OsString::from),
    );
    if !cli.application_args.is_empty() {
        arguments.push("--".into());
        arguments.extend(cli.application_args.iter().cloned());
    }
    arguments
}

#[cfg(windows)]
mod platform {
    use std::ffi::{OsStr, OsString};
    use std::sync::{Arc, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_service::define_windows_service;
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceDependency,
        ServiceErrorControl, ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState,
        ServiceStatus, ServiceType, UserEventCode,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    use super::{Cli, Config, Error, Result, installed_launch_arguments};
    use crate::cli::LegacyCommand;
    use crate::supervisor::{Control, Supervisor};
    use crate::telemetry::{Event, EventKind, EventPublisher};
    use crate::windows_process;

    const SCM_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
    const STOP_WAIT_MARGIN: Duration = Duration::from_secs(10);

    define_windows_service!(ffi_service_main, windows_service_main);

    pub fn run_dispatcher(_cli: &Cli, config: &Config) -> Result<i32> {
        let service_name = service_name(config)?;
        service_dispatcher::start(service_name, ffi_service_main).map_err(service_error)?;
        Ok(0)
    }

    pub fn execute_command(cli: &Cli, config: &Config) -> Result<i32> {
        match cli.command {
            LegacyCommand::Install => {
                install(cli, config)?;
                Ok(0)
            }
            LegacyCommand::InstallStart => {
                install(cli, config)?;
                start(config)?;
                Ok(0)
            }
            LegacyCommand::Start => {
                start(config)?;
                Ok(0)
            }
            LegacyCommand::Stop => {
                stop(config)?;
                Ok(0)
            }
            LegacyCommand::Pause => {
                let service = open_service(
                    config,
                    ServiceAccess::PAUSE_CONTINUE | ServiceAccess::QUERY_STATUS,
                )?;
                service.pause().map_err(service_error)?;
                wait_for_state(&service, ServiceState::Paused, SCM_WAIT_TIMEOUT)?;
                Ok(0)
            }
            LegacyCommand::Resume => {
                let service = open_service(
                    config,
                    ServiceAccess::PAUSE_CONTINUE | ServiceAccess::QUERY_STATUS,
                )?;
                service.resume().map_err(service_error)?;
                wait_for_state(&service, ServiceState::Running, SCM_WAIT_TIMEOUT)?;
                Ok(0)
            }
            LegacyCommand::Remove => {
                remove(config)?;
                Ok(0)
            }
            LegacyCommand::ControlCode(code) => {
                notify(config, code)?;
                Ok(0)
            }
            LegacyCommand::Dump => {
                let code = config.get_u64("wrapper.thread_dump_control_code", 255);
                notify(
                    config,
                    u32::try_from(code).map_err(|_| {
                        Error::Config("wrapper.thread_dump_control_code is out of range".into())
                    })?,
                )?;
                Ok(0)
            }
            LegacyCommand::HeapDump => {
                notify(config, heap_dump_control_code(config)?)?;
                Ok(0)
            }
            LegacyCommand::Query { silent } => query(config, silent),
            _ => Err(Error::Service(
                "the command is not a service operation".into(),
            )),
        }
    }

    fn install(cli: &Cli, config: &Config) -> Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .map_err(service_error)?;
        let account_name = config
            .get("wrapper.ntservice.account")
            .filter(|value| !value.trim().is_empty())
            .map(OsString::from);
        let account_password = account_name
            .as_ref()
            .and_then(|_| config.get("wrapper.ntservice.password").map(OsString::from));
        let service_type =
            if account_name.is_none() && config.get_bool("wrapper.ntservice.interactive", false) {
                ServiceType::OWN_PROCESS | ServiceType::INTERACTIVE_PROCESS
            } else {
                ServiceType::OWN_PROCESS
            };
        let dependencies = config
            .numbered("wrapper.ntservice.dependency")
            .into_iter()
            .filter_map(|(_, value)| {
                let value = value.trim();
                if value.is_empty() {
                    None
                } else if let Some(group) = value.strip_prefix('+') {
                    Some(ServiceDependency::Group(group.into()))
                } else {
                    Some(ServiceDependency::Service(value.into()))
                }
            })
            .collect();
        let info = ServiceInfo {
            name: service_name(config)?.into(),
            display_name: config
                .get_or("wrapper.ntservice.displayname", service_name(config)?)
                .into(),
            service_type,
            start_type: start_type(config)?,
            error_control: ServiceErrorControl::Normal,
            executable_path: std::env::current_exe()?,
            launch_arguments: installed_launch_arguments(cli),
            dependencies,
            account_name: account_name.clone(),
            account_password,
        };
        let service = manager
            .create_service(
                &info,
                ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::CHANGE_CONFIG,
            )
            .map_err(service_error)?;
        if let Some(description) = config.get("wrapper.ntservice.description") {
            service
                .set_description(description)
                .map_err(service_error)?;
        }
        println!(
            "Service '{}' installed with {} ({:?}).",
            service_name(config)?,
            if account_name.is_some() {
                "the configured account"
            } else {
                "LocalSystem"
            },
            start_type(config)?
        );
        Ok(())
    }

    fn start(config: &Config) -> Result<()> {
        let service = open_service(config, ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;
        service.start::<&OsStr>(&[]).map_err(service_error)?;
        let timeout = Duration::from_secs(config.get_u64("wrapper.startup.timeout", 30))
            .max(SCM_WAIT_TIMEOUT);
        wait_for_state(&service, ServiceState::Running, timeout)?;
        println!("Service '{}' started.", service_name(config)?);
        Ok(())
    }

    fn stop(config: &Config) -> Result<()> {
        let service = open_service(config, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?;
        if service.query_status().map_err(service_error)?.current_state != ServiceState::Stopped {
            service.stop().map_err(service_error)?;
            wait_for_state(&service, ServiceState::Stopped, stop_wait_hint(config))?;
        }
        println!("Service '{}' stopped.", service_name(config)?);
        Ok(())
    }

    fn remove(config: &Config) -> Result<()> {
        let service = open_service(
            config,
            ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE,
        )?;
        if service.query_status().map_err(service_error)?.current_state != ServiceState::Stopped {
            service.stop().map_err(service_error)?;
            wait_for_state(&service, ServiceState::Stopped, stop_wait_hint(config))?;
        }
        service.delete().map_err(service_error)?;
        println!("Service '{}' removed.", service_name(config)?);
        Ok(())
    }

    fn notify(config: &Config, raw_code: u32) -> Result<()> {
        let code = UserEventCode::from_raw(raw_code).map_err(|_| {
            Error::Config(format!(
                "the control code must be between 128 and 255: {raw_code}"
            ))
        })?;
        open_service(
            config,
            ServiceAccess::USER_DEFINED_CONTROL | ServiceAccess::QUERY_STATUS,
        )?
        .notify(code)
        .map_err(service_error)?;
        Ok(())
    }

    fn heap_dump_control_code(config: &Config) -> Result<u32> {
        let raw = config.get_u64("jss.heapdump.control_code", 254);
        let code = u32::try_from(raw).map_err(|_| {
            Error::Config("jss.heapdump.control_code is outside the valid range".into())
        })?;
        if !(128..=255).contains(&code) {
            return Err(Error::Config(
                "jss.heapdump.control_code must be between 128 and 255".into(),
            ));
        }
        Ok(code)
    }

    fn query(config: &Config, silent: bool) -> Result<i32> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(service_error)?;
        let Ok(service) = manager.open_service(
            service_name(config)?,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
        ) else {
            if !silent {
                println!("Service '{}' is not installed.", service_name(config)?);
            }
            return Ok(0);
        };
        let status = service.query_status().map_err(service_error)?;
        let service_config = service.query_config().map_err(service_error)?;
        let mut mask = 1;
        if status.current_state != ServiceState::Stopped {
            mask |= 2;
        }
        if status.current_state == ServiceState::Paused {
            mask |= 64;
        }
        if service_config
            .service_type
            .contains(ServiceType::INTERACTIVE_PROCESS)
        {
            mask |= 4;
        }
        mask |= match service_config.start_type {
            ServiceStartType::AutoStart => 8,
            ServiceStartType::OnDemand => 16,
            ServiceStartType::Disabled => 32,
            _ => 0,
        };
        if !silent {
            println!(
                "Service '{}': {:?}, start type {:?} (status mask {}).",
                service_name(config)?,
                status.current_state,
                service_config.start_type,
                mask
            );
        }
        Ok(mask)
    }

    fn wait_for_state(
        service: &windows_service::service::Service,
        expected: ServiceState,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = service.query_status().map_err(service_error)?;
            if status.current_state == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Service(format!(
                    "timed out waiting for state {expected:?}; current state is {:?}",
                    status.current_state
                )));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    fn open_service(
        config: &Config,
        access: ServiceAccess,
    ) -> Result<windows_service::service::Service> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(service_error)?;
        manager
            .open_service(service_name(config)?, access)
            .map_err(service_error)
    }

    fn start_type(config: &Config) -> Result<ServiceStartType> {
        match config
            .get_or("wrapper.ntservice.starttype", "AUTO_START")
            .trim()
            .to_ascii_uppercase()
            .as_str()
        {
            "AUTO_START" | "AUTO" => Ok(ServiceStartType::AutoStart),
            "DEMAND_START" | "MANUAL" => Ok(ServiceStartType::OnDemand),
            "DISABLED" => Ok(ServiceStartType::Disabled),
            value => Err(Error::Config(format!(
                "unsupported wrapper.ntservice.starttype: {value}"
            ))),
        }
    }

    fn service_name(config: &Config) -> Result<&str> {
        config.required("wrapper.ntservice.name")
    }

    fn service_error(error: windows_service::Error) -> Error {
        Error::Service(error.to_string())
    }

    /// Worst-case time the service may need to stop: the graceful JVM
    /// shutdown, an optional thread dump grace period, and a margin.
    fn stop_wait_hint(config: &Config) -> Duration {
        let mut hint = Duration::from_secs(config.get_u64("wrapper.shutdown.timeout", 30));
        if config.get_bool("wrapper.request_thread_dump_on_failed_jvm_exit", false) {
            hint += Duration::from_secs(
                config.get_u64("wrapper.request_thread_dump_on_failed_jvm_exit.delay", 5),
            );
        }
        hint + STOP_WAIT_MARGIN
    }

    fn windows_service_main(_arguments: Vec<OsString>) {
        if let Err(error) = run_service_worker() {
            // The log file may not exist yet (for example when wrapper.conf
            // cannot be read), so the failure is recorded where an operator
            // can still find it.
            let source = service_source_name().unwrap_or_else(|| "Java Service Steward".into());
            let message = format!("Java Service Steward could not run the service: {error}");
            if windows_process::report_event_log_error(&source, &message).is_err() {
                eprintln!("{message}");
            }
        }
    }

    fn service_source_name() -> Option<String> {
        let cli = Cli::parse_env().ok()?;
        let executable = std::env::current_exe().ok()?;
        let executable_directory = executable.parent()?;
        let config = Config::load(
            cli.resolve_configuration(executable_directory),
            executable_directory,
            &cli.overrides,
        )
        .ok()?;
        config.get("wrapper.ntservice.name").map(str::to_owned)
    }

    fn run_service_worker() -> Result<()> {
        let cli = Cli::parse_env()?;
        let executable = std::env::current_exe()?;
        let executable_directory = executable
            .parent()
            .ok_or_else(|| Error::Config("the executable has no parent directory".into()))?;
        let config = Config::load(
            cli.resolve_configuration(executable_directory),
            executable_directory,
            &cli.overrides,
        )?;
        let name = service_name(&config)?.to_owned();
        let dump_code = config.get_u64("wrapper.thread_dump_control_code", 255);
        let heap_dump_code = u64::from(heap_dump_control_code(&config)?);
        if heap_dump_code == dump_code {
            return Err(Error::Config(
                "jss.heapdump.control_code must differ from wrapper.thread_dump_control_code"
                    .into(),
            ));
        }
        let pausable = is_pausable(&config);
        let stop_hint_ms = u64::try_from(stop_wait_hint(&config).as_millis()).unwrap_or(u64::MAX);
        let (control_sender, control_receiver) = crossbeam_channel::bounded(32);
        let status_slot = Arc::new(OnceLock::<ServiceStatusHandle>::new());
        let handler_status_slot = Arc::clone(&status_slot);
        let handler = move |event| -> ServiceControlHandlerResult {
            let control = match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    if let Some(handle) = handler_status_slot.get() {
                        let _ = set_status(
                            handle,
                            ServiceState::StopPending,
                            1,
                            stop_hint_ms,
                            pausable,
                        );
                    }
                    Some(Control::Stop)
                }
                ServiceControl::Pause if pausable => {
                    if let Some(handle) = handler_status_slot.get() {
                        let _ = set_status(
                            handle,
                            ServiceState::PausePending,
                            1,
                            stop_hint_ms,
                            pausable,
                        );
                    }
                    Some(Control::Pause)
                }
                ServiceControl::Continue if pausable => {
                    if let Some(handle) = handler_status_slot.get() {
                        let _ =
                            set_status(handle, ServiceState::ContinuePending, 1, 30_000, pausable);
                    }
                    Some(Control::Resume)
                }
                ServiceControl::Pause | ServiceControl::Continue => {
                    return ServiceControlHandlerResult::NotImplemented;
                }
                ServiceControl::UserEvent(code) if u64::from(code.to_raw()) == dump_code => {
                    Some(Control::ThreadDump)
                }
                ServiceControl::UserEvent(code) if u64::from(code.to_raw()) == heap_dump_code => {
                    Some(Control::HeapDump)
                }
                ServiceControl::UserEvent(code) => Some(Control::User(code.to_raw())),
                ServiceControl::Interrogate => return ServiceControlHandlerResult::NoError,
                _ => return ServiceControlHandlerResult::NotImplemented,
            };
            if let Some(control) = control {
                let _ = control_sender.try_send(control);
            }
            ServiceControlHandlerResult::NoError
        };
        let status_handle =
            service_control_handler::register(&name, handler).map_err(service_error)?;
        status_slot
            .set(status_handle)
            .map_err(|_| Error::Service("the status handler was already registered".into()))?;
        set_status(
            &status_handle,
            ServiceState::StartPending,
            1,
            config
                .get_u64("wrapper.startup.timeout", 30)
                .max(1)
                .saturating_mul(1_000),
            pausable,
        )?;

        let (events, event_receiver) = EventPublisher::bounded(512);
        let reporter_handle = status_handle;
        let status_reporter = thread::spawn(move || -> Result<()> {
            while let Ok(Event { kind, .. }) = event_receiver.recv() {
                match kind {
                    EventKind::JvmStarted { .. } => {
                        set_status(&reporter_handle, ServiceState::Running, 0, 0, pausable)?
                    }
                    EventKind::ServicePaused => {
                        set_status(&reporter_handle, ServiceState::Paused, 0, 0, pausable)?
                    }
                    EventKind::ServiceResumed => {
                        set_status(&reporter_handle, ServiceState::Running, 0, 0, pausable)?
                    }
                    _ => {}
                }
            }
            Ok(())
        });
        let result = Supervisor::new(
            &config,
            &cli.application_args,
            true,
            control_receiver,
            events,
        )
        .run();
        let reporter_result = status_reporter
            .join()
            .map_err(|_| Error::Service("the status reporter thread ended unexpectedly".into()))?;
        let code = result.as_ref().copied().unwrap_or(1);
        status_handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(u32::try_from(code).unwrap_or(1)),
                checkpoint: 0,
                wait_hint: Duration::ZERO,
                process_id: None,
            })
            .map_err(service_error)?;
        reporter_result?;
        result.map(|_| ())
    }

    fn set_status(
        handle: &ServiceStatusHandle,
        state: ServiceState,
        checkpoint: u32,
        wait_hint_ms: u64,
        pausable: bool,
    ) -> Result<()> {
        let accepted = if matches!(state, ServiceState::StartPending | ServiceState::Stopped) {
            ServiceControlAccept::empty()
        } else {
            let mut accepted = ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN;
            if pausable {
                accepted |= ServiceControlAccept::PAUSE_CONTINUE;
            }
            accepted
        };
        handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accepted,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint,
                wait_hint: Duration::from_millis(wait_hint_ms),
                process_id: None,
            })
            .map_err(service_error)
    }

    fn is_pausable(config: &Config) -> bool {
        config.get_bool(
            "wrapper.pausable",
            config.get_bool("wrapper.ntservice.pausable", false),
        )
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{Cli, Config, Error, Result};

    pub fn run_dispatcher(_cli: &Cli, _config: &Config) -> Result<i32> {
        Err(Error::UnsupportedPlatform("Windows services"))
    }

    pub fn execute_command(_cli: &Cli, _config: &Config) -> Result<i32> {
        Err(Error::UnsupportedPlatform("Windows services"))
    }
}

pub use platform::{execute_command, run_dispatcher};

#[cfg(test)]
mod tests {
    use super::installed_launch_arguments;
    use crate::cli::Cli;

    #[test]
    fn install_rewrites_only_the_command_to_hidden_service_mode() {
        let cli = Cli::parse([
            "-i",
            "wrapper.conf",
            "wrapper.debug=false",
            "--",
            "application-argument",
        ])
        .expect("parse install command");
        assert_eq!(
            installed_launch_arguments(&cli),
            [
                "-s",
                "wrapper.conf",
                "wrapper.debug=false",
                "--",
                "application-argument"
            ]
        );
    }

    #[test]
    fn install_does_not_persist_the_service_password_in_image_path() {
        let cli = Cli::parse([
            "-i",
            "wrapper.conf",
            "wrapper.ntservice.account=synthetic-user",
            "wrapper.ntservice.password=synthetic-password",
        ])
        .expect("parse install command");
        assert_eq!(
            installed_launch_arguments(&cli),
            [
                "-s",
                "wrapper.conf",
                "wrapper.ntservice.account=synthetic-user"
            ]
        );
    }
}
