//! Child processes that die with hanstat, however it ends: closed, crashed
//! or killed from Task Manager. `Drop` alone cannot promise that, and an
//! orphaned `llama-server.exe` keeps gigabytes of GPU memory (three of them
//! held 7 GB of a 12 GB RTX 5070 on 2026-11-17, and slowed the next rating
//! run fifteenfold).
//!
//! - Windows: the child goes into a Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` whose only handle hanstat holds
//!   (not inheritable), so the kernel kills the job's processes when that
//!   handle is closed, which it is when hanstat's process ends for any
//!   reason. Only the child is put in the job, not hanstat itself: what
//!   hanstat opens with the shell (a browser, Explorer) or the updater
//!   starts must outlive it.
//! - Linux: `PR_SET_PDEATHSIG` with `SIGKILL`. The signal follows the
//!   *thread* that spawned the child, so spawn from a thread that lives at
//!   least as long as the child should.
//! - macOS has no equivalent short of a watchdog process; there only the
//!   caller's own cleanup (`Drop`) stops the child.

use std::io;
use std::process::{Child, Command};

/// `cmd.spawn()`, with the child tied to this process as described above.
/// If tying fails after the child started (Windows), the child is still
/// returned and `warn` is told why: the caller's own cleanup still applies.
pub fn spawn(cmd: &mut Command, warn: impl FnOnce(String)) -> io::Result<Child> {
    #[cfg(target_os = "linux")]
    linux::kill_with_parent(cmd);
    let child = cmd.spawn()?;
    #[cfg(windows)]
    if let Err(e) = windows::assign(&child) {
        warn(format!(
            "Cannot tie process {} to hanstat ({e}): it may outlive hanstat if hanstat is killed",
            child.id()
        ));
    }
    #[cfg(not(windows))]
    let _ = warn;
    Ok(child)
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// The process's one job, created on first use and never closed: the
    /// OS closes it when the process ends, which is the point. Stored as
    /// an address since a raw `HANDLE` is not `Sync`.
    static JOB: OnceLock<Result<usize, String>> = OnceLock::new();

    fn job() -> Result<isize, String> {
        JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!("CreateJobObjectW: {}", io::Error::last_os_error()));
            }
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                let e = io::Error::last_os_error();
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(format!("SetInformationJobObject: {e}"));
            }
            Ok(job as usize)
        })
        .clone()
        .map(|h| h as isize)
    }

    pub fn assign(child: &Child) -> Result<(), String> {
        let job = job()? as *mut core::ffi::c_void;
        // A child already in another job (hanstat's own, if it was started
        // in one) can still be added: jobs nest since Windows 8.
        if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
            return Err(format!(
                "AssignProcessToJobObject: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub fn kill_with_parent(cmd: &mut Command) {
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // hanstat died between fork and prctl: the signal will not come.
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    /// The child is killed when the job's last handle closes, which a test
    /// cannot do to its own process; this checks the job is made and taken.
    #[cfg(windows)]
    #[test]
    fn child_joins_the_job() {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
        let mut warned = None;
        let mut child = super::spawn(&mut cmd, |w| warned = Some(w)).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(warned, None);
    }
}
