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

use super::Spawned;
use crate::{
    Command,
    windows::{attributes::Attributes, checked, job::Job, owned, pipe, wait_process},
};
use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, OwnedHandle},
    process::ExitStatus,
    ptr,
    time::Duration,
};
use windows_sys::Win32::System::Threading::*;

pub struct Child {
    process: OwnedHandle,
    job: Job,
    pid: u32,
    status: Option<ExitStatus>,
    cleaned: bool,
}
pub async fn spawn(plan: Command) -> io::Result<Spawned> {
    let mut buffers = plan.windows()?;
    let (stdin, stdin_peer) = pipe::input().await?;
    let (stdout, stdout_peer) = pipe::output().await?;
    let (stderr, stderr_peer) = pipe::output().await?;
    let job = Job::new()?;
    let jobs = [job.0.as_raw_handle()];
    let handles = [
        stdin_peer.as_raw_handle(),
        stdout_peer.as_raw_handle(),
        stderr_peer.as_raw_handle(),
    ];
    let mut attributes = Attributes::stdio(&handles, &jobs)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.as_ptr();
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: buffers are terminated and live; attribute lists bind the Job
    // and exact pipe handles before any child code executes.
    unsafe {
        checked(CreateProcessW(
            buffers.executable.as_ptr(),
            buffers.line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            buffers.environment.as_ptr().cast(),
            buffers.cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        ))?;
    }
    let process = owned(info.hProcess)?;
    let _thread = owned(info.hThread)?;
    Ok(Spawned {
        stdin,
        stdout,
        stderr,
        child: Child {
            process,
            job,
            pid: info.dwProcessId,
            status: None,
            cleaned: false,
        },
    })
}
impl Child {
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn terminate(&mut self) -> io::Result<()> {
        self.job.terminate(130)
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        if self.status.is_none() {
            self.status = Some(wait_process(&self.process).await?);
        }
        if !self.cleaned {
            self.terminate()?;
            tokio::time::timeout(Duration::from_secs(2), async {
                while !self.job.is_empty()? {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok::<_, io::Error>(())
            })
            .await
            .map_err(|_| io::Error::other("process Job exit is unconfirmed"))??;
            self.cleaned = true;
        }
        Ok(self.status.expect("root exited"))
    }
}
// Job's kill-on-close is emergency cleanup. Only wait() confirms tree exit.
