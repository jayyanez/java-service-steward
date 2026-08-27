// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Java command construction, main-class resolution and bridge detection.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::PRODUCT_VERSION;
use crate::config::{Config, is_sensitive_name, remove_grouping_quotes};
use crate::error::{Error, Result};

/// Package of the bundled Java bridge.
pub const BRIDGE_PACKAGE: &str = "io.github.jayyanez.jss.bridge";

/// Simple names of the launcher classes shipped in the bridge.
pub const BUNDLED_LAUNCHERS: &[&str] = &["SimpleApp", "StartStopApp", "JarApp"];

/// Class file whose presence identifies the bundled bridge on a classpath.
pub const BRIDGE_MARKER_CLASS: &str = "io/github/jayyanez/jss/bridge/SimpleApp.class";
const JAR_SCAN_LIMIT: u64 = 4 * 1024 * 1024;

pub const MISSING_BRIDGE_MESSAGE: &str = "wrapper.java.classpath does not contain the bundled Java Service Steward bridge (wrapper.jar). Only the bundled wrapper.jar is supported; deploy it at the path named by wrapper.java.classpath.";

/// How `wrapper.java.mainclass` is launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainClass {
    /// One of the bundled launchers. `mapped_from` records a configured class
    /// name that was substituted because it is absent from the classpath and
    /// named like a launcher (for example a launcher of another product).
    Bundled {
        launcher: &'static str,
        mapped_from: Option<String>,
    },
    /// Any other class, launched exactly as configured.
    Direct(String),
}

impl MainClass {
    #[must_use]
    pub fn launched(&self) -> String {
        match self {
            Self::Bundled { launcher, .. } => format!("{BRIDGE_PACKAGE}.{launcher}"),
            Self::Direct(name) => name.clone(),
        }
    }

    #[must_use]
    pub fn requires_bridge(&self) -> bool {
        matches!(self, Self::Bundled { .. })
    }
}

/// Resolves the configured main class:
///
/// 1. a bundled launcher named explicitly is used as is;
/// 2. a class that is **not** on the classpath and whose simple name ends
///    with the name of a bundled launcher is replaced by that launcher, so a
///    configuration written for another product's launcher keeps working
///    unchanged;
/// 3. anything else is launched exactly as configured.
#[must_use]
pub fn resolve_main_class(configured: &str, classpath: &[PathBuf]) -> MainClass {
    if let Some(launcher) = explicit_launcher(configured) {
        return MainClass::Bundled {
            launcher,
            mapped_from: None,
        };
    }
    let simple_name = configured.rsplit('.').next().unwrap_or(configured);
    let lookalike = BUNDLED_LAUNCHERS
        .iter()
        .copied()
        .find(|launcher| simple_name.ends_with(launcher));
    if let Some(launcher) = lookalike
        && !classpath_contains_class(classpath, configured)
    {
        return MainClass::Bundled {
            launcher,
            mapped_from: Some(configured.to_owned()),
        };
    }
    MainClass::Direct(configured.to_owned())
}

fn explicit_launcher(configured: &str) -> Option<&'static str> {
    let simple_name = configured.strip_prefix(BRIDGE_PACKAGE)?.strip_prefix('.')?;
    BUNDLED_LAUNCHERS
        .iter()
        .copied()
        .find(|launcher| *launcher == simple_name)
}

#[derive(Debug, Clone)]
pub struct BackendLaunch {
    pub key: String,
    pub port: u16,
    pub jvm_port: Option<u16>,
    pub jvm_port_min: u16,
    pub jvm_port_max: u16,
    pub jvm_id: u32,
    pub service: bool,
}

#[derive(Debug, Clone)]
pub struct JvmCommand {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    /// Operator-facing note about how the main class was resolved, when the
    /// configured value was substituted.
    pub launch_note: Option<String>,
}

impl JvmCommand {
    pub fn build(
        config: &Config,
        backend: &BackendLaunch,
        application_args: &[OsString],
    ) -> Result<Self> {
        let program = resolve_java_command(config, config.required("wrapper.java.command")?);
        let mut arguments = Vec::<OsString>::new();

        for (_, value) in config.numbered("wrapper.java.additional") {
            arguments.extend(parse_legacy_windows_fragment(value));
        }

        let init_memory = config.get_u64("wrapper.java.initmemory", 0);
        if init_memory > 0 {
            arguments.push(format!("-Xms{}m", init_memory.max(1)).into());
        }
        let max_memory = config.get_u64("wrapper.java.maxmemory", 0);
        if max_memory > 0 {
            arguments.push(format!("-Xmx{}m", max_memory.max(init_memory.max(1))).into());
        }

        let library_path = joined_paths(&resolved_paths(config, "wrapper.java.library.path"));
        if !library_path.is_empty() {
            arguments.push(format!("-Djava.library.path={library_path}").into());
        }

        let classpath_entries = resolved_paths(config, "wrapper.java.classpath");
        let classpath = joined_paths(&classpath_entries);
        if !classpath.is_empty() {
            arguments.push("-classpath".into());
            arguments.push(classpath.into());
        }

        arguments.push(format!("-Dwrapper.key={}", backend.key).into());
        arguments.push(format!("-Dwrapper.port={}", backend.port).into());
        if let Some(port) = backend.jvm_port {
            arguments.push(format!("-Dwrapper.jvm.port={port}").into());
        }
        arguments.push(format!("-Dwrapper.jvm.port.min={}", backend.jvm_port_min).into());
        arguments.push(format!("-Dwrapper.jvm.port.max={}", backend.jvm_port_max).into());
        if config.get_bool("wrapper.debug", false) {
            arguments.push("-Dwrapper.debug=TRUE".into());
        }
        copy_boolean_property(config, &mut arguments, "wrapper.disable_console_input");
        copy_boolean_property(config, &mut arguments, "wrapper.listener.force_stop");
        arguments.push(format!("-Dwrapper.pid={}", std::process::id()).into());
        if config.get_bool("wrapper.use_system_time", false) {
            arguments.push("-Dwrapper.use_system_time=TRUE".into());
        }
        if backend.service {
            arguments.push("-Dwrapper.service=TRUE".into());
        }
        copy_boolean_property(config, &mut arguments, "wrapper.disable_shutdown_hook");
        arguments.push(format!("-Dwrapper.jvmid={}", backend.jvm_id).into());
        arguments.push(format!("-Djss.version={PRODUCT_VERSION}").into());

        let configured_main =
            remove_grouping_quotes(config.get_or("wrapper.java.mainclass", "Main")).trim();
        let main_class = resolve_main_class(configured_main, &classpath_entries);
        if main_class.requires_bridge() && !classpath_contains_bridge(&classpath_entries) {
            return Err(Error::Config(MISSING_BRIDGE_MESSAGE.into()));
        }
        let launched_main = main_class.launched();
        let launch_note = match &main_class {
            MainClass::Bundled {
                mapped_from: Some(configured),
                ..
            } => Some(format!(
                "Main class {configured} is not on the classpath; launching the bundled {launched_main} instead."
            )),
            _ => None,
        };
        arguments.push(launched_main.into());
        for (_, value) in config.numbered("wrapper.app.parameter") {
            arguments.extend(parse_legacy_windows_fragment(value));
        }
        arguments.extend_from_slice(application_args);

        let working_directory = config.get("wrapper.working.dir").map_or_else(
            || config.executable_directory().to_path_buf(),
            |path| config.resolve_path(path),
        );
        let environment = config
            .environment()
            .iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();

        Ok(Self {
            program,
            arguments,
            working_directory,
            environment,
            launch_note,
        })
    }

    #[must_use]
    pub fn redacted_summary(&self) -> String {
        format!(
            "{} ({} arguments; working directory {})",
            self.program.display(),
            self.arguments.len(),
            self.working_directory.display()
        )
    }

    #[must_use]
    pub fn redacted_command_line(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.arguments.iter().map(|argument| {
                    redact_command_argument(&argument.to_string_lossy()).into_owned()
                }),
            )
            .map(|argument| quote_for_display(&argument))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Parses one configured argument fragment using the quoting rules used by a
/// Windows command line: unquoted spaces split arguments while grouping quotes
/// do not become part of the Java-visible value.
#[must_use]
pub fn parse_legacy_windows_fragment(fragment: &str) -> Vec<OsString> {
    let characters: Vec<char> = fragment.chars().collect();
    let mut arguments = Vec::new();
    let mut cursor = 0;
    while cursor < characters.len() {
        while cursor < characters.len() && characters[cursor].is_whitespace() {
            cursor += 1;
        }
        if cursor == characters.len() {
            break;
        }

        let mut argument = String::new();
        let mut quoted = false;
        let mut saw_token = false;
        while cursor < characters.len() {
            if !quoted && characters[cursor].is_whitespace() {
                break;
            }
            if characters[cursor] == '\\' {
                let start = cursor;
                while cursor < characters.len() && characters[cursor] == '\\' {
                    cursor += 1;
                }
                let slash_count = cursor - start;
                if cursor < characters.len() && characters[cursor] == '"' {
                    argument.extend(std::iter::repeat_n('\\', slash_count / 2));
                    if slash_count % 2 == 0 {
                        quoted = !quoted;
                    } else {
                        argument.push('"');
                    }
                    saw_token = true;
                    cursor += 1;
                } else {
                    argument.extend(std::iter::repeat_n('\\', slash_count));
                }
                continue;
            }
            if characters[cursor] == '"' {
                quoted = !quoted;
                saw_token = true;
                cursor += 1;
                continue;
            }
            argument.push(characters[cursor]);
            saw_token = true;
            cursor += 1;
        }
        if saw_token {
            arguments.push(argument.into());
        }
    }
    arguments
}

fn copy_boolean_property(config: &Config, arguments: &mut Vec<OsString>, name: &str) {
    if config.get_bool(name, false) {
        arguments.push(format!("-D{name}=TRUE").into());
    }
}

fn redact_command_argument(argument: &str) -> std::borrow::Cow<'_, str> {
    let property = argument.strip_prefix("-D").unwrap_or(argument);
    let Some((name, _)) = property.split_once('=') else {
        return argument.into();
    };
    if is_sensitive_name(name) {
        let prefix_length = argument.len() - property.len();
        return format!("{}{}=<redacted>", &argument[..prefix_length], name).into();
    }
    argument.into()
}

fn quote_for_display(argument: &str) -> String {
    if argument.is_empty() || argument.chars().any(char::is_whitespace) {
        format!("\"{}\"", argument.replace('"', "\\\""))
    } else {
        argument.into()
    }
}

fn resolve_java_command(config: &Config, configured: &str) -> PathBuf {
    let configured = remove_grouping_quotes(configured.trim());
    let path = PathBuf::from(configured);
    if path.is_absolute()
        || path
            .parent()
            .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        path
    } else {
        config.resolve_path(configured)
    }
}

fn resolved_paths(config: &Config, prefix: &str) -> Vec<PathBuf> {
    config
        .numbered(prefix)
        .into_iter()
        .filter_map(|(_, value)| {
            let value = remove_grouping_quotes(value.trim());
            (!value.is_empty()).then(|| config.resolve_path(value))
        })
        .collect()
}

fn joined_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" })
}

/// Checks whether the bundled bridge is reachable through the classpath
/// entries: an explicit JAR, a class directory, or a `dir/*` wildcard.
#[must_use]
pub fn classpath_contains_bridge(entries: &[PathBuf]) -> bool {
    classpath_contains_entry(entries, BRIDGE_MARKER_CLASS)
}

/// Checks whether a class (dotted name) is reachable through the classpath
/// entries. Nested JARs and custom class loaders are not inspected, so a
/// negative answer is a hint, not a proof.
#[must_use]
pub fn classpath_contains_class(entries: &[PathBuf], class_name: &str) -> bool {
    let relative = format!("{}.class", class_name.replace('.', "/"));
    classpath_contains_entry(entries, &relative)
}

fn classpath_contains_entry(entries: &[PathBuf], relative: &str) -> bool {
    entries.iter().any(|entry| {
        if entry.file_name().is_some_and(|name| name == "*") {
            return entry
                .parent()
                .is_some_and(|directory| directory_jars_contain_entry(directory, relative));
        }
        if entry.is_dir() {
            return entry.join(Path::new(relative)).is_file();
        }
        is_jar(entry) && jar_contains_entry(entry, relative)
    })
}

fn directory_jars_contain_entry(directory: &Path, relative: &str) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    entries.filter_map(std::result::Result::ok).any(|entry| {
        let path = entry.path();
        is_jar(&path) && jar_contains_entry(&path, relative)
    })
}

fn is_jar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}

/// JAR entry names are repeated verbatim in the ZIP central directory, which
/// sits at the end of the file. Reading only a bounded tail avoids loading a
/// large application JAR at startup while reliably finding an entry name.
fn jar_contains_entry(path: &Path, relative: &str) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return false;
    };
    let scan_length = length.min(JAR_SCAN_LIMIT);
    if file
        .seek(SeekFrom::Start(length.saturating_sub(scan_length)))
        .is_err()
    {
        return false;
    }
    let mut bytes = Vec::with_capacity(scan_length as usize);
    let needle = relative.as_bytes();
    file.take(scan_length).read_to_end(&mut bytes).is_ok()
        && bytes
            .windows(needle.len())
            .any(|candidate| candidate == needle)
}

#[cfg(test)]
mod tests {
    use super::{
        BRIDGE_MARKER_CLASS, BRIDGE_PACKAGE, BUNDLED_LAUNCHERS, BackendLaunch, JvmCommand,
        MISSING_BRIDGE_MESSAGE, MainClass, parse_legacy_windows_fragment, resolve_main_class,
    };
    use crate::config::Config;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    /// A launcher name from another product: not on the classpath, ends in
    /// the name of a bundled launcher.
    const FOREIGN_SIMPLE_LAUNCHER: &str = "legacy.launchers.LegacySimpleApp";
    const SIMPLE_APP: &str = "io.github.jayyanez.jss.bridge.SimpleApp";

    #[test]
    fn builds_java_command_and_skips_empty_additional_values() {
        let directory = unique_directory();
        write_synthetic_bridge(&directory.join("wrapper.jar"));
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            format!(
                "wrapper.java.command=java.exe\nwrapper.java.additional.1=-Done=1\nwrapper.java.additional.2=\nwrapper.java.additional.3=-Dthree=3\nwrapper.java.classpath.1=wrapper.jar\nwrapper.java.mainclass={FOREIGN_SIMPLE_LAUNCHER}\nwrapper.app.parameter.1=example.Main\n"
            ),
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let command = JvmCommand::build(
            &config,
            &BackendLaunch {
                key: "synthetic-key".into(),
                port: 32_123,
                jvm_port: None,
                jvm_port_min: 31_000,
                jvm_port_max: 31_999,
                jvm_id: 1,
                service: true,
            },
            &[OsString::from("passthrough")],
        )
        .expect("build command");
        let args: Vec<String> = command
            .arguments
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-Done=1".into()));
        assert!(args.contains(&"-Dthree=3".into()));
        assert!(!args.contains(&String::new()));
        assert!(args.contains(&format!("-Djss.version={}", env!("CARGO_PKG_VERSION"))));
        assert!(args.contains(&"-Dwrapper.service=TRUE".into()));
        assert!(args.contains(&SIMPLE_APP.into()));
        assert!(!args.contains(&FOREIGN_SIMPLE_LAUNCHER.into()));
        assert!(!args.iter().any(|arg| arg.starts_with("-Dwrapper.version=")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("-Dwrapper.native_library="))
        );
        assert_eq!(args.last(), Some(&"passthrough".into()));
        let note = command.launch_note.expect("substitution is reported");
        assert!(note.contains(FOREIGN_SIMPLE_LAUNCHER) && note.contains(SIMPLE_APP));

        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn command_diagnostics_redact_sensitive_system_properties() {
        let command = JvmCommand {
            program: "java.exe".into(),
            arguments: vec![
                "-Dordinary=value".into(),
                "-Dwrapper.key=do-not-log".into(),
                "-DdatabasePassword=also-secret".into(),
                "argument with spaces".into(),
            ],
            working_directory: ".".into(),
            environment: Vec::new(),
            launch_note: None,
        };
        let rendered = command.redacted_command_line();
        assert!(rendered.contains("-Dordinary=value"));
        assert!(rendered.contains("-Dwrapper.key=<redacted>"));
        assert!(rendered.contains("-DdatabasePassword=<redacted>"));
        assert!(rendered.contains("\"argument with spaces\""));
        assert!(!rendered.contains("do-not-log"));
        assert!(!rendered.contains("also-secret"));
    }

    #[test]
    fn absent_classes_named_like_a_launcher_map_to_the_bundled_one() {
        let empty: Vec<PathBuf> = Vec::new();
        for launcher in BUNDLED_LAUNCHERS {
            let foreign = format!("legacy.launchers.Legacy{launcher}");
            assert_eq!(
                resolve_main_class(&foreign, &empty),
                MainClass::Bundled {
                    launcher,
                    mapped_from: Some(foreign.clone()),
                }
            );
            let explicit = format!("{BRIDGE_PACKAGE}.{launcher}");
            let resolved = resolve_main_class(&explicit, &empty);
            assert_eq!(
                resolved,
                MainClass::Bundled {
                    launcher,
                    mapped_from: None,
                }
            );
            assert_eq!(resolved.launched(), explicit);
            assert!(resolved.requires_bridge());
        }
        let direct = resolve_main_class("example.Main", &empty);
        assert_eq!(direct, MainClass::Direct("example.Main".into()));
        assert!(!direct.requires_bridge());
        assert_eq!(
            resolve_main_class("SimpleApplication", &empty),
            MainClass::Direct("SimpleApplication".into())
        );
    }

    #[test]
    fn a_launcher_lookalike_present_on_the_classpath_is_launched_directly() {
        let directory = unique_directory();
        let classes = directory.join("classes");
        let own = classes.join("com/acme/SimpleApp.class");
        fs::create_dir_all(own.parent().expect("package directory")).expect("create package");
        fs::write(&own, b"class").expect("write application class");
        assert_eq!(
            resolve_main_class("com.acme.SimpleApp", &[classes]),
            MainClass::Direct("com.acme.SimpleApp".into())
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn direct_main_class_is_launched_without_the_bridge() {
        let directory = unique_directory();
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            "wrapper.java.command=java\nwrapper.java.mainclass=example.Main\n",
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        let command = JvmCommand::build(&config, &backend(), &[]).expect("build command");
        assert_eq!(command.program, std::path::PathBuf::from("java"));
        assert!(
            command
                .arguments
                .iter()
                .any(|argument| argument == "example.Main")
        );
        assert!(command.launch_note.is_none());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn launcher_without_the_bridge_fails_before_launching_java() {
        let directory = unique_directory();
        fs::write(directory.join("wrapper.jar"), b"not the bridge").expect("write foreign jar");
        let config = load_launcher_config(&directory);
        let error = JvmCommand::build(&config, &backend(), &[]).expect_err("missing bridge");
        assert_eq!(
            error.to_string(),
            format!("invalid configuration: {MISSING_BRIDGE_MESSAGE}")
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn detects_the_bridge_inside_a_wildcard_classpath() {
        let directory = unique_directory();
        let core = directory.join("core");
        fs::create_dir_all(&core).expect("create wildcard directory");
        write_synthetic_bridge(&core.join("wrapper.jar"));
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            format!(
                "wrapper.java.command=java.exe\nwrapper.java.classpath.1=core/*\nwrapper.java.mainclass={FOREIGN_SIMPLE_LAUNCHER}\nwrapper.app.parameter.1=example.Main\n"
            ),
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");

        let command = JvmCommand::build(&config, &backend(), &[]).expect("build command");
        assert!(
            command
                .arguments
                .iter()
                .any(|argument| argument == SIMPLE_APP)
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn detects_the_bridge_inside_a_class_directory() {
        let directory = unique_directory();
        let classes = directory.join("classes");
        let marker = classes.join(BRIDGE_MARKER_CLASS);
        fs::create_dir_all(marker.parent().expect("marker parent")).expect("create package");
        fs::write(&marker, b"class").expect("write marker class");
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            format!(
                "wrapper.java.command=java.exe\nwrapper.java.classpath.1=classes\nwrapper.java.mainclass={SIMPLE_APP}\nwrapper.app.parameter.1=example.Main\n"
            ),
        )
        .expect("write config");
        let config = Config::load(&path, &directory, &[]).expect("load config");
        JvmCommand::build(&config, &backend(), &[]).expect("build command");
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn parses_grouping_quotes_like_the_windows_command_line() {
        assert_eq!(
            parse_legacy_windows_fragment("-Dapp.home=\"C:\\Program Files\\App\""),
            ["-Dapp.home=C:\\Program Files\\App"]
        );
        assert_eq!(
            parse_legacy_windows_fragment("unquoted spaces split"),
            ["unquoted", "spaces", "split"]
        );
        assert_eq!(parse_legacy_windows_fragment("\"\""), [""]);
        assert!(parse_legacy_windows_fragment("").is_empty());
    }

    fn write_synthetic_bridge(path: &std::path::Path) {
        fs::write(
            path,
            format!("synthetic {BRIDGE_MARKER_CLASS} marker").as_bytes(),
        )
        .expect("write synthetic bridge");
    }

    fn unique_directory() -> std::path::PathBuf {
        crate::test_support::unique_directory("jvm")
    }

    fn load_launcher_config(directory: &std::path::Path) -> Config {
        let path = directory.join("wrapper.conf");
        fs::write(
            &path,
            format!(
                "wrapper.java.command=java.exe\nwrapper.java.classpath.1=wrapper.jar\nwrapper.java.mainclass={FOREIGN_SIMPLE_LAUNCHER}\nwrapper.app.parameter.1=example.Main\n"
            ),
        )
        .expect("write config");
        Config::load(&path, directory, &[]).expect("load config")
    }

    fn backend() -> BackendLaunch {
        BackendLaunch {
            key: "synthetic-key".into(),
            port: 32_123,
            jvm_port: None,
            jvm_port_min: 31_000,
            jvm_port_max: 31_999,
            jvm_id: 1,
            service: false,
        }
    }
}
