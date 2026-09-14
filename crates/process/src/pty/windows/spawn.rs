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

use super::{Console, PtyCommand};
use crate::{
    shell::command::{quote, wide},
    windows::{checked, job::Job, owned},
};
use std::{
    collections::BTreeMap,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::System::Threading::*;

struct Attributes(Vec<usize>);
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
        }
    }
}

pub(super) fn launch(
    plan: &PtyCommand,
    console: &Console,
    job: &Job,
) -> io::Result<(OwnedHandle, u32)> {
    if !plan.executable.is_absolute() || !plan.cwd.is_absolute() {
        return Err(io::Error::other(
            "ConPTY requires captured absolute executable and cwd",
        ));
    }
    let executable = wide(dunce::simplified(&plan.executable))?;
    let cwd = wide(dunce::simplified(&plan.cwd))?;
    let executable_arg = dunce::simplified(&plan.executable)
        .to_str()
        // cmd.exe scans slash switches even inside argv[0]. Windows accepts
        // mixed path separators, but its command parser requires backslashes.
        .map(|path| quote(&path.replace('/', "\\")))
        .ok_or_else(|| io::Error::other("PTY executable must be UTF-8"))?;
    let arguments = std::iter::once(Ok(executable_arg))
        .chain(plan.args.iter().map(super::super::Argument::command_line))
        .collect::<io::Result<Vec<_>>>()?
        .join(" ");
    let mut line = wide(Path::new(&arguments))?;
    let mut sorted = BTreeMap::new();
    for (key, value) in &plan.environment {
        if key.is_empty()
            || key.encode_wide().any(|c| c == 0)
            || value.encode_wide().any(|c| c == 0)
        {
            return Err(io::Error::other("invalid PTY environment"));
        }
        sorted.insert(key.to_string_lossy().to_uppercase(), (key, value));
    }
    let mut environment = Vec::new();
    for (key, value) in sorted.into_values() {
        environment.extend(key.encode_wide());
        environment.push('=' as u16);
        environment.extend(value.encode_wide());
        environment.push(0);
    }
    environment.push(0);
    let jobs = [job.0.as_raw_handle()];
    let mut bytes = 0;
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut bytes);
    }
    if bytes == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    unsafe {
        checked(InitializeProcThreadAttributeList(
            storage.as_mut_ptr().cast(),
            2,
            0,
            &mut bytes,
        ))?;
    }
    let mut attributes = Attributes(storage);
    // SAFETY: the Job array and HPCON live through CreateProcess; the console
    // attribute takes the handle value itself, unlike the Job-list pointer.
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
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            console.0 as *const _,
            size_of_val(&console.0),
            ptr::null_mut(),
            ptr::null(),
        ))?;
    }
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // Null standard handles must be explicit: otherwise Windows can duplicate
    // the host's redirected stdio even with bInheritHandles=false, bypassing PTY.
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    let mut process = PROCESS_INFORMATION::default();
    // No standard handle inheritance: ConPTY supplies the child's console.
    unsafe {
        checked(CreateProcessW(
            executable.as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        ))?;
    }
    let handle = owned(process.hProcess)?;
    let _thread = owned(process.hThread)?;
    Ok((handle, process.dwProcessId))
}
