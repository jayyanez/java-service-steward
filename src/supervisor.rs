// SPDX-License-Identifier: Apache-2.0 OR MIT
//! JVM lifecycle state machine: launch, start-up deadline, ping watchdog,
//! output filters, restart throttling, pause/resume and shutdown.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, never, select, tick};

use crate::config::{Config, WarningLevel};
use crate::error::{Error, Result};
use crate::heap_dump;
use crate::jvm::{BackendLaunch, JvmCommand};
use crate::logging::{
    FilterAction, FilterMatch, Filters, LogLevel, LogSource, LogWriter, java_command_log_level,
    low_log_level,
};
use crate::protocol::{
    self, BackendListener, Connection, LOG_BASE, LOGFILE, LOW_LOG_LEVEL, PING, PROPERTIES, RESTART,
    ReceiveEvent, START, START_PENDING, STARTED, STOP, STOP_PENDING, STOPPED,
};
use crate::telemetry::{EventKind, EventPublisher};
use crate::thread_dump::{self, Method as ThreadDumpMethod};
use crate::windows_process::{self, JobObject};

/// Log markers kept stable for monitoring tools.
pub const STARTED_AS_SERVICE_MARKER: &str = "--> Wrapper Started as Service";
pub const STARTED_AS_CONSOLE_MARKER: &str = "--> Wrapper Started as Console";
pub const STOPPED_MARKER: &str = "<-- Wrapper Stopped";

const TICK_INTERVAL: Duration = Duration::from_millis(50);
const OUTPUT_BATCH: usize = 4096;
const MAX_OUTPUT_LINE: usize = 64 * 1024;
const MIN_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Stop,
    ConsoleInterrupt,
    ConsoleBreak,
    Pause,
    Resume,
    ThreadDump,
    HeapDump,
    User(u32),
}

#[derive(Debug)]
struct OutputLine {
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOutcome {
    Stop(i32),
    Pause,
    Exited {
        elapsed: Duration,
        exit_code: i32,
    },
    Restart {
        elapsed: Duration,
        automatic: bool,
        exit_code: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PausedOutcome {
    Resume,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitAction {
    Shutdown,
    Restart,
    Pause,
}

struct OwnedPidFile {
    path: PathBuf,
    contents: Vec<u8>,
}

impl OwnedPidFile {
    fn create(config: &Config, property: &str, pid: u32, strict: bool) -> Result<Option<Self>> {
        let Some(value) = config
            .get(property)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let path = config.resolve_path(value);
        if strict && path.exists() {
            return Err(Error::Config(format!(
                "{property} already exists and wrapper.pidfile.strict is enabled: {}",
                path.display()
            )));
        }
        let contents = format!("{pid}\r\n").into_bytes();
        fs::write(&path, &contents)?;
        Ok(Some(Self { path, contents }))
    }

    fn update(&mut self, value: u32) -> Result<()> {
        let contents = format!("{value}\r\n").into_bytes();
        fs::write(&self.path, &contents)?;
        self.contents = contents;
        Ok(())
    }
}

impl Drop for OwnedPidFile {
    fn drop(&mut self) {
        if fs::read(&self.path).is_ok_and(|current| current == self.contents) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub struct Supervisor<'a> {
    config: &'a Config,
    application_args: &'a [OsString],
    service: bool,
    controls: Receiver<Control>,
    events: EventPublisher,
}

impl<'a> Supervisor<'a> {
    #[must_use]
    pub fn new(
        config: &'a Config,
        application_args: &'a [OsString],
        service: bool,
        controls: Receiver<Control>,
        events: EventPublisher,
    ) -> Self {
        Self {
            config,
            application_args,
            service,
            controls,
            events,
        }
    }

    pub fn run(&self) -> Result<i32> {
        let _wrapper_pid_file = OwnedPidFile::create(
            self.config,
            "wrapper.pidfile",
            std::process::id(),
            self.config.get_bool("wrapper.pidfile.strict", false),
        )?;
        self.events.publish(EventKind::WrapperStarted);
        let mut logger = LogWriter::from_config_with_console(self.config, !self.service)?;
        if self.service
            && let Err(error) = windows_process::prepare_service_console(
                self.config
                    .get_bool("wrapper.ntservice.generate_console", true),
            )
        {
            logger.write(
                LogLevel::Warn,
                LogSource::Wrapper,
                &format!("Unable to allocate the service console: {error}"),
            )?;
        }
        if let Some(title) = self
            .config
            .get("wrapper.console.title.windows")
            .or_else(|| self.config.get("wrapper.console.title"))
            && let Err(error) = windows_process::set_console_title(title)
        {
            logger.write(
                LogLevel::Warn,
                LogSource::Wrapper,
                &format!("Unable to set the console title: {error}"),
            )?;
        }
        logger.write(
            LogLevel::Status,
            LogSource::Wrapper,
            if self.service {
                STARTED_AS_SERVICE_MARKER
            } else {
                STARTED_AS_CONSOLE_MARKER
            },
        )?;
        logger.write(
            LogLevel::Status,
            LogSource::Wrapper,
            &crate::product_banner(),
        )?;
        logger.write(LogLevel::Status, LogSource::Wrapper, "")?;
        for warning in self.config.warnings() {
            let level = match warning.level {
                WarningLevel::Info => LogLevel::Info,
                WarningLevel::Warn => LogLevel::Warn,
            };
            logger.write(level, LogSource::Wrapper, &warning.message)?;
        }
        let filters = Filters::from_config(self.config);
        let mut jvm_id = 1_u32;
        let mut java_id_file: Option<OwnedPidFile> = None;
        let mut failed_invocations = 0_u64;
        let mut resuming = false;

        if self.config.get_bool("wrapper.pause_on_startup", false) && self.pausable() {
            self.events.publish(EventKind::ServicePaused);
            match self.wait_while_paused() {
                PausedOutcome::Resume => resuming = true,
                PausedOutcome::Stop => {
                    logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
                    return Ok(0);
                }
            }
        }
        let startup_delay = startup_delay(self.config, self.service);
        if startup_delay > Duration::ZERO && self.wait_for_restart(startup_delay) {
            logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
            return Ok(0);
        }

        loop {
            let java_id_result = match java_id_file.as_mut() {
                Some(file) => file.update(jvm_id),
                None => {
                    match OwnedPidFile::create(self.config, "wrapper.java.idfile", jvm_id, false) {
                        Ok(file) => {
                            java_id_file = file;
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = java_id_result {
                logger.write(
                    LogLevel::Warn,
                    LogSource::Wrapper,
                    &format!("Unable to write wrapper.java.idfile: {error}"),
                )?;
            }
            if jvm_id > 1 {
                logger.roll_for_jvm_restart()?;
            }
            self.events.publish(EventKind::JvmLaunching { id: jvm_id });
            let outcome = self.run_once(jvm_id, resuming, &mut logger, &filters)?;
            let outcome = match outcome {
                RunOutcome::Exited { elapsed, exit_code } => {
                    match on_exit_action(self.config, exit_code) {
                        ExitAction::Shutdown => RunOutcome::Stop(exit_code),
                        ExitAction::Restart => RunOutcome::Restart {
                            elapsed,
                            automatic: true,
                            exit_code,
                        },
                        ExitAction::Pause if self.pausable() => RunOutcome::Pause,
                        ExitAction::Pause => RunOutcome::Restart {
                            elapsed,
                            automatic: true,
                            exit_code,
                        },
                    }
                }
                other => other,
            };
            match outcome {
                RunOutcome::Stop(code) => {
                    logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
                    return Ok(code);
                }
                RunOutcome::Pause => {
                    failed_invocations = 0;
                    match self.wait_while_paused() {
                        PausedOutcome::Resume => {
                            jvm_id = jvm_id.saturating_add(1);
                            resuming = true;
                        }
                        PausedOutcome::Stop => {
                            logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
                            return Ok(0);
                        }
                    }
                }
                RunOutcome::Restart {
                    elapsed,
                    automatic,
                    exit_code,
                } => {
                    resuming = false;
                    if self.config.get_bool("wrapper.disable_restarts", false)
                        || (automatic
                            && self
                                .config
                                .get_bool("wrapper.disable_restarts.automatic", false))
                    {
                        logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            if automatic {
                                "Automatic JVM restarts are disabled; stopping."
                            } else {
                                "JVM restarts are disabled; stopping."
                            },
                        )?;
                        logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
                        return Ok(exit_code);
                    }
                    let successful_seconds = self
                        .config
                        .get_u64("wrapper.successful_invocation_time", 300);
                    if elapsed >= Duration::from_secs(successful_seconds) {
                        failed_invocations = 0;
                    } else {
                        failed_invocations = failed_invocations.saturating_add(1);
                    }
                    let maximum = self
                        .config
                        .get_u64("wrapper.max_failed_invocations", 5)
                        .max(1);
                    if failed_invocations >= maximum {
                        logger.write(
                            LogLevel::Fatal,
                            LogSource::Wrapper,
                            &format!(
                                "{failed_invocations} consecutive JVM launches ended within {successful_seconds} seconds; giving up."
                            ),
                        )?;
                        logger.write(
                            LogLevel::Fatal,
                            LogSource::Wrapper,
                            "Check the application configuration and the log above.",
                        )?;
                        logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
                        return Ok(exit_code);
                    }
                    jvm_id = jvm_id.saturating_add(1);
                    let delay =
                        Duration::from_secs(self.config.get_u64("wrapper.restart.delay", 5));
                    if self.wait_for_restart(delay) {
                        logger.write(LogLevel::Status, LogSource::Wrapper, STOPPED_MARKER)?;
                        return Ok(0);
                    }
                }
                RunOutcome::Exited { .. } => unreachable!("exit actions are resolved above"),
            }
        }
    }

    fn run_once(
        &self,
        jvm_id: u32,
        resuming: bool,
        logger: &mut LogWriter,
        filters: &Filters,
    ) -> Result<RunOutcome> {
        let launched_at = Instant::now();
        let configured_port = optional_port(self.config, "wrapper.port")?;
        let port_min = port(self.config, "wrapper.port.min", 32_000)?;
        let port_max = port(self.config, "wrapper.port.max", 32_999)?;
        let listener = BackendListener::bind(configured_port, port_min, port_max)?;
        let key = protocol::generate_key()?;
        let launch = BackendLaunch {
            key: key.clone(),
            port: listener.port(),
            jvm_port: optional_port(self.config, "wrapper.jvm.port")?,
            jvm_port_min: port(self.config, "wrapper.jvm.port.min", 31_000)?,
            jvm_port_max: port(self.config, "wrapper.jvm.port.max", 31_999)?,
            jvm_id,
            service: self.service,
        };
        let command = JvmCommand::build(self.config, &launch, self.application_args)?;
        if let Some(note) = &command.launch_note {
            logger.write(LogLevel::Info, LogSource::Wrapper, note)?;
        }
        let command_log_level = java_command_log_level(self.config);
        if command_log_level != LogLevel::None {
            logger.write(
                command_log_level,
                LogSource::Wrapper,
                &format!("Command: {}", command.redacted_command_line()),
            )?;
        }
        logger.write(
            LogLevel::Status,
            LogSource::Wrapper,
            &format!("Launching JVM #{jvm_id}"),
        )?;
        let mut child = match spawn_child(&command) {
            Ok(child) => child,
            Err(error) => {
                logger.write(
                    LogLevel::Error,
                    LogSource::Wrapper,
                    &format!("Unable to launch the JVM: {error}"),
                )?;
                return Ok(RunOutcome::Restart {
                    elapsed: launched_at.elapsed(),
                    automatic: true,
                    exit_code: 1,
                });
            }
        };
        let job = if self.config.get_bool("jss.java.job_object", true) {
            match JobObject::kill_on_close().and_then(|job| job.assign(&child).map(|()| job)) {
                Ok(job) => Some(job),
                Err(error) => {
                    logger.write(
                        LogLevel::Warn,
                        LogSource::Wrapper,
                        &format!(
                            "Unable to place the JVM in a job object; child processes may outlive the wrapper: {error}"
                        ),
                    )?;
                    None
                }
            }
        } else {
            None
        };
        let pid = child.id();
        let java_pid_file =
            match OwnedPidFile::create(self.config, "wrapper.java.pidfile", pid, false) {
                Ok(file) => file,
                Err(error) => {
                    terminate_child(&mut child)?;
                    return Err(error);
                }
            };
        let output = capture_output(&mut child);
        let startup_seconds = self.config.get_u64("wrapper.startup.timeout", 30);
        let ping_interval =
            Duration::from_secs(self.config.get_u64("wrapper.ping.interval", 5).max(1));
        let ping_timeout = Duration::from_secs(self.config.get_u64("wrapper.ping.timeout", 30));
        let now = Instant::now();

        let mut run = JvmRun {
            supervisor: self,
            jvm_id,
            resuming,
            logger,
            filters,
            command,
            child,
            pid,
            _job: job,
            _java_pid_file: java_pid_file,
            output,
            output_open: true,
            listener,
            key,
            connection: None,
            ever_connected: false,
            launched_at,
            started: false,
            startup_seconds,
            startup_deadline: (startup_seconds > 0)
                .then(|| now + Duration::from_secs(startup_seconds)),
            ping_interval,
            ping_timeout,
            next_ping: now + ping_interval,
            last_ping_response: now,
            paused_in_place: false,
            heap_dump_task: None,
        };
        run.event_loop()
    }

    fn wait_for_restart(&self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline {
            match self.controls.recv_timeout(Duration::from_millis(100)) {
                Ok(Control::Stop | Control::ConsoleInterrupt)
                | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return true;
                }
                Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
        }
        false
    }

    fn wait_while_paused(&self) -> PausedOutcome {
        loop {
            match self.controls.recv() {
                Ok(Control::Resume) => return PausedOutcome::Resume,
                Ok(Control::Stop | Control::ConsoleInterrupt) | Err(_) => {
                    return PausedOutcome::Stop;
                }
                Ok(
                    Control::Pause
                    | Control::ConsoleBreak
                    | Control::ThreadDump
                    | Control::HeapDump
                    | Control::User(_),
                ) => {}
            }
        }
    }

    fn pausable(&self) -> bool {
        self.config.get_bool(
            "wrapper.pausable",
            self.config.get_bool("wrapper.ntservice.pausable", false),
        )
    }

    fn pausable_stops_jvm(&self) -> bool {
        self.config.get_bool(
            "wrapper.pausable.stop_jvm",
            self.config
                .get_bool("wrapper.ntservice.pausable.stop_jvm", true),
        )
    }
}

/// State of one JVM invocation. All lifecycle decisions happen on the
/// supervisor thread; reader threads only feed channels.
struct JvmRun<'s, 'c> {
    supervisor: &'s Supervisor<'c>,
    jvm_id: u32,
    resuming: bool,
    logger: &'s mut LogWriter,
    filters: &'s Filters,
    command: JvmCommand,
    child: Child,
    pid: u32,
    _job: Option<JobObject>,
    _java_pid_file: Option<OwnedPidFile>,
    output: Receiver<OutputLine>,
    output_open: bool,
    listener: BackendListener,
    key: String,
    connection: Option<Connection>,
    ever_connected: bool,
    launched_at: Instant,
    started: bool,
    startup_seconds: u64,
    startup_deadline: Option<Instant>,
    ping_interval: Duration,
    ping_timeout: Duration,
    next_ping: Instant,
    last_ping_response: Instant,
    paused_in_place: bool,
    heap_dump_task: Option<heap_dump::Task>,
}

impl JvmRun<'_, '_> {
    fn config(&self) -> &Config {
        self.supervisor.config
    }

    fn events(&self) -> &EventPublisher {
        &self.supervisor.events
    }

    fn event_loop(&mut self) -> Result<RunOutcome> {
        let ticker = tick(TICK_INTERVAL);
        let closed = never::<OutputLine>();
        let no_protocol = never::<ReceiveEvent>();
        let controls = self.supervisor.controls.clone();
        loop {
            let output = if self.output_open {
                self.output.clone()
            } else {
                closed.clone()
            };
            let protocol_events = self.connection.as_ref().map_or_else(
                || no_protocol.clone(),
                |connection| connection.events().clone(),
            );
            let outcome = select! {
                recv(output) -> line => match line {
                    Ok(line) => self.handle_output_batch(line)?,
                    Err(_) => {
                        self.output_open = false;
                        None
                    }
                },
                recv(controls) -> control => match control {
                    Ok(control) => self.handle_control(control)?,
                    Err(_) => Some(self.stop_with(0)?),
                },
                recv(protocol_events) -> event => match event {
                    Ok(event) => self.handle_protocol(event)?,
                    Err(_) => {
                        self.mark_disconnected()?;
                        None
                    }
                },
                recv(ticker) -> _ => self.tick()?,
            };
            if let Some(outcome) = outcome {
                return Ok(outcome);
            }
        }
    }

    fn handle_output_batch(&mut self, first: OutputLine) -> Result<Option<RunOutcome>> {
        if let Some(outcome) = self.handle_output_line(first)? {
            return Ok(Some(outcome));
        }
        let batch: Vec<OutputLine> = self.output.try_iter().take(OUTPUT_BATCH).collect();
        for line in batch {
            if let Some(outcome) = self.handle_output_line(line)? {
                return Ok(Some(outcome));
            }
        }
        Ok(None)
    }

    fn handle_output_line(&mut self, line: OutputLine) -> Result<Option<RunOutcome>> {
        let matches = self.record_output(&line)?;
        self.apply_filter_matches(matches)
    }

    fn record_output(&mut self, line: &OutputLine) -> Result<Vec<FilterMatch>> {
        self.logger
            .write_bytes(LogLevel::Info, LogSource::Jvm(self.jvm_id), &line.bytes)?;
        let matches = self.filters.inspect_bytes(&line.bytes);
        for matched in &matches {
            self.events().publish(EventKind::FilterMatched {
                index: matched.index,
            });
        }
        Ok(matches)
    }

    fn apply_filter_matches(&mut self, matches: Vec<FilterMatch>) -> Result<Option<RunOutcome>> {
        for matched in matches {
            for action in matched.actions {
                match action {
                    FilterAction::None => {}
                    FilterAction::Debug => self.logger.write(
                        LogLevel::Debug,
                        LogSource::Wrapper,
                        &filter_message(&matched.message, "Filter action: debug."),
                    )?,
                    FilterAction::Dump => {
                        self.logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            &filter_message(
                                &matched.message,
                                "Filter action: requesting a thread dump.",
                            ),
                        )?;
                        self.request_thread_dump()?;
                    }
                    FilterAction::Gc => {
                        self.logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            &filter_message(
                                &matched.message,
                                "Filter action: requesting garbage collection.",
                            ),
                        )?;
                        self.send_if_connected(protocol::GC, "gc")?;
                    }
                    FilterAction::Restart => {
                        self.logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            &filter_message(&matched.message, "Filter action: restarting the JVM."),
                        )?;
                        self.events().publish(EventKind::RestartRequested {
                            reason: format!("wrapper.filter.action.{}=RESTART", matched.index),
                        });
                        self.shutdown_and_drain(0)?;
                        return Ok(Some(RunOutcome::Restart {
                            elapsed: self.launched_at.elapsed(),
                            automatic: false,
                            exit_code: 0,
                        }));
                    }
                    FilterAction::Shutdown => {
                        self.logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            &filter_message(&matched.message, "Filter action: stopping."),
                        )?;
                        return Ok(Some(self.stop_with(0)?));
                    }
                    FilterAction::Pause if self.supervisor.pausable() && !self.paused_in_place => {
                        self.logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            &filter_message(&matched.message, "Filter action: pausing."),
                        )?;
                        if let Some(outcome) = self.pause()? {
                            return Ok(Some(outcome));
                        }
                    }
                    FilterAction::Resume if self.paused_in_place => {
                        self.logger.write(
                            LogLevel::Status,
                            LogSource::Wrapper,
                            &filter_message(&matched.message, "Filter action: resuming."),
                        )?;
                        self.resume()?;
                    }
                    FilterAction::Pause | FilterAction::Resume => {}
                }
            }
        }
        Ok(None)
    }

    fn handle_control(&mut self, control: Control) -> Result<Option<RunOutcome>> {
        match control {
            Control::Stop => Ok(Some(self.stop_with(0)?)),
            Control::ConsoleInterrupt => {
                self.logger.write(
                    LogLevel::Status,
                    LogSource::Wrapper,
                    "Console interrupt received; stopping.",
                )?;
                Ok(Some(self.stop_with(0)?))
            }
            Control::ConsoleBreak => {
                self.logger.write(
                    LogLevel::Status,
                    LogSource::Wrapper,
                    "Console break received; requesting a thread dump.",
                )?;
                self.request_thread_dump()?;
                Ok(None)
            }
            Control::Pause => {
                if self.supervisor.pausable() && !self.paused_in_place {
                    return self.pause();
                }
                Ok(None)
            }
            Control::Resume => {
                if self.paused_in_place {
                    self.resume()?;
                }
                Ok(None)
            }
            Control::ThreadDump => {
                self.logger.write(
                    LogLevel::Status,
                    LogSource::Wrapper,
                    "Requesting a thread dump from the JVM.",
                )?;
                self.request_thread_dump()?;
                Ok(None)
            }
            Control::HeapDump => {
                self.start_heap_dump()?;
                Ok(None)
            }
            Control::User(code) => {
                let message = code.to_string();
                self.send_if_connected(protocol::SERVICE_CONTROL, &message)?;
                Ok(None)
            }
        }
    }

    fn handle_protocol(&mut self, event: ReceiveEvent) -> Result<Option<RunOutcome>> {
        let packet = match event {
            ReceiveEvent::Packet(packet) => packet,
            ReceiveEvent::Disconnected => {
                self.mark_disconnected()?;
                return Ok(None);
            }
        };
        self.last_ping_response = Instant::now();
        match packet.code {
            STARTED => {
                self.started = true;
                self.startup_deadline = None;
                self.events().publish(EventKind::JvmStarted {
                    id: self.jvm_id,
                    pid: self.pid,
                });
                if self.resuming {
                    self.events().publish(EventKind::ServiceResumed);
                }
            }
            START_PENDING => {
                if self.startup_seconds > 0 {
                    let wait_hint = packet
                        .message_lossy()
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(self.startup_seconds.saturating_mul(1_000));
                    self.startup_deadline =
                        Some(Instant::now() + Duration::from_millis(wait_hint.max(1)));
                }
            }
            PING => {}
            RESTART => {
                self.events().publish(EventKind::RestartRequested {
                    reason: "requested by the application".into(),
                });
                self.shutdown_and_drain(0)?;
                return Ok(Some(RunOutcome::Restart {
                    elapsed: self.launched_at.elapsed(),
                    automatic: false,
                    exit_code: 0,
                }));
            }
            STOP => {
                let code = packet.message_lossy().trim().parse().unwrap_or(0);
                self.shutdown_and_drain(code)?;
                let observed = self.child.try_wait()?.and_then(|status| status.code());
                self.log_jvm_exit(observed)?;
                return Ok(Some(RunOutcome::Exited {
                    elapsed: self.launched_at.elapsed(),
                    exit_code: code,
                }));
            }
            STOP_PENDING | STOPPED => {}
            code if (LOG_BASE + 1..=LOG_BASE + 8).contains(&code) => {
                if let Some(level) = LogLevel::from_protocol_code(code) {
                    self.logger
                        .write_bytes(level, LogSource::Jvm(self.jvm_id), &packet.message)?;
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn tick(&mut self) -> Result<Option<RunOutcome>> {
        self.poll_heap_dump()?;

        if let Some(status) = self.child.try_wait()? {
            self.drain_remaining_output()?;
            let exit_code = status.code();
            self.log_jvm_exit(exit_code)?;
            return Ok(Some(RunOutcome::Exited {
                elapsed: self.launched_at.elapsed(),
                exit_code: exit_code.unwrap_or(1),
            }));
        }

        if self.connection.is_none() {
            let handshake_timeout =
                Duration::from_secs(self.startup_seconds).max(MIN_HANDSHAKE_TIMEOUT);
            if let Some(mut connection) = self
                .listener
                .poll_authentication(&self.key, handshake_timeout)?
            {
                connection.send(LOW_LOG_LEVEL, &low_log_level(self.config()).to_string())?;
                let logfile_path = self
                    .logger
                    .path()
                    .canonicalize()
                    .unwrap_or_else(|_| self.logger.path().to_path_buf());
                connection.send(LOGFILE, &logfile_path.to_string_lossy())?;
                connection.send(PROPERTIES, &self.config().protocol_properties())?;
                connection.send(START, "start")?;
                if self.ever_connected {
                    self.logger.write(
                        LogLevel::Debug,
                        LogSource::Protocol,
                        "JVM control channel reconnected.",
                    )?;
                }
                self.ever_connected = true;
                self.last_ping_response = Instant::now();
                self.connection = Some(connection);
                self.events().publish(EventKind::ProtocolAuthenticated);
            }
        }

        let now = Instant::now();
        if !self.started
            && self
                .startup_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            let message = if self.ever_connected {
                format!(
                    "The application did not report started within {} seconds; restarting.",
                    self.startup_seconds
                )
            } else {
                format!(
                    "The JVM did not connect to the control channel within {} seconds; restarting.",
                    self.startup_seconds
                )
            };
            self.logger
                .write(LogLevel::Error, LogSource::Wrapper, &message)?;
            self.shutdown_and_drain(1)?;
            return Ok(Some(RunOutcome::Restart {
                elapsed: self.launched_at.elapsed(),
                automatic: true,
                exit_code: 1,
            }));
        }
        if self.connection.is_some() && now >= self.next_ping {
            let message = if self.started { "ping" } else { "silent" };
            self.send_if_connected(PING, message)?;
            self.next_ping = now + self.ping_interval;
        }
        if self.ping_timeout > Duration::ZERO && self.ever_connected {
            let extra = self
                .heap_dump_task
                .as_ref()
                .map_or(Duration::ZERO, heap_dump::Task::timeout);
            let allowed = self.ping_timeout + self.ping_interval + extra;
            if now.duration_since(self.last_ping_response) > allowed {
                self.logger.write(
                    LogLevel::Error,
                    LogSource::Wrapper,
                    &format!(
                        "No ping response from the JVM for {} seconds; restarting.",
                        allowed.as_secs()
                    ),
                )?;
                self.shutdown_and_drain(1)?;
                return Ok(Some(RunOutcome::Restart {
                    elapsed: self.launched_at.elapsed(),
                    automatic: true,
                    exit_code: 1,
                }));
            }
        }
        Ok(None)
    }

    /// Records the JVM exit in the log and publishes the lifecycle event.
    fn log_jvm_exit(&mut self, exit_code: Option<i32>) -> Result<()> {
        self.events().publish(EventKind::JvmStopped {
            id: self.jvm_id,
            exit_code,
        });
        self.logger.write(
            LogLevel::Status,
            LogSource::Wrapper,
            &format!(
                "JVM #{} exited with code {}",
                self.jvm_id,
                exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
            ),
        )
    }

    fn pause(&mut self) -> Result<Option<RunOutcome>> {
        if self.supervisor.pausable_stops_jvm() {
            self.shutdown_and_drain(0)?;
            self.events().publish(EventKind::ServicePaused);
            return Ok(Some(RunOutcome::Pause));
        }
        if self.send_if_connected(protocol::PAUSE, "0")? {
            self.paused_in_place = true;
            self.events().publish(EventKind::ServicePaused);
        }
        Ok(None)
    }

    fn resume(&mut self) -> Result<()> {
        if self.send_if_connected(protocol::RESUME, "0")? {
            self.paused_in_place = false;
            self.events().publish(EventKind::ServiceResumed);
        }
        Ok(())
    }

    fn stop_with(&mut self, exit_code: i32) -> Result<RunOutcome> {
        self.shutdown_and_drain(exit_code)?;
        Ok(RunOutcome::Stop(exit_code))
    }

    fn send_if_connected(&mut self, code: u8, message: &str) -> Result<bool> {
        let Some(connection) = self.connection.as_mut() else {
            return Ok(false);
        };
        if let Err(error) = connection.send(code, message) {
            self.logger.write(
                LogLevel::Error,
                LogSource::Protocol,
                &format!("Control channel write failed: {error}"),
            )?;
            self.mark_disconnected()?;
            return Ok(false);
        }
        Ok(true)
    }

    fn mark_disconnected(&mut self) -> Result<()> {
        if self.connection.take().is_some() {
            self.logger.write(
                LogLevel::Debug,
                LogSource::Protocol,
                "JVM control channel disconnected; waiting for the JVM to exit or reconnect.",
            )?;
            self.events().publish(EventKind::ProtocolDisconnected);
        }
        Ok(())
    }

    fn request_thread_dump(&mut self) -> Result<bool> {
        let method = thread_dump::method(self.config(), &self.command.arguments);
        let result = match method {
            Ok(ThreadDumpMethod::ConsoleBreak) => {
                self.events().publish(EventKind::ThreadDumpStarted {
                    jvm_id: self.jvm_id,
                    pid: self.pid,
                    method: "CTRL_BREAK".into(),
                });
                windows_process::request_thread_dump(self.pid).map(|()| "CTRL_BREAK")
            }
            Ok(ThreadDumpMethod::Jcmd) => {
                self.logger.write(
                    LogLevel::Status,
                    LogSource::Wrapper,
                    "Requesting a thread dump with jcmd.",
                )?;
                self.events().publish(EventKind::ThreadDumpStarted {
                    jvm_id: self.jvm_id,
                    pid: self.pid,
                    method: "JCMD".into(),
                });
                let timeout =
                    Duration::from_secs(self.config().get_u64("jss.threaddump.timeout", 30).max(1));
                let jvm_id = self.jvm_id;
                let logger = &mut *self.logger;
                thread_dump::capture_with_jcmd(
                    &self.command.program,
                    &self.command.arguments,
                    &self.command.environment,
                    &self.command.working_directory,
                    self.pid,
                    timeout,
                    |line| logger.write_bytes(LogLevel::Info, LogSource::Jvm(jvm_id), line),
                )
                .map(|()| "JCMD")
            }
            Err(error) => Err(error),
        };

        match result {
            Ok(method) => {
                self.events().publish(EventKind::ThreadDumpCompleted {
                    jvm_id: self.jvm_id,
                    method: method.into(),
                });
                Ok(true)
            }
            Err(error) => {
                let message = format!("Unable to request thread dump: {error}");
                self.logger
                    .write(LogLevel::Error, LogSource::Wrapper, &message)?;
                self.events().publish(EventKind::Warning { message });
                Ok(false)
            }
        }
    }

    fn start_heap_dump(&mut self) -> Result<()> {
        if !self.started {
            self.logger.write(
                LogLevel::Warn,
                LogSource::Wrapper,
                "Heap dump request ignored because the application has not reported started.",
            )?;
            return Ok(());
        }
        if let Some(task) = &self.heap_dump_task {
            self.logger.write(
                LogLevel::Warn,
                LogSource::Wrapper,
                &format!(
                    "A heap dump is already in progress: {}",
                    task.path().display()
                ),
            )?;
            return Ok(());
        }

        match heap_dump::start(
            self.config(),
            &self.command,
            self.logger.path(),
            self.pid,
            self.jvm_id,
        ) {
            Ok(task) => {
                let path = task.path().display().to_string();
                self.logger.write(
                    LogLevel::Status,
                    LogSource::Wrapper,
                    &format!("Heap dump requested: {path}"),
                )?;
                self.events().publish(EventKind::HeapDumpStarted {
                    jvm_id: self.jvm_id,
                    pid: self.pid,
                    path,
                });
                self.heap_dump_task = Some(task);
            }
            Err(error) => {
                let message = format!("Unable to request heap dump: {error}");
                self.logger
                    .write(LogLevel::Error, LogSource::Wrapper, &message)?;
                self.events().publish(EventKind::Warning { message });
            }
        }
        Ok(())
    }

    fn poll_heap_dump(&mut self) -> Result<()> {
        let Some(task) = self.heap_dump_task.as_ref() else {
            return Ok(());
        };
        let heap_dump::Poll::Complete(completion) = task.poll() else {
            return Ok(());
        };
        self.heap_dump_task = None;
        match completion {
            heap_dump::Completion::Created { path, bytes } => {
                let path = path.display().to_string();
                self.logger.write(
                    LogLevel::Status,
                    LogSource::Wrapper,
                    &format!("Heap dump completed: {path} ({bytes} bytes)"),
                )?;
                self.events().publish(EventKind::HeapDumpCompleted {
                    jvm_id: self.jvm_id,
                    path,
                    bytes,
                });
            }
            heap_dump::Completion::Failed { path, message } => {
                let message = format!("Heap dump failed for {}: {message}", path.display());
                self.logger
                    .write(LogLevel::Error, LogSource::Wrapper, &message)?;
                self.events().publish(EventKind::Warning { message });
            }
        }
        Ok(())
    }

    /// Asks a connected JVM to stop with `exit_code`, waits up to
    /// `wrapper.shutdown.timeout` while logging its output, and terminates it
    /// forcibly when it does not exit. A JVM without a control connection is
    /// terminated immediately because it cannot receive the request.
    fn shutdown_and_drain(&mut self, exit_code: i32) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return self.drain_remaining_output();
        }
        if !self.send_if_connected(STOP, &exit_code.to_string())? {
            terminate_child(&mut self.child)?;
            return self.drain_remaining_output();
        }
        let timeout = shutdown_timeout(self.config());
        if self.wait_for_exit(timeout)? {
            return Ok(());
        }

        self.logger.write(
            LogLevel::Error,
            LogSource::Wrapper,
            &format!("The JVM did not stop within {} seconds.", timeout.as_secs()),
        )?;
        if self
            .config()
            .get_bool("wrapper.request_thread_dump_on_failed_jvm_exit", false)
        {
            self.logger.write(
                LogLevel::Status,
                LogSource::Wrapper,
                "Requesting a thread dump before terminating the JVM.",
            )?;
            if self.request_thread_dump()? {
                let dump_delay = Duration::from_secs(
                    self.config()
                        .get_u64("wrapper.request_thread_dump_on_failed_jvm_exit.delay", 5)
                        .max(1),
                );
                if self.wait_for_exit(dump_delay)? {
                    return Ok(());
                }
            }
        }
        self.logger.write(
            LogLevel::Error,
            LogSource::Wrapper,
            "Terminating the JVM forcibly.",
        )?;
        terminate_child(&mut self.child)?;
        self.drain_remaining_output()
    }

    /// Logs output while waiting for the child to exit; returns `true` when
    /// it exited within `timeout`.
    fn wait_for_exit(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.child.try_wait()?.is_some() {
                self.drain_remaining_output()?;
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            match self.output.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let _ = self.record_output(&line)?;
                    let batch: Vec<OutputLine> =
                        self.output.try_iter().take(OUTPUT_BATCH).collect();
                    for line in batch {
                        let _ = self.record_output(&line)?;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn drain_remaining_output(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.output.recv_timeout(Duration::from_millis(25)) {
                Ok(line) => {
                    let _ = self.record_output(&line)?;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(())
    }
}

pub fn console_control_channel() -> Result<(Sender<Control>, Receiver<Control>)> {
    let (sender, receiver) = crossbeam_channel::bounded(16);
    let signals = windows_process::console_signal_channel()?;
    let handler_sender = sender.clone();
    thread::spawn(move || {
        while let Ok(signal) = signals.recv() {
            let control = match signal {
                windows_process::ConsoleSignal::Interrupt => Control::ConsoleInterrupt,
                windows_process::ConsoleSignal::Break => Control::ConsoleBreak,
            };
            let _ = handler_sender.try_send(control);
        }
    });
    Ok((sender, receiver))
}

fn spawn_child(command: &JvmCommand) -> Result<Child> {
    let mut process = Command::new(&command.program);
    windows_process::configure_child(&mut process);
    process
        .args(&command.arguments)
        .current_dir(&command.working_directory)
        .envs(command.environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(process.spawn()?)
}

fn filter_message(configured: &str, action: &str) -> String {
    let configured = configured.trim();
    if configured.is_empty() {
        action.into()
    } else {
        format!("{configured}  {action}")
    }
}

fn capture_output(child: &mut Child) -> Receiver<OutputLine> {
    let (sender, receiver) = crossbeam_channel::bounded(1024);
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, sender);
    }
    receiver
}

/// Reads lines from a child pipe. Lines longer than `MAX_OUTPUT_LINE` are
/// split so a missing newline can never grow memory without bound.
fn spawn_reader(reader: impl Read + Send + 'static, sender: Sender<OutputLine>) {
    thread::spawn(move || {
        let mut reader = BufReader::with_capacity(64 * 1024, reader);
        let mut line = Vec::new();
        loop {
            let buffer = match reader.fill_buf() {
                Ok([]) => break,
                Ok(buffer) => buffer,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(buffer.len(), |index| index + 1);
            let take = take.min(MAX_OUTPUT_LINE.saturating_sub(line.len()).max(1));
            line.extend_from_slice(&buffer[..take]);
            reader.consume(take);
            let complete = line.last() == Some(&b'\n');
            if complete || line.len() >= MAX_OUTPUT_LINE {
                if complete {
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                }
                if sender
                    .send(OutputLine {
                        bytes: std::mem::take(&mut line),
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
        if !line.is_empty() {
            let _ = sender.send(OutputLine { bytes: line });
        }
    });
}

fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    let _ = child.wait()?;
    Ok(())
}

fn shutdown_timeout(config: &Config) -> Duration {
    Duration::from_secs(config.get_u64("wrapper.shutdown.timeout", 30))
}

fn startup_delay(config: &Config, service: bool) -> Duration {
    let default = config.get_u64("wrapper.startup.delay", 0);
    let property = if service {
        "wrapper.startup.delay.service"
    } else {
        "wrapper.startup.delay.console"
    };
    Duration::from_secs(config.get_u64(property, default))
}

fn on_exit_action(config: &Config, exit_code: i32) -> ExitAction {
    let specific = format!("wrapper.on_exit.{exit_code}");
    let configured = config
        .get(&specific)
        .unwrap_or_else(|| config.get_or("wrapper.on_exit.default", "SHUTDOWN"));
    match configured.trim().to_ascii_uppercase().as_str() {
        "RESTART" => ExitAction::Restart,
        "PAUSE" => ExitAction::Pause,
        _ => ExitAction::Shutdown,
    }
}

fn optional_port(config: &Config, name: &str) -> Result<Option<u16>> {
    match config.get(name) {
        None => Ok(None),
        Some(value) => {
            let parsed = value
                .trim()
                .parse::<u16>()
                .map_err(|_| Error::Config(format!("{name} is not a valid port: {value}")))?;
            Ok((parsed != 0).then_some(parsed))
        }
    }
}

fn port(config: &Config, name: &str, default: u16) -> Result<u16> {
    optional_port(config, name)?.map_or(Ok(default), Ok)
}

#[cfg(test)]
mod tests {
    use super::{ExitAction, OwnedPidFile, on_exit_action, startup_delay};
    use crate::config::Config;
    use std::fs;

    fn test_directory(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "jss-{label}-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&directory).expect("create test directory");
        directory
    }

    #[test]
    fn exit_actions_default_to_shutdown_and_allow_code_specific_overrides() {
        let directory = test_directory("on-exit");
        let default_path = directory.join("default.conf");
        fs::write(&default_path, "").expect("write default config");
        let default = Config::load(&default_path, &directory, &[]).expect("load default config");
        assert_eq!(on_exit_action(&default, 1), ExitAction::Shutdown);

        let configured_path = directory.join("configured.conf");
        fs::write(
            &configured_path,
            "wrapper.on_exit.default=RESTART\nwrapper.on_exit.0=SHUTDOWN\nwrapper.on_exit.75=PAUSE\n",
        )
        .expect("write configured exit actions");
        let configured =
            Config::load(&configured_path, &directory, &[]).expect("load configured exit actions");
        assert_eq!(on_exit_action(&configured, 1), ExitAction::Restart);
        assert_eq!(on_exit_action(&configured, 0), ExitAction::Shutdown);
        assert_eq!(on_exit_action(&configured, 75), ExitAction::Pause);
        assert!(configured.warnings().is_empty());

        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn startup_delay_uses_the_mode_specific_override() {
        let directory = test_directory("startup-delay");
        let config_path = directory.join("wrapper.conf");
        fs::write(
            &config_path,
            "wrapper.startup.delay=7\n\
             wrapper.startup.delay.console=0\n\
             wrapper.startup.delay.service=19\n\
             wrapper.console.title.windows=Example\n\
             wrapper.request_thread_dump_on_failed_jvm_exit=TRUE\n\
             wrapper.request_thread_dump_on_failed_jvm_exit.delay=2\n\
             wrapper.java.idfile=java.id\n",
        )
        .expect("write config");
        let config = Config::load(&config_path, &directory, &[]).expect("load config");
        assert_eq!(startup_delay(&config, false), std::time::Duration::ZERO);
        assert_eq!(
            startup_delay(&config, true),
            std::time::Duration::from_secs(19)
        );
        assert!(config.warnings().is_empty());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn pid_files_use_crlf_and_are_removed_only_while_still_owned() {
        let directory = test_directory("pidfile");
        let config_path = directory.join("wrapper.conf");
        fs::write(
            &config_path,
            "wrapper.pidfile=wrapper.pid\n\
             wrapper.java.pidfile=java.pid\n\
             wrapper.java.idfile=java.id\n",
        )
        .expect("write config");
        let config = Config::load(&config_path, &directory, &[]).expect("load config");

        {
            let _owned = OwnedPidFile::create(&config, "wrapper.pidfile", 12_345, false)
                .expect("create wrapper pid file");
            assert_eq!(
                fs::read(directory.join("wrapper.pid")).expect("read wrapper pid file"),
                b"12345\r\n"
            );
        }
        assert!(!directory.join("wrapper.pid").exists());
        fs::write(directory.join("wrapper.pid"), b"stale\r\n")
            .expect("write stale wrapper pid file");
        assert!(
            OwnedPidFile::create(&config, "wrapper.pidfile", 12_345, true).is_err(),
            "strict mode must reject an existing pid file"
        );
        assert_eq!(
            fs::read(directory.join("wrapper.pid")).expect("preserve strict pid file"),
            b"stale\r\n"
        );

        {
            let _owned = OwnedPidFile::create(&config, "wrapper.java.pidfile", 54_321, false)
                .expect("create Java pid file");
            fs::write(directory.join("java.pid"), b"replaced externally\r\n")
                .expect("replace Java pid file");
        }
        assert_eq!(
            fs::read(directory.join("java.pid")).expect("preserve replaced pid file"),
            b"replaced externally\r\n"
        );

        {
            let mut owned = OwnedPidFile::create(&config, "wrapper.java.idfile", 1, false)
                .expect("create Java id file")
                .expect("configured Java id file");
            owned.update(2).expect("update Java id file");
            assert_eq!(
                fs::read(directory.join("java.id")).expect("read Java id file"),
                b"2\r\n"
            );
        }
        assert!(!directory.join("java.id").exists());
        assert!(config.warnings().is_empty());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn failed_process_creation_uses_the_restart_throttle() {
        let directory = test_directory("spawn-failure");
        let config_path = directory.join("wrapper.conf");
        fs::write(
            &config_path,
            "wrapper.java.command=definitely-missing-jss-java-command\n\
             wrapper.java.mainclass=example.Main\n\
             wrapper.restart.delay=0\n\
             wrapper.max_failed_invocations=2\n\
             wrapper.successful_invocation_time=300\n\
             wrapper.logfile=wrapper.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.console.loglevel=NONE\n",
        )
        .expect("write config");
        let config = Config::load(&config_path, &directory, &[]).expect("load config");
        let (_control_sender, controls) = crossbeam_channel::bounded(4);
        let (events, _event_receiver) = crate::telemetry::EventPublisher::bounded(16);
        assert_eq!(
            super::Supervisor::new(&config, &[], false, controls, events)
                .run()
                .expect("run supervisor"),
            1
        );
        let log = fs::read_to_string(directory.join("wrapper.log")).expect("read wrapper log");
        assert_eq!(log.matches("Unable to launch the JVM").count(), 2);
        assert!(log.contains("2 consecutive JVM launches ended within"));
        assert!(log.contains(super::STOPPED_MARKER));
        for marker in crate::FOREIGN_NAME_MARKERS {
            assert!(!log.contains(marker), "log must not contain {marker}");
        }
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn long_output_lines_are_split_instead_of_growing_without_bound() {
        let (sender, receiver) = crossbeam_channel::bounded(64);
        let payload = vec![b'x'; super::MAX_OUTPUT_LINE * 2 + 10];
        super::spawn_reader(std::io::Cursor::new(payload), sender);
        let mut lengths = Vec::new();
        while let Ok(line) = receiver.recv_timeout(std::time::Duration::from_secs(2)) {
            lengths.push(line.bytes.len());
        }
        assert_eq!(
            lengths,
            [super::MAX_OUTPUT_LINE, super::MAX_OUTPUT_LINE, 10]
        );
    }
}
