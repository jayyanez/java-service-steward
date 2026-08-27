// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Integration tests that build the Java bridge and run real JVMs through the
//! supervisor. They need `javac`, `jar`, `jcmd` and `jlink` on `PATH` and skip
//! otherwise, unless `JSS_REQUIRE_JAVA_TESTS=1` turns a skip into a failure.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use java_service_steward::config::Config;
use java_service_steward::supervisor::{Control, Supervisor};
use java_service_steward::telemetry::{EventKind, EventPublisher};

// Launcher names as another product would configure them: absent from the
// classpath and ending in the name of a bundled launcher.
const SIMPLE_ALIAS: &str = "legacy.launchers.LegacySimpleApp";
const START_STOP_ALIAS: &str = "legacy.launchers.LegacyStartStopApp";
const JAR_ALIAS: &str = "legacy.launchers.LegacyJarApp";
const STOPPED_MARKER: &str = "<-- Wrapper Stopped";

struct Jdk {
    javac: PathBuf,
    java: PathBuf,
    jar: PathBuf,
    jcmd: PathBuf,
    jlink: PathBuf,
}

/// A prepared run directory with the bridge JAR and the synthetic classes.
struct Fixture {
    jdk: Jdk,
    directory: PathBuf,
}

impl Fixture {
    fn prepare(name: &str) -> Option<Self> {
        let jdk = find_jdk()?;
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let directory = manifest.join("target").join(name);
        if directory.exists() {
            fs::remove_dir_all(&directory).expect("clear generated fixture");
        }
        let classes = directory.join("classes");
        let bridge_classes = directory.join("bridge-classes");
        fs::create_dir_all(&classes).expect("create application classes directory");
        fs::create_dir_all(&bridge_classes).expect("create bridge classes directory");
        fs::create_dir_all(directory.join("logs")).expect("create logfile directory");
        build_bridge_jar(&jdk, &manifest, &directory, &bridge_classes);
        compile_applications(&jdk, &manifest, &directory, &classes);
        assert!(
            !directory.join("wrapper.dll").exists(),
            "the fixture must not contain a native library"
        );
        Some(Self { jdk, directory })
    }

    fn write_config(&self, file_name: &str, body: &str) -> Config {
        let path = self.directory.join(file_name);
        fs::write(
            &path,
            format!("wrapper.java.command={}\n{body}", self.jdk.java.display()),
        )
        .expect("write generated configuration");
        Config::load(&path, &self.directory, &[]).expect("load generated configuration")
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.directory.join("logs").join(name)).expect("read generated log")
    }

    fn log_path(&self, name: &str) -> PathBuf {
        self.directory.join("logs").join(name)
    }
}

#[test]
fn bridge_starts_pings_dumps_and_stops_without_a_native_library() {
    let Some(fixture) = Fixture::prepare("pure-java-bridge-test") else {
        return;
    };
    let config = fixture.write_config(
        "wrapper.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.java.additional.1=-Xmx32m\n\
             wrapper.java.additional.2=-Xrs\n\
             wrapper.app.parameter.1=SyntheticApplication\n\
             wrapper.ping.interval=1\n\
             wrapper.ping.timeout=2\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.disable_restarts=true\n\
             wrapper.pidfile=wrapper.pid\n\
             wrapper.java.pidfile=java.pid\n\
             wrapper.java.idfile=java.id\n\
             jss.heapdump.directory=heapdumps\n\
             wrapper.logfile=logs/wrapper.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-pure-java-test\n"
        ),
    );
    let (control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, event_receiver) = EventPublisher::bounded(64);

    thread::scope(|scope| {
        let supervisor =
            scope.spawn(|| Supervisor::new(&config, &[], false, controls, events).run());
        let java_pid = wait_for_started(&event_receiver);
        assert_eq!(
            fs::read_to_string(fixture.directory.join("wrapper.pid"))
                .expect("read live wrapper pid file"),
            format!("{}\r\n", std::process::id())
        );
        assert_eq!(
            fs::read_to_string(fixture.directory.join("java.pid"))
                .expect("read live Java pid file"),
            format!("{java_pid}\r\n")
        );
        assert_eq!(
            fs::read_to_string(fixture.directory.join("java.id")).expect("read live Java id file"),
            "1\r\n"
        );
        wait_for_log_text(&fixture.log_path("wrapper.log"), "JSS_SYNTHETIC_READY");
        control_sender
            .send(Control::ThreadDump)
            .expect("request thread dump with -Xrs");
        wait_for_log_text(&fixture.log_path("wrapper.log"), "Full thread dump");
        control_sender
            .send(Control::HeapDump)
            .expect("request heap dump");
        let heap_dump = wait_for_heap_dump(&fixture.directory.join("heapdumps"));
        wait_for_log_text(&fixture.log_path("wrapper.log"), "Heap dump completed:");
        assert!(
            fs::metadata(&heap_dump)
                .expect("read heap dump metadata")
                .len()
                > 0
        );
        // Cross several ping intervals and the watchdog boundary. If the
        // bridge does not echo PING, the supervisor exits before this stop.
        thread::sleep(Duration::from_secs(4));
        control_sender.send(Control::Stop).expect("stop fixture");
        assert_eq!(
            supervisor
                .join()
                .expect("join supervisor")
                .expect("run supervisor"),
            0
        );
        fs::remove_file(heap_dump).expect("remove generated heap dump");
    });

    let log = fixture.log("wrapper.log");
    assert!(log.contains("Java Service Steward bridge"));
    assert!(log.contains("JSS_SYNTHETIC_READY arguments=0"));
    assert!(!log.contains("UnsatisfiedLinkError"));
    assert!(!log.contains("No ping response"));
    assert!(log.contains("Requesting a thread dump with jcmd."));
    assert!(log.contains("Full thread dump"));
    assert!(log.contains("Heap dump requested:"));
    assert!(log.contains("Heap dump completed:"));
    assert!(log.contains(STOPPED_MARKER));
    for marker in java_service_steward::FOREIGN_NAME_MARKERS {
        assert!(!log.contains(marker), "log must not contain {marker}");
    }
    assert!(!fixture.directory.join("wrapper.pid").exists());
    assert!(!fixture.directory.join("java.pid").exists());
    assert!(!fixture.directory.join("java.id").exists());
}

#[test]
fn blocked_shutdown_hook_is_dumped_and_terminated() {
    let Some(fixture) = Fixture::prepare("blocked-shutdown-test") else {
        return;
    };
    let config = fixture.write_config(
        "failed-shutdown.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=SyntheticApplication\n\
             wrapper.app.parameter.2=hang-shutdown\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=1\n\
             wrapper.request_thread_dump_on_failed_jvm_exit=TRUE\n\
             wrapper.request_thread_dump_on_failed_jvm_exit.delay=1\n\
             wrapper.disable_restarts=true\n\
             wrapper.logfile=logs/failed-shutdown.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-pure-java-test\n"
        ),
    );
    let (control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, event_receiver) = EventPublisher::bounded(64);
    thread::scope(|scope| {
        let supervisor =
            scope.spawn(|| Supervisor::new(&config, &[], false, controls, events).run());
        wait_for_started(&event_receiver);
        wait_for_log_text(
            &fixture.log_path("failed-shutdown.log"),
            "JSS_SYNTHETIC_READY",
        );
        control_sender
            .send(Control::Stop)
            .expect("request failed-shutdown fixture stop");
        assert_eq!(
            supervisor
                .join()
                .expect("join failed-shutdown supervisor")
                .expect("run failed-shutdown supervisor"),
            0
        );
    });
    let log = fixture.log("failed-shutdown.log");
    assert!(log.contains("The JVM did not stop within 1 seconds."));
    assert!(log.contains("Requesting a thread dump before terminating the JVM."));
    assert!(log.contains("JSS_SYNTHETIC_SHUTDOWN_HOOK_BLOCKED"));
    assert!(log.contains("Full thread dump"));
    assert!(log.contains("Terminating the JVM forcibly."));
    assert!(log.contains(STOPPED_MARKER));
}

#[test]
fn application_requested_exit_codes_drive_on_exit_actions() {
    let Some(fixture) = Fixture::prepare("on-exit-test") else {
        return;
    };
    let config = fixture.write_config(
        "on-exit.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=ControlApplication\n\
             wrapper.app.parameter.2=7\n\
             wrapper.on_exit.default=SHUTDOWN\n\
             wrapper.on_exit.7=RESTART\n\
             wrapper.restart.delay=0\n\
             wrapper.max_failed_invocations=2\n\
             wrapper.successful_invocation_time=300\n\
             wrapper.ping.interval=1\n\
             wrapper.ping.timeout=3\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.logfile=logs/on-exit.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-pure-java-on-exit-test\n"
        ),
    );
    let (_control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, _event_receiver) = EventPublisher::bounded(64);
    assert_eq!(
        Supervisor::new(&config, &[], false, controls, events)
            .run()
            .expect("run on-exit supervisor"),
        7
    );
    let log = fixture.log("on-exit.log");
    assert!(log.contains("jvm 1"));
    assert!(log.contains("jvm 2"));
    assert!(log.contains("2 consecutive JVM launches ended within"));
    assert!(log.contains("JSS_CONTROL_PROTOCOL_LOG"));
    assert!(log.contains("JSS_CONTROL_PROPERTY jss-pure-java-on-exit-test"));
    assert!(log.contains("JSS_CONTROL_JVM_ID 1"));
    assert!(log.contains("JSS_CONTROL_JVM_ID 2"));
    assert!(log.contains("JSS_CONTROL_MANAGED true"));
    assert!(log.contains(&format!(
        "JSS_CONTROL_VERSION {}",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(!log.contains("JSS_CONTROL_JAVA_PID 0"));
}

#[test]
fn filter_shutdown_stops_the_wrapper() {
    let Some(fixture) = Fixture::prepare("filter-shutdown-test") else {
        return;
    };
    let config = fixture.write_config(
        "filter-shutdown.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=SyntheticApplication\n\
             wrapper.filter.trigger.1=JSS_SYNTHETIC_READY*\n\
             wrapper.filter.allow_wildcards.1=TRUE\n\
             wrapper.filter.action.1=SHUTDOWN\n\
             wrapper.filter.message.1=Synthetic filter.\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.logfile=logs/filter-shutdown.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-pure-java-filter-test\n"
        ),
    );
    let (_control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, _event_receiver) = EventPublisher::bounded(64);
    assert_eq!(
        Supervisor::new(&config, &[], false, controls, events)
            .run()
            .expect("run filter shutdown supervisor"),
        0
    );
    let log = fixture.log("filter-shutdown.log");
    assert!(log.contains("Synthetic filter.  Filter action: stopping."));
    assert!(log.contains(STOPPED_MARKER));
}

#[test]
fn application_requested_restart_launches_a_new_jvm() {
    let Some(fixture) = Fixture::prepare("bridge-restart-test") else {
        return;
    };
    let config = fixture.write_config(
        "bridge-restart.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=ControlApplication\n\
             wrapper.app.parameter.2=restart\n\
             wrapper.restart.delay=0\n\
             wrapper.max_failed_invocations=2\n\
             wrapper.successful_invocation_time=300\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.logfile=logs/bridge-restart.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-pure-java-restart-test\n"
        ),
    );
    let (_control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, _event_receiver) = EventPublisher::bounded(64);
    assert_eq!(
        Supervisor::new(&config, &[], false, controls, events)
            .run()
            .expect("run bridge restart supervisor"),
        0
    );
    let log = fixture.log("bridge-restart.log");
    assert!(log.contains("jvm 1"));
    assert!(log.contains("jvm 2"));
    assert!(log.contains("2 consecutive JVM launches ended within"));
}

#[test]
fn jvm_exits_when_the_application_main_returns() {
    let Some(fixture) = Fixture::prepare("quick-exit-test") else {
        return;
    };
    let config = fixture.write_config(
        "quick-exit.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=QuickExitApplication\n\
             wrapper.on_exit.default=SHUTDOWN\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.ping.interval=1\n\
             wrapper.ping.timeout=5\n\
             wrapper.logfile=logs/quick-exit.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-quick-exit-test\n"
        ),
    );
    let exit_code = run_with_deadline(config, Duration::from_secs(30));
    assert_eq!(exit_code, 0);
    let log = fixture.log("quick-exit.log");
    assert!(log.contains("JSS_QUICK_EXIT_RETURNING"));
    assert!(
        log.contains("JVM #1 exited with code 0"),
        "the JVM must exit on its own after main returns:\n{log}"
    );
    assert!(log.contains(STOPPED_MARKER));
    assert!(!log.contains("No ping response"));
}

#[test]
fn start_stop_launcher_invokes_the_stop_class() {
    let Some(fixture) = Fixture::prepare("start-stop-test") else {
        return;
    };
    let config = fixture.write_config(
        "start-stop.conf",
        &format!(
            "wrapper.java.mainclass={START_STOP_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=StartStopApplication\n\
             wrapper.app.parameter.2=2\n\
             wrapper.app.parameter.3=first\n\
             wrapper.app.parameter.4=second\n\
             wrapper.app.parameter.5=StartStopApplicationStop\n\
             wrapper.app.parameter.6=true\n\
             wrapper.app.parameter.7=1\n\
             wrapper.app.parameter.8=stop-argument\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.disable_restarts=true\n\
             wrapper.logfile=logs/start-stop.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-start-stop-test\n"
        ),
    );
    let (control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, event_receiver) = EventPublisher::bounded(64);
    thread::scope(|scope| {
        let supervisor =
            scope.spawn(|| Supervisor::new(&config, &[], false, controls, events).run());
        wait_for_started(&event_receiver);
        wait_for_log_text(
            &fixture.log_path("start-stop.log"),
            "JSS_STARTSTOP_READY arguments=2",
        );
        control_sender.send(Control::Stop).expect("stop fixture");
        assert_eq!(
            supervisor
                .join()
                .expect("join supervisor")
                .expect("run supervisor"),
            0
        );
    });
    let log = fixture.log("start-stop.log");
    assert!(log.contains("JSS_STARTSTOP_STOP_INVOKED arguments=1"));
    assert!(log.contains("JSS_STARTSTOP_WORKER_FINISHED"));
    assert!(!log.contains("Terminating the JVM forcibly."));
    assert!(log.contains(STOPPED_MARKER));
}

#[test]
fn jar_launcher_runs_the_manifest_main_class() {
    let Some(fixture) = Fixture::prepare("jar-app-test") else {
        return;
    };
    let application_jar = fixture.directory.join("application.jar");
    let status = Command::new(&fixture.jdk.jar)
        .arg("cfe")
        .arg(&application_jar)
        .arg("JarMainApplication")
        .arg("-C")
        .arg(fixture.directory.join("classes"))
        .arg("JarMainApplication.class")
        .status()
        .expect("package application jar");
    assert!(status.success(), "package the application jar");
    let config = fixture.write_config(
        "jar-app.conf",
        &format!(
            "wrapper.java.mainclass={JAR_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.app.parameter.1=application.jar\n\
             wrapper.app.parameter.2=extra\n\
             wrapper.filter.trigger.1=JSS_JAR_READY arguments=1\n\
             wrapper.filter.action.1=SHUTDOWN\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.logfile=logs/jar-app.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-jar-app-test\n"
        ),
    );
    let (_control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, _event_receiver) = EventPublisher::bounded(64);
    assert_eq!(
        Supervisor::new(&config, &[], false, controls, events)
            .run()
            .expect("run jar launcher supervisor"),
        0
    );
    let log = fixture.log("jar-app.log");
    assert!(log.contains("JSS_JAR_READY arguments=1"));
    assert!(log.contains(STOPPED_MARKER));
}

#[test]
fn service_listener_receives_start_control_and_stop() {
    let Some(fixture) = Fixture::prepare("listener-test") else {
        return;
    };
    let config = fixture.write_config(
        "listener.conf",
        "wrapper.java.mainclass=ListenerApplication\n\
         wrapper.java.classpath.1=jss-bridge.jar\n\
         wrapper.java.classpath.2=classes\n\
         wrapper.app.parameter.1=one\n\
         wrapper.startup.timeout=20\n\
         wrapper.shutdown.timeout=10\n\
         wrapper.disable_restarts=true\n\
         wrapper.logfile=logs/listener.log\n\
         wrapper.logfile.rollmode=NONE\n\
         wrapper.logfile.loglevel=INFO\n\
         wrapper.console.loglevel=NONE\n\
         wrapper.ntservice.name=jss-listener-test\n",
    );
    let (control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, event_receiver) = EventPublisher::bounded(64);
    thread::scope(|scope| {
        let supervisor =
            scope.spawn(|| Supervisor::new(&config, &[], false, controls, events).run());
        wait_for_started(&event_receiver);
        wait_for_log_text(
            &fixture.log_path("listener.log"),
            "JSS_LISTENER_START arguments=1",
        );
        control_sender
            .send(Control::User(200))
            .expect("send user control");
        wait_for_log_text(
            &fixture.log_path("listener.log"),
            "JSS_LISTENER_CONTROL 200",
        );
        control_sender.send(Control::Stop).expect("stop fixture");
        assert_eq!(
            supervisor
                .join()
                .expect("join supervisor")
                .expect("run supervisor"),
            0
        );
    });
    let log = fixture.log("listener.log");
    assert!(log.contains("JSS_LISTENER_MAIN_RETURNED"));
    assert!(log.contains("JSS_LISTENER_STOP_CALLED 0"));
    assert!(!log.contains("Terminating the JVM forcibly."));
    assert!(log.contains(STOPPED_MARKER));
}

#[test]
fn high_volume_output_is_logged_completely_without_ping_timeouts() {
    let Some(fixture) = Fixture::prepare("flood-test") else {
        return;
    };
    let lines = 50_000;
    let config = fixture.write_config(
        "flood.conf",
        &format!(
            "wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=FloodApplication\n\
             wrapper.app.parameter.2={lines}\n\
             wrapper.ping.interval=1\n\
             wrapper.ping.timeout=2\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=30\n\
             wrapper.disable_restarts=true\n\
             wrapper.logfile=logs/flood.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-flood-test\n"
        ),
    );
    let started = Instant::now();
    let exit_code = run_with_deadline(config, Duration::from_secs(120));
    let elapsed = started.elapsed();
    assert_eq!(exit_code, 0);
    let log = fixture.log("flood.log");
    assert_eq!(log.matches("JSS_FLOOD_LINE ").count(), lines);
    assert!(log.contains(&format!("JSS_FLOOD_DONE {lines}")));
    assert!(!log.contains("No ping response"));
    assert!(log.contains(STOPPED_MARKER));
    eprintln!("flood test logged {lines} lines in {elapsed:?}");
}

#[test]
fn runtime_only_java_keeps_core_supervision_when_jcmd_is_absent() {
    let Some(jdk) = find_jdk() else {
        return;
    };
    let Some(jdk_directory) = jdk.javac.parent().and_then(Path::parent) else {
        eprintln!("skipping runtime-only test because the JDK layout is unknown");
        return;
    };
    let runtime_java = jdk_directory.join("jre/bin/java.exe");
    let runtime_jcmd = runtime_java.with_file_name("jcmd.exe");
    if !runtime_java.is_file() || runtime_jcmd.exists() {
        eprintln!(
            "skipping runtime-only test because this JDK has no separate JRE without jcmd.exe"
        );
        return;
    }
    let Some(fixture) = Fixture::prepare("runtime-only-java-test") else {
        return;
    };
    let path = fixture.directory.join("wrapper.conf");
    fs::write(
        &path,
        format!(
            "wrapper.java.command={}\n\
             wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.java.additional.1=-Xmx32m\n\
             wrapper.java.additional.2=-Xrs\n\
             wrapper.app.parameter.1=SyntheticApplication\n\
             wrapper.ping.interval=1\n\
             wrapper.ping.timeout=2\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.disable_restarts=true\n\
             jss.heapdump.directory=heapdumps\n\
             wrapper.logfile=logs/wrapper.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-runtime-only-test\n",
            runtime_java.display()
        ),
    )
    .expect("write runtime-only configuration");
    let config =
        Config::load(&path, &fixture.directory, &[]).expect("load runtime-only configuration");
    let (control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, event_receiver) = EventPublisher::bounded(64);
    thread::scope(|scope| {
        let supervisor =
            scope.spawn(|| Supervisor::new(&config, &[], false, controls, events).run());
        wait_for_started(&event_receiver);
        wait_for_log_text(&fixture.log_path("wrapper.log"), "JSS_SYNTHETIC_READY");
        control_sender
            .send(Control::ThreadDump)
            .expect("request unavailable JCMD thread dump");
        wait_for_log_text(
            &fixture.log_path("wrapper.log"),
            "Unable to request thread dump: diagnostic capability unavailable",
        );
        control_sender
            .send(Control::HeapDump)
            .expect("request unavailable heap dump");
        wait_for_log_text(
            &fixture.log_path("wrapper.log"),
            "Unable to request heap dump: diagnostic capability unavailable",
        );

        // The absence of optional JDK tools must not trip the ping watchdog or
        // change the Java application's lifecycle.
        thread::sleep(Duration::from_secs(3));
        control_sender
            .send(Control::Stop)
            .expect("stop runtime-only fixture");
        assert_eq!(
            supervisor
                .join()
                .expect("join runtime-only supervisor")
                .expect("run runtime-only supervisor"),
            0
        );
    });

    let log = fixture.log("wrapper.log");
    assert!(log.contains("core service supervision remains available"));
    assert!(!log.contains("Heap dump requested:"));
    assert!(!fixture.directory.join("heapdumps").exists());
    assert!(!log.contains("No ping response"));
    assert!(log.contains(STOPPED_MARKER));
}

#[test]
fn bridge_runs_on_a_java_base_only_jlink_image() {
    let Some(fixture) = Fixture::prepare("java-base-jlink-test") else {
        return;
    };
    if !fixture.jdk.jlink.is_file() {
        // Java 8 has no jlink; this test only applies to modular runtimes.
        eprintln!("skipping jlink test because jlink.exe is not part of this JDK");
        return;
    }
    let runtime = fixture.directory.join("runtime");
    let status = Command::new(&fixture.jdk.jlink)
        .args([
            "--add-modules",
            "java.base",
            "--no-header-files",
            "--no-man-pages",
        ])
        .arg("--output")
        .arg(&runtime)
        .status()
        .expect("create java.base-only runtime");
    assert!(status.success(), "create java.base-only runtime");
    assert!(runtime.join("bin/java.exe").is_file());
    assert!(!runtime.join("bin/jcmd.exe").exists());

    let path = fixture.directory.join("wrapper.conf");
    fs::write(
        &path,
        format!(
            "wrapper.java.command={}\n\
             wrapper.java.mainclass={SIMPLE_ALIAS}\n\
             wrapper.java.classpath.1=jss-bridge.jar\n\
             wrapper.java.classpath.2=classes\n\
             wrapper.app.parameter.1=SyntheticApplication\n\
             wrapper.filter.trigger.1=JSS_SYNTHETIC_READY*\n\
             wrapper.filter.allow_wildcards.1=TRUE\n\
             wrapper.filter.action.1=SHUTDOWN\n\
             wrapper.startup.timeout=20\n\
             wrapper.shutdown.timeout=10\n\
             wrapper.logfile=logs/wrapper.log\n\
             wrapper.logfile.rollmode=NONE\n\
             wrapper.logfile.loglevel=INFO\n\
             wrapper.console.loglevel=NONE\n\
             wrapper.ntservice.name=jss-jlink-test\n",
            runtime.join("bin/java.exe").display()
        ),
    )
    .expect("write jlink configuration");
    let config = Config::load(&path, &fixture.directory, &[]).expect("load jlink configuration");
    let (_control_sender, controls) = crossbeam_channel::bounded(16);
    let (events, _event_receiver) = EventPublisher::bounded(64);
    assert_eq!(
        Supervisor::new(&config, &[], false, controls, events)
            .run()
            .expect("run java.base-only supervisor"),
        0
    );
    let log = fixture.log("wrapper.log");
    assert!(log.contains("JSS_SYNTHETIC_READY"));
    assert!(log.contains(STOPPED_MARKER));
    assert!(!log.contains("NoClassDefFoundError"));
    assert!(!log.contains("java.lang.management"));
}

/// Runs the supervisor on its own thread and fails if it does not finish
/// within `deadline`; a hung JVM would otherwise block the test forever.
fn run_with_deadline(config: Config, deadline: Duration) -> i32 {
    let (_control_sender, controls) = crossbeam_channel::bounded::<Control>(16);
    let (events, _event_receiver) = EventPublisher::bounded(64);
    let worker = thread::spawn(move || {
        Supervisor::new(&config, &[], false, controls, events)
            .run()
            .expect("run supervisor")
    });
    let started = Instant::now();
    while !worker.is_finished() {
        assert!(
            started.elapsed() < deadline,
            "the supervisor did not finish within {deadline:?}"
        );
        thread::sleep(Duration::from_millis(100));
    }
    worker.join().expect("join supervisor")
}

fn compile_applications(jdk: &Jdk, manifest: &Path, run_directory: &Path, classes: &Path) {
    let mut sources = Vec::new();
    collect_java_sources(&manifest.join("tests/java"), &mut sources);
    let status = Command::new(&jdk.javac)
        .args([
            "-source",
            "8",
            "-target",
            "8",
            "-Xlint:-options",
            "-encoding",
            "UTF-8",
            "-d",
        ])
        .arg(classes)
        .arg("-classpath")
        .arg(run_directory.join("jss-bridge.jar"))
        .args(&sources)
        .status()
        .expect("compile synthetic applications");
    assert!(status.success(), "compile the synthetic applications");
}

fn build_bridge_jar(jdk: &Jdk, manifest: &Path, run_directory: &Path, bridge_classes: &Path) {
    let source_root = manifest.join("java/bridge/src/main/java");
    let mut sources = Vec::new();
    collect_java_sources(&source_root, &mut sources);
    let status = Command::new(&jdk.javac)
        .args([
            "-source",
            "8",
            "-target",
            "8",
            "-Xlint:-options",
            "-encoding",
            "UTF-8",
            "-d",
        ])
        .arg(bridge_classes)
        .args(&sources)
        .status()
        .expect("compile Java bridge");
    assert!(status.success(), "compile the Java bridge");

    let generated_manifest = run_directory.join("MANIFEST.MF");
    let manifest_text = fs::read_to_string(manifest.join("java/bridge/MANIFEST.MF"))
        .expect("read Java bridge manifest template")
        .replace("@PROJECT_VERSION@", env!("CARGO_PKG_VERSION"));
    fs::write(&generated_manifest, manifest_text).expect("write generated Java bridge manifest");

    let status = Command::new(&jdk.jar)
        .arg("cfm")
        .arg(run_directory.join("jss-bridge.jar"))
        .arg(generated_manifest)
        .arg("-C")
        .arg(bridge_classes)
        .arg(".")
        .status()
        .expect("package Java bridge");
    assert!(status.success(), "package the Java bridge");
}

fn collect_java_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| {
            panic!(
                "read Java source directory {}: {error}",
                directory.display()
            )
        })
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_dir() {
            collect_java_sources(&path, output);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "java")
        {
            output.push(path);
        }
    }
}

fn wait_for_started(
    events: &crossbeam_channel::Receiver<java_service_steward::telemetry::Event>,
) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                if let EventKind::JvmStarted { pid, .. } = event.kind {
                    return pid;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    panic!("the Java bridge did not report started");
}

fn wait_for_log_text(path: &Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if fs::read_to_string(path).is_ok_and(|text| text.contains(expected)) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "generated log {} did not contain {expected}",
        path.display()
    );
}

fn wait_for_heap_dump(directory: &Path) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "hprof")
                    && fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 0)
                {
                    return path;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("heap dump was not created in {}", directory.display());
}

/// Locates the JDK tools next to `javac.exe` on `PATH`. Returns `None` and
/// prints the reason when they are missing, or panics when
/// `JSS_REQUIRE_JAVA_TESTS=1` demands a JDK.
fn find_jdk() -> Option<Jdk> {
    let required = std::env::var("JSS_REQUIRE_JAVA_TESTS").is_ok_and(|value| value == "1");
    let skip = |reason: &str| -> Option<Jdk> {
        assert!(!required, "JSS_REQUIRE_JAVA_TESTS=1 but {reason}");
        eprintln!("skipping JVM test because {reason}");
        None
    };
    let Some(javac) = find_on_path("javac.exe") else {
        return skip("javac.exe is not on PATH");
    };
    let jdk = Jdk {
        java: javac.with_file_name("java.exe"),
        jar: javac.with_file_name("jar.exe"),
        jcmd: javac.with_file_name("jcmd.exe"),
        jlink: javac.with_file_name("jlink.exe"),
        javac,
    };
    for (name, path) in [
        ("java.exe", &jdk.java),
        ("jar.exe", &jdk.jar),
        ("jcmd.exe", &jdk.jcmd),
    ] {
        if !path.is_file() {
            return skip(&format!("{name} is missing next to javac.exe"));
        }
    }
    Some(jdk)
}

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let output = Command::new("where.exe").arg(executable).output().ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(PathBuf::from)
    })?
}
