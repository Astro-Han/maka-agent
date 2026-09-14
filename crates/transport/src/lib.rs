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

//! Pull-based message transports: one read and one flushed write at a time.
//! There are no user-space message queues or background tasks. Backpressure is
//! applied directly to the stream. Unlike the TS event adapters, these do not
//! prefetch 64 messages / 2 MiB or fail a WebSocket when such a queue overflows.
//! Dropping both halves releases the stream. Cancellation wakes pending I/O;
//! owners must then drop both halves to release their socket resources.
pub mod ndjson;
pub mod websocket;

use maka_protocol::{ErrorCode, ProtocolError, decode_message};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("Runtime Host transport closed")]
    Closed,
    #[error("Runtime Host transport I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Runtime Host WebSocket transport failed: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}
pub type Result<T> = std::result::Result<T, TransportError>;

/// Common host-dispatch boundary. &mut self enforces one pending read.
pub trait MessageReader: Send {
    fn read(&mut self) -> impl Future<Output = Result<Option<Value>>> + Send;
}

/// Writes complete after flush; dropping a started write cancels the connection.
pub trait MessageWriter: Send {
    fn write(&mut self, value: &Value) -> impl Future<Output = Result<()>> + Send;
    fn close_after_flush(&mut self) -> impl Future<Output = Result<()>> + Send;
}

impl<R: tokio::io::AsyncRead + Unpin + Send> MessageReader for ndjson::NdjsonReader<R> {
    fn read(&mut self) -> impl Future<Output = Result<Option<Value>>> + Send {
        self.read()
    }
}

impl<W: tokio::io::AsyncWrite + Unpin + Send> MessageWriter for ndjson::NdjsonWriter<W> {
    fn write(&mut self, value: &Value) -> impl Future<Output = Result<()>> + Send {
        self.write(value)
    }
    fn close_after_flush(&mut self) -> impl Future<Output = Result<()>> + Send {
        self.close_after_flush()
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> MessageReader
    for websocket::WebSocketReader<S>
{
    fn read(&mut self) -> impl Future<Output = Result<Option<Value>>> + Send {
        self.read()
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> MessageWriter
    for websocket::WebSocketWriter<S>
{
    fn write(&mut self, value: &Value) -> impl Future<Output = Result<()>> + Send {
        self.write(value)
    }
    fn close_after_flush(&mut self) -> impl Future<Output = Result<()>> + Send {
        self.close_after_flush()
    }
}

pub(crate) fn invalid(message: &str) -> TransportError {
    ProtocolError::invalid(message).into()
}

pub(crate) fn too_large() -> TransportError {
    ProtocolError {
        code: ErrorCode::FrameTooLarge,
        message: "Runtime Host message exceeds the byte limit".into(),
    }
    .into()
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() {
        return Err(invalid("Runtime Host frame is empty"));
    }
    // Node's non-streaming TextDecoder strips an initial UTF-8 BOM per message.
    Ok(decode_message(
        bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes),
    )?)
}

// A dropped write future may have emitted a partial message. Poison both halves
// immediately so a later write cannot append another frame to that prefix.
pub(crate) struct WriteGuard(Option<CancellationToken>);
impl WriteGuard {
    pub(crate) fn new(token: &CancellationToken) -> Self {
        Self(Some(token.clone()))
    }
    pub(crate) fn complete(mut self) {
        self.0 = None;
    }
}
impl Drop for WriteGuard {
    fn drop(&mut self) {
        if let Some(token) = &self.0 {
            token.cancel();
        }
    }
}
