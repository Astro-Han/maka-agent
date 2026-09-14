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

use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::windows::named_pipe::NamedPipeServer,
};

pub(in crate::server) struct LocalStream(pub(super) NamedPipeServer);

impl AsyncRead for LocalStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for LocalStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Tokio's pipe flush is a no-op, while Mio may still own a queued write.
        // A zero-byte write checks Mio's previous-write completion/error first.
        // try_write also clears stale readiness on WouldBlock; readiness alone
        // does not establish completion. Byte-mode pipes (fixed at creation)
        // do not deliver zero-byte writes as messages or EOF to readers.
        loop {
            ready!(self.0.poll_write_ready(cx))?;
            match self.0.try_write(&[]) {
                Ok(_) => return Poll::Ready(Ok(())),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

#[cfg(test)]
mod tests {
    use crate::server::local::LocalListener;
    use std::{path::PathBuf, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::windows::named_pipe::ClientOptions,
    };

    #[tokio::test]
    async fn flush_waits_for_pending_bytes_and_reports_a_disconnected_reader() {
        let path = PathBuf::from(format!(
            r"\\.\pipe\maka-flush-test-{}",
            uuid::Uuid::new_v4()
        ));
        let mut listener = LocalListener::bind(&path).unwrap();
        let payload = vec![b'x'; 512 * 1024];
        let mut client = ClientOptions::new().open(&path).unwrap();
        let mut stream = listener.accept().await.unwrap();
        stream.write_all(&payload).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.flush())
                .await
                .is_err()
        );
        let read = tokio::spawn(async move {
            let mut actual = vec![0; 512 * 1024];
            client.read_exact(&mut actual).await.unwrap();
            (client, actual)
        });
        tokio::time::timeout(Duration::from_secs(5), stream.flush())
            .await
            .unwrap()
            .unwrap();
        let (mut client, actual) = tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual, payload);
        stream.write_all(b"next").await.unwrap();
        stream.flush().await.unwrap();
        let mut next = [0; 4];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut next))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            &next, b"next",
            "flush must not deliver an empty message or EOF"
        );
        drop(client);
        drop(stream);

        let client = ClientOptions::new().open(&path).unwrap();
        let mut stream = listener.accept().await.unwrap();
        stream.write_all(&payload).await.unwrap();
        drop(client);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), stream.flush())
                .await
                .unwrap()
                .is_err()
        );
    }
}
