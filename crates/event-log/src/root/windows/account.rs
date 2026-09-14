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

use super::{LocalAllocation, checked};
use std::{
    ffi::OsString,
    io,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Security::{
        Authorization::ConvertSidToStringSidW, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
    UI::Shell::GetUserProfileDirectoryW,
};

pub(super) fn token() -> io::Result<OwnedHandle> {
    let mut handle = ptr::null_mut();
    // SAFETY: valid process pseudo-handle and writable handle output.
    checked(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) })?;
    // SAFETY: successful OpenProcessToken transfers a unique owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

pub(super) struct AccountSid(Vec<usize>);
impl AccountSid {
    pub(super) fn current() -> io::Result<Self> {
        let token = token()?;
        let mut size = 0;
        // SAFETY: documented size-query call; no output buffer is supplied.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut size,
            )
        };
        if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            || size < size_of::<TOKEN_USER>() as u32
            || size > 65536
        {
            return Err(io::Error::other("invalid Windows account token size"));
        }
        // usize storage guarantees the pointer alignment required by TOKEN_USER.
        let mut bytes = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        // SAFETY: buffer is aligned, bounded and has at least size bytes.
        checked(unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                bytes.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        })?;
        Ok(Self(bytes))
    }

    pub(super) fn as_ptr(&self) -> PSID {
        // SAFETY: successful TokenUser query initialized TOKEN_USER and its SID
        // within this buffer; moving the Vec does not move its allocation.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    pub(super) fn text(&self) -> io::Result<String> {
        let mut text = ptr::null_mut();
        // SAFETY: SID remains backed by self; API allocates the string output.
        checked(unsafe { ConvertSidToStringSidW(self.as_ptr(), &mut text) })?;
        let allocation = LocalAllocation(text.cast());
        let mut length = 0;
        // SAFETY: successful API output is a NUL-terminated UTF-16 SID string.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        let value = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .map_err(|_| io::Error::other("invalid account SID string"));
        drop(allocation);
        value
    }
}

/// Uses the OS account token, never HOME/USERPROFILE/APPDATA overrides.
pub fn account_home() -> io::Result<PathBuf> {
    let token = token()?;
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    // SAFETY: writable UTF-16 buffer and its exact capacity are supplied.
    checked(unsafe {
        GetUserProfileDirectoryW(token.as_raw_handle(), buffer.as_mut_ptr(), &mut size)
    })?;
    if size == 0 || size as usize > buffer.len() || buffer[size as usize - 1] != 0 {
        return Err(io::Error::other("invalid OS account profile path"));
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..size as usize - 1]));
    if !path.is_absolute() {
        return Err(io::Error::other("OS account profile must be absolute"));
    }
    Ok(path)
}
