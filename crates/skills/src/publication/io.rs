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

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};

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
#[cfg(unix)]
pub(super) fn sync(directory: &Dir) -> std::io::Result<()> {
    directory.open(".")?.sync_all()
}
#[cfg(windows)]
pub(super) fn sync(_: &Dir) -> std::io::Result<()> {
    // Windows has no portable directory fsync. Files are flushed before the
    // write-through, no-clobber publication below.
    Ok(())
}
