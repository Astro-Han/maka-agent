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

use super::Error;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    io::{Read, Write},
    path::{Component, Path},
};

pub(super) fn open_path(root: &Dir, path: &Path) -> std::io::Result<Dir> {
    let mut directory = root.try_clone()?;
    for part in path.components() {
        let Component::Normal(name) = part else {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        };
        directory = directory.open_dir_nofollow(name)?;
    }
    Ok(directory)
}
pub(super) fn child(root: &Dir, name: &str) -> std::io::Result<Dir> {
    match root.create_dir(name) {
        Ok(()) => sync(root)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    root.open_dir_nofollow(name)
}

pub(super) struct PublicationLock(std::fs::File);
impl Drop for PublicationLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(super) fn lock(data: &Dir) -> std::io::Result<PublicationLock> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = data.open_with("publication.lock", &options)?.into_std();
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "publication lock is not a regular file",
        ));
    }
    file.try_lock().map_err(std::io::Error::from)?;
    Ok(PublicationLock(file))
}
pub(super) fn read(root: &Dir, path: &Path, limit: usize) -> Result<(Vec<u8>, u32), Error> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = root.open_with(path, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(Error::Invalid(
            "Skill resource is not a bounded regular file".into(),
        ));
    }
    #[cfg(unix)]
    let mode = {
        use cap_std::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(windows)]
    let mode = 0o600;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::Invalid(
            "Skill resource grew beyond its limit".into(),
        ));
    }
    Ok((bytes, mode))
}
pub(super) fn write_new(root: &Dir, path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let mut file = root.open_with(path, OpenOptions::new().write(true).create_new(true))?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        file.set_permissions(cap_std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(windows)]
    let _ = mode;
    file.sync_all()?;
    sync(root)
}

#[cfg(unix)]
pub(super) fn sync(directory: &Dir) -> std::io::Result<()> {
    directory.try_clone()?.into_std_file().sync_all()
}
#[cfg(windows)]
pub(super) fn sync(_: &Dir) -> std::io::Result<()> {
    // Windows has no portable directory fsync. Files are flushed before the
    // write-through, no-clobber publication below.
    Ok(())
}

#[cfg(unix)]
pub(super) fn rename(source: &Dir, from: &str, target: &Dir, to: &str) -> std::io::Result<()> {
    rustix::fs::renameat_with(source, from, target, to, rustix::fs::RenameFlags::NOREPLACE)?;
    sync(target)?;
    sync(source)
}
#[cfg(windows)]
pub(super) fn rename(source: &Dir, from: &str, target: &Dir, to: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source = directory_path(source)?.join(from);
    let target = directory_path(target)?.join(to);
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // cap-std directory handles exclude FILE_SHARE_DELETE: parents stay pinned.
    // Omitting REPLACE_EXISTING preserves a concurrently created destination.
    // SAFETY: both terminated paths remain live for this synchronous call.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(windows)]
fn directory_path(directory: &Dir) -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buffer = vec![0u16; 512];
    loop {
        // SAFETY: a live directory handle and a writable buffer of the supplied size.
        let size = unsafe {
            GetFinalPathNameByHandleW(
                directory.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        };
        if size == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if (size as usize) < buffer.len() {
            return Ok(std::ffi::OsString::from_wide(&buffer[..size as usize]).into());
        }
        if size > 32768 {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        buffer.resize(size as usize + 1, 0);
    }
}
