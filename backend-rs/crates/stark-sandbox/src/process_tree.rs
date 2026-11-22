//! Process-tree containment for spawned commands.
//!
//! Killing the shell process is not enough: `npm run dev`, `cargo watch`, or a
//! command that starts a background daemon can leave descendants running after
//! the harness times out or the user cancels. Windows Job Objects and Unix
//! process groups terminate the whole tree instead.
//!
//! Windows assignment happens immediately after spawn. A descendant created in
//! the tiny window before assignment can escape; Windows offers no portable
//! "assign before exec" for Tokio/PortablePTY children without a suspended
//! launch. Assignment failure is reported to the caller, which logs it and
//! falls back to killing the root process.

use std::io;

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// Owns a Windows Job Object configured to kill its members when the last
    /// handle closes.
    #[derive(Debug)]
    pub struct ProcessTree {
        job: OwnedHandle,
    }

    impl ProcessTree {
        pub fn create() -> io::Result<Self> {
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: CreateJobObjectW returned a unique owned handle.
            let job = unsafe { OwnedHandle::from_raw_handle(raw as RawHandle) };

            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job.as_raw_handle() as HANDLE,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { job })
        }

        /// Assigns a just-spawned process to the job.
        pub fn assign(&self, process: RawHandle) -> io::Result<()> {
            let assigned = unsafe {
                AssignProcessToJobObject(self.job.as_raw_handle() as HANDLE, process as HANDLE)
            };
            if assigned == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Terminates every process currently in the job.
        pub fn terminate(&self) -> io::Result<()> {
            let terminated = unsafe {
                TerminateJobObject(self.job.as_raw_handle() as HANDLE, /*exit_code*/ 1)
            };
            if terminated == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::io;

    /// A Unix process group. The caller must put the child in its own group
    /// (`process_group(0)` / `setsid`) for group termination to reach
    /// descendants.
    #[derive(Debug, Clone, Copy)]
    pub struct ProcessTree {
        pgid: u32,
    }

    impl ProcessTree {
        /// Placeholder until the child pid is known.
        pub fn pending() -> Self {
            Self { pgid: 0 }
        }

        pub fn for_pid(pid: u32) -> io::Result<Self> {
            if pid == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cannot create a process group for pid 0",
                ));
            }
            Ok(Self { pgid: pid })
        }

        pub fn terminate(&self) -> io::Result<()> {
            // A negative pid targets the process group. If the child was not a
            // group leader, fall back to the root process.
            let group = unsafe { libc::kill(-(self.pgid as i32), libc::SIGKILL) };
            if group == 0 {
                return Ok(());
            }
            let root = unsafe { libc::kill(self.pgid as i32, libc::SIGKILL) };
            if root == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use std::io;

    #[derive(Debug, Clone, Copy)]
    pub struct ProcessTree;

    impl ProcessTree {
        pub fn create() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn for_pid(_pid: u32) -> io::Result<Self> {
            Ok(Self)
        }

        pub fn assign(&self, _process: *mut std::ffi::c_void) -> io::Result<()> {
            Ok(())
        }

        pub fn terminate(&self) -> io::Result<()> {
            Ok(())
        }
    }
}

pub use imp::ProcessTree;

/// Creates an empty containment scope before spawning.
pub fn create() -> io::Result<ProcessTree> {
    #[cfg(windows)]
    {
        ProcessTree::create()
    }
    #[cfg(unix)]
    {
        // The process group is created after spawn, once the pid is known.
        Ok(ProcessTree::pending())
    }
    #[cfg(not(any(windows, unix)))]
    {
        ProcessTree::create()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    #[test]
    fn windows_job_terminates_a_running_child() {
        use std::os::windows::io::AsRawHandle;

        let tree = ProcessTree::create().expect("create job");
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .expect("spawn ping");
        // GitHub/desktop runners may already put the process in a job that
        // forbids nested assignment. Skip rather than fail the suite.
        if tree.assign(child.as_raw_handle()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        let started = Instant::now();
        tree.terminate().expect("terminate job");
        let status = child.wait().expect("wait child");
        assert!(!status.success());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "job termination did not stop the child promptly"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_process_group_terminates_a_running_child() {
        use std::os::unix::process::CommandExt;

        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .expect("spawn sh");
        let tree = ProcessTree::for_pid(child.id()).expect("process group");
        tree.terminate().expect("kill group");
        let status = child.wait().expect("wait child");
        assert!(!status.success());
    }
}
