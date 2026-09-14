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

use super::{checked, job::Job, owned, pipe};
use crate::shell::{ShellPlan, command};
use std::{
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, OwnedHandle},
    path::Path,
    ptr,
};
use tokio::net::windows::named_pipe::NamedPipeServer;
use windows_sys::Win32::{
    Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
    System::Threading::*,
};

pub(super) struct Child {
    pub process: OwnedHandle,
    pub job: Job,
}
pub(super) struct Spawned {
    pub child: Child,
    pub stdout: NamedPipeServer,
    pub stderr: NamedPipeServer,
}

struct Attributes(Vec<usize>);
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: only successfully initialized lists construct this owner.
        unsafe {
            DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
        }
    }
}

pub(super) async fn spawn(
    shell: &ShellPlan,
    cwd: &Path,
    source: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) -> io::Result<Spawned> {
    let mut command = command::prepare(shell, source)?;
    let cwd = command::wide(dunce::simplified(cwd))?;
    let (stdout, stdout_writer) = pipe::output().await?;
    let (stderr, stderr_writer) = pipe::output().await?;
    let stdin: File = OpenOptions::new().read(true).open("NUL")?;
    // SAFETY: owned NUL input, explicitly included in the inheritance whitelist.
    unsafe {
        checked(SetHandleInformation(
            stdin.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        ))?;
    }
    let job = Job::new()?;
    let jobs = [job.0.as_raw_handle()];
    let handles = [
        stdin.as_raw_handle(),
        stdout_writer.as_raw_handle(),
        stderr_writer.as_raw_handle(),
    ];
    let mut bytes = 0;
    // SAFETY: documented size query, without a supplied list.
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut bytes);
    }
    if bytes == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    // SAFETY: usize storage provides sufficient pointer alignment and size.
    unsafe {
        checked(InitializeProcThreadAttributeList(
            storage.as_mut_ptr().cast(),
            2,
            0,
            &mut bytes,
        ))?;
    }
    let mut attributes = Attributes(storage);
    // SAFETY: both arrays outlive the list and CreateProcess call. Only the three
    // stdio handles can be inherited; the Job itself is not inheritable.
    unsafe {
        checked(UpdateProcThreadAttribute(
            attributes.0.as_mut_ptr().cast(),
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            jobs.as_ptr().cast(),
            size_of_val(&jobs),
            ptr::null_mut(),
            ptr::null(),
        ))?;
        checked(UpdateProcThreadAttribute(
            attributes.0.as_mut_ptr().cast(),
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of_val(&handles),
            ptr::null_mut(),
            ptr::null(),
        ))?;
    }
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    let mut process = PROCESS_INFORMATION::default();
    if cancellation.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Shell cancelled before spawn",
        ));
    }
    // SAFETY: all UTF-16 buffers are terminated and live, the command line is
    // writable, and the two valid attributes bind the Job before any user code.
    unsafe {
        checked(CreateProcessW(
            command.executable.as_ptr(),
            command.line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            command.environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        ))?;
    }
    let process_handle = owned(process.hProcess)?;
    let _thread = owned(process.hThread)?;
    // Parent copies of write ends close here; EOF belongs to the child tree.
    Ok(Spawned {
        child: Child {
            process: process_handle,
            job,
        },
        stdout,
        stderr,
    })
}
