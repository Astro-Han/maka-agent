/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

//! Native PTY handles. The resource owner supplies a captured command, persists
//! startup before calling spawn, and drains output before publishing its exit fence.
//! These handles do not create a second resource owner or perform recovery.

use super::{PtyCommand, stream};
use maka_runtime::terminal::TerminalSize;
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags},
    process::{Pid, Signal},
};
use std::{
    io,
    process::{ExitStatus, Stdio},
    sync::Arc,
};
use tokio::{io::unix::AsyncFd, process::Child};

pub use stream::PtyIo;

/// The unreaped root pins the root process/group identity.
/// Normal exit preserves descendants. Explicit termination also signals the
/// current foreground job; escaped/background process-tree policy belongs to
/// the resource lifecycle, not this OS handle.
pub struct PtyChild {
    child: Child,
    pid: Pid,
    master: Arc<AsyncFd<OwnedFd>>,
    status: Option<ExitStatus>,
}

pub async fn spawn(plan: PtyCommand, size: TerminalSize) -> io::Result<(PtyChild, PtyIo)> {
    let mut command = std::process::Command::new(plan.executable);
    command
        .args(plan.args)
        .current_dir(plan.cwd)
        .env_clear()
        .envs(plan.environment);
    // Open the multiplexor with CLOEXEC atomically: openpty + a later fcntl
    // leaves a descriptor-inheritance window when other sessions spawn.
    let master = rustix::fs::open(
        c"/dev/ptmx",
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    #[cfg(target_os = "linux")]
    let slave = rustix::pty::ioctl_tiocgptpeer(
        &master,
        rustix::pty::OpenptFlags::RDWR
            | rustix::pty::OpenptFlags::NOCTTY
            | rustix::pty::OpenptFlags::CLOEXEC,
    )?;
    #[cfg(target_os = "macos")]
    let slave = rustix::fs::open(
        rustix::pty::ptsname(&master, Vec::new())?,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    stream::resize(&master, size)?;
    let flags = rustix::fs::fcntl_getfl(&master)?;
    rustix::fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK)?;
    let master = Arc::new(AsyncFd::new(master)?);
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    // SAFETY: pre_exec performs only async-signal-safe syscalls and creates no
    // allocations/locks. std has already installed the owned slave on fd 0..2.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            for signal in [
                libc::SIGCHLD,
                libc::SIGHUP,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGALRM,
                libc::SIGTSTP,
                libc::SIGTTIN,
                libc::SIGTTOU,
            ] {
                libc::signal(signal, libc::SIG_DFL);
            }
            let mut set = std::mem::zeroed();
            if libc::sigemptyset(&mut set) != 0
                || libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    // Drop the Command's parent copies of the slave before returning the master.
    let pid = Pid::from_raw(child.id().expect("spawned child has a pid") as i32)
        .expect("child pid is positive");
    Ok((
        PtyChild {
            child,
            pid,
            master: master.clone(),
            status: None,
        },
        PtyIo { master },
    ))
}

impl PtyChild {
    pub fn resize(&mut self, size: TerminalSize) -> io::Result<()> {
        stream::resize(self.master.get_ref(), size)
    }

    /// Unix has no separate console server. EOF/drain remains the caller's job.
    pub async fn close(&mut self) -> io::Result<()> {
        if self.status.is_none() {
            return Err(io::Error::other("wait for the PTY root before closing"));
        }
        Ok(())
    }

    pub fn id(&self) -> u32 {
        self.pid.as_raw_nonzero().get() as u32
    }

    /// Cancellation-safe root wait; output may still remain in the master.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let status = self.child.wait().await?;
        self.status = Some(status);
        Ok(status)
    }

    /// Signal while the root remains unreaped. Foreground group signalling is
    /// best-effort: a numeric tcgetpgrp snapshot is not a stable process handle.
    /// Caller must still wait and drain before treating termination as complete.
    pub fn terminate(&mut self) -> io::Result<bool> {
        if self.status.is_some() {
            return Ok(false);
        }
        let mut foreground_result = Ok(false);
        if let Ok(group) = rustix::termios::tcgetpgrp(self.master.get_ref())
            && group != self.pid
            && rustix::termios::tcgetsid(self.master.get_ref()) == Ok(self.pid)
        {
            // Only use the foreground snapshot while the terminal still belongs
            // to our session. This is not an atomic group-identity guarantee.
            foreground_result = signal(group);
        }
        // A privileged foreground job may reject our signal; that must not
        // prevent cleanup of the root that we can still terminate.
        let root_result = signal(self.pid);
        Ok(foreground_result? | root_result?)
    }
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        // Emergency cleanup only. Normal resource shutdown calls terminate,
        // awaits wait(), and drains the master so it can persist real evidence.
        let _ = self.terminate();
        // Tokio's kill-on-drop / orphan reaper owns the unreaped root after drop.
    }
}

fn signal(group: Pid) -> io::Result<bool> {
    match rustix::process::kill_process_group(group, Signal::KILL) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
