// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::process::{Child, Command};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleSignal {
    Interrupt,
    Break,
}

#[cfg(windows)]
mod platform {
    #![allow(unsafe_code)]

    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE};
    use windows_sys::Win32::System::Console::{
        AllocConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT, GenerateConsoleCtrlEvent, GetConsoleWindow, SetConsoleCtrlHandler,
        SetConsoleTitleW,
    };
    use windows_sys::Win32::System::EventLog::{
        DeregisterEventSource, EVENTLOG_ERROR_TYPE, RegisterEventSourceW, ReportEventW,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};

    use super::{Child, Command, ConsoleSignal, Result};

    static CONSOLE_SIGNALS: OnceLock<crossbeam_channel::Sender<ConsoleSignal>> = OnceLock::new();

    unsafe extern "system" fn console_control_handler(control: u32) -> i32 {
        let signal = match control {
            CTRL_C_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                ConsoleSignal::Interrupt
            }
            CTRL_BREAK_EVENT => ConsoleSignal::Break,
            _ => return 0,
        };
        CONSOLE_SIGNALS
            .get()
            .is_some_and(|sender| sender.try_send(signal).is_ok())
            .into()
    }

    pub fn configure_child(command: &mut Command) {
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    pub fn console_signal_channel() -> Result<crossbeam_channel::Receiver<ConsoleSignal>> {
        let (sender, receiver) = crossbeam_channel::bounded(16);
        CONSOLE_SIGNALS.set(sender).map_err(|_| {
            crate::error::Error::Config("the console handler was already registered".into())
        })?;
        // SAFETY: console_control_handler has the required system ABI and a
        // process-long lifetime. It only publishes into a bounded channel.
        if unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 1) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(receiver)
    }

    pub fn prepare_service_console(enabled: bool) -> Result<()> {
        if !enabled {
            return Ok(());
        }

        // SAFETY: these Win32 functions take no borrowed pointers. The window
        // handle returned by GetConsoleWindow is used immediately and is never
        // retained. AllocConsole is called only when no console is attached.
        unsafe {
            if !GetConsoleWindow().is_null() {
                return Ok(());
            }
            if AllocConsole() == 0 {
                let error = std::io::Error::last_os_error();
                // ConPTY sessions can have a console without exposing a
                // console window. AllocConsole then reports ACCESS_DENIED,
                // which means no additional allocation is necessary.
                if error.raw_os_error() != Some(ERROR_ACCESS_DENIED as i32) {
                    return Err(error.into());
                }
                return Ok(());
            }
            let window = GetConsoleWindow();
            if !window.is_null() {
                ShowWindow(window, SW_HIDE);
            }
        }
        Ok(())
    }

    pub fn request_thread_dump(process_group_id: u32) -> Result<()> {
        // SAFETY: GenerateConsoleCtrlEvent receives only integer values. The
        // child was created with CREATE_NEW_PROCESS_GROUP, making its PID the
        // process-group identifier required by this call.
        if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, process_group_id) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub fn set_console_title(title: &str) -> Result<()> {
        let wide = wide_string(title);
        // SAFETY: `wide` is NUL-terminated and remains alive for the duration
        // of the call. SetConsoleTitleW does not retain the supplied pointer.
        if unsafe { SetConsoleTitleW(wide.as_ptr()) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    /// Writes an error record to the Windows Application event log under
    /// `source`. Used when a service cannot start before its log file exists.
    pub fn report_event_log_error(source: &str, message: &str) -> Result<()> {
        let source_wide = wide_string(source);
        let message_wide = wide_string(message);
        // SAFETY: both buffers are NUL-terminated and outlive the calls. The
        // handle returned by RegisterEventSourceW is released before returning.
        unsafe {
            let handle = RegisterEventSourceW(std::ptr::null(), source_wide.as_ptr());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let strings = [message_wide.as_ptr()];
            let reported = ReportEventW(
                handle,
                EVENTLOG_ERROR_TYPE,
                0,
                0,
                std::ptr::null_mut(),
                1,
                0,
                strings.as_ptr(),
                std::ptr::null(),
            );
            let error = std::io::Error::last_os_error();
            DeregisterEventSource(handle);
            if reported == 0 {
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// A Windows Job Object configured so that every process assigned to it
    /// is terminated when the last handle to the job closes, which includes
    /// the wrapper process ending for any reason.
    pub struct JobObject {
        handle: HANDLE,
    }

    // SAFETY: a job handle is a kernel object reference without thread affinity.
    unsafe impl Send for JobObject {}

    impl JobObject {
        pub fn kill_on_close() -> Result<Self> {
            // SAFETY: CreateJobObjectW accepts null attributes and a null
            // name. The returned handle is owned by this struct and closed in
            // Drop. The limit structure is zero-initialized plain data.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let job = Self { handle };
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let applied = SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if applied == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                Ok(job)
            }
        }

        pub fn assign(&self, child: &Child) -> Result<()> {
            // SAFETY: both handles are valid for the duration of the call; the
            // child handle is borrowed from the live `Child`.
            if unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle() as HANDLE) }
                == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by CreateJobObjectW and is
            // closed exactly once.
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    fn wide_string(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{Child, Command, ConsoleSignal, Result};
    use crate::error::Error;

    pub fn configure_child(_command: &mut Command) {}

    pub fn console_signal_channel() -> Result<crossbeam_channel::Receiver<ConsoleSignal>> {
        let (sender, receiver) = crossbeam_channel::bounded(16);
        ctrlc::set_handler(move || {
            let _ = sender.try_send(ConsoleSignal::Interrupt);
        })
        .map_err(|error| {
            Error::Config(format!("could not register the Ctrl+C handler: {error}"))
        })?;
        Ok(receiver)
    }

    pub fn prepare_service_console(_enabled: bool) -> Result<()> {
        Ok(())
    }

    pub fn request_thread_dump(_process_group_id: u32) -> Result<()> {
        Err(Error::UnsupportedPlatform("CTRL_BREAK thread dumps"))
    }

    pub fn set_console_title(_title: &str) -> Result<()> {
        Ok(())
    }

    pub fn report_event_log_error(_source: &str, _message: &str) -> Result<()> {
        Err(Error::UnsupportedPlatform("the Windows event log"))
    }

    pub struct JobObject;

    impl JobObject {
        pub fn kill_on_close() -> Result<Self> {
            Err(Error::UnsupportedPlatform("job objects"))
        }

        pub fn assign(&self, _child: &Child) -> Result<()> {
            Ok(())
        }
    }
}

pub use platform::{
    JobObject, configure_child, console_signal_channel, prepare_service_console,
    report_event_log_error, request_thread_dump, set_console_title,
};
