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

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};

pub(super) struct Proxy {
    pub port: u16,
    pub requests: Arc<AtomicUsize>,
    pub closed: Arc<AtomicUsize>,
    worker: JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.worker.abort();
    }
}
impl Proxy {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let received = requests.clone();
        let disconnected = closed.clone();
        let worker = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let requests = received.clone();
                        let closed = disconnected.clone();
                        connections.spawn(async move {
                            let mut header = Vec::new();
                            while !header.ends_with(b"\r\n\r\n") {
                                header.push(socket.read_u8().await.unwrap());
                                assert!(header.len() < 65536);
                            }
                            let header = String::from_utf8(header).unwrap();
                            let mut first = header.lines().next().unwrap().split_whitespace();
                            let method = first.next().unwrap();
                            let target = first.next().unwrap();
                            assert!(target.starts_with("http://maka-http.invalid/"), "{target}");
                            requests.fetch_add(1, Ordering::SeqCst);
                            if target.ends_with("/upload") {
                                assert_eq!(method, "POST");
                                let mut body = vec![0; 1024 * 1024];
                                socket.read_exact(&mut body).await.unwrap();
                                assert!(body.iter().all(|byte| *byte == 255));
                                socket.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
                            } else if target.ends_with("/stream") {
                                assert_eq!(method, "POST");
                                let mut body = vec![0; "request 測試🦀".len()];
                                socket.read_exact(&mut body).await.unwrap();
                                assert_eq!(body, "request 測試🦀".as_bytes());
                                let body = "測試🦀".repeat(5000);
                                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Repeat: one\r\nX-Repeat: two\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                                socket.write_all(body.as_bytes()).await.unwrap();
                            } else if target.ends_with("/redirect") {
                                assert_eq!(method, "POST");
                                let mut body = [0; 4];
                                socket.read_exact(&mut body).await.unwrap();
                                assert_eq!(&body, b"once");
                                socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://maka-http.invalid/unexpected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                            } else {
                                assert!(target.ends_with("/truncated") || target.ends_with("/blocked"));
                                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nprefix").await.unwrap();
                                if target.ends_with("/blocked") {
                                    let mut byte = [0];
                                    let read = socket.read(&mut byte).await;
                                    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
                                    closed.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            port,
            requests,
            closed,
            worker,
        }
    }
}
