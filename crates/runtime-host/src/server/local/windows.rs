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

use super::HostError;
use maka_event_log::root::windows::PrivateSecurity;
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};

mod stream;
use stream::LocalStream;

/// The listening instance stays alive until the replacement exists, so no
/// other account can claim the name between accepted connections.
pub struct LocalListener {
    listener: NamedPipeServer,
    path: PathBuf,
}

impl LocalListener {
    pub fn bind(path: &Path) -> Result<Self, HostError> {
        let name = path.to_str().ok_or("pipe path must be UTF-8")?;
        let suffix = name
            .strip_prefix(r"\\.\pipe\")
            .ok_or("local pipe path must start with \\\\.\\pipe\\")?;
        if suffix.is_empty() || suffix.contains(['\\', '/', '\0']) {
            return Err("local pipe name must be a single nonempty component".into());
        }
        Ok(Self {
            listener: create(path, true)?,
            path: path.to_owned(),
        })
    }

    pub(in crate::server) async fn accept(&mut self) -> io::Result<LocalStream> {
        // Tokio connect is cancellation-safe: select may retry this same instance.
        self.listener.connect().await?;
        let next = create(&self.path, false)?;
        Ok(LocalStream(std::mem::replace(&mut self.listener, next)))
    }
}

fn create(path: &Path, first: bool) -> io::Result<NamedPipeServer> {
    let security = PrivateSecurity::current_account()?;
    let mut attributes = security.attributes();
    // SAFETY: the descriptor and attributes live throughout synchronous creation.
    // The handle is non-inheritable and its ACL is private before it is visible.
    unsafe {
        ServerOptions::new()
            .pipe_mode(PipeMode::Byte)
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(path, std::ptr::from_mut(&mut attributes).cast())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        os::windows::io::{AsRawHandle, BorrowedHandle},
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::windows::named_pipe::ClientOptions,
    };

    #[tokio::test]
    async fn private_pipe_survives_cancelled_accept_and_releases_its_name() {
        let path = PathBuf::from(format!(r"\\.\pipe\maka-pipe-test-{}", uuid::Uuid::new_v4()));
        let mut listener = LocalListener::bind(&path).unwrap();
        assert!(LocalListener::bind(&path).is_err());
        {
            // SAFETY: listener owns this handle throughout duplication.
            let handle = unsafe { BorrowedHandle::borrow_raw(listener.listener.as_raw_handle()) };
            let duplicate = File::from(handle.try_clone_to_owned().unwrap());
            maka_event_log::root::windows::validate_private(&duplicate).unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(10), listener.accept())
                .await
                .is_err()
        );
        for message in [b"one", b"two"] {
            let mut client = ClientOptions::new().open(&path).unwrap();
            let mut server = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            client.write_all(message).await.unwrap();
            let mut actual = [0; 3];
            tokio::time::timeout(Duration::from_secs(2), server.read_exact(&mut actual))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&actual, message);
        }
        drop(listener);
        assert!(ClientOptions::new().open(&path).is_err());
        // Dropped Mio pipes retain their handles until cancelled read/connect
        // completions are collected by IOCP. Verify eventual reclamation.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match LocalListener::bind(&path) {
                    Ok(rebound) => break drop(rebound),
                    Err(error) => {
                        let error = error.downcast_ref::<io::Error>().unwrap();
                        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
            }
        })
        .await
        .unwrap();
    }
}
