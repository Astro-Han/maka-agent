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

use crate::{Result, TransportError, WriteGuard, decode, invalid, too_large};
use maka_protocol::{MAX_MESSAGE_BYTES, encode_message};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf,
};
use tokio_util::sync::CancellationToken;

pub struct NdjsonReader<R> {
    stream: BufReader<R>,
    pending: Vec<u8>,
    ended: bool,
    cancel: CancellationToken,
}
pub struct NdjsonWriter<W> {
    stream: W,
    closed: bool,
    cancel: CancellationToken,
}

/// The token aborts both directions. Clean read EOF preserves the write side.
pub fn split<S: AsyncRead + AsyncWrite>(
    stream: S,
    cancel: CancellationToken,
) -> (NdjsonReader<ReadHalf<S>>, NdjsonWriter<WriteHalf<S>>) {
    let (read, write) = tokio::io::split(stream);
    (
        NdjsonReader {
            stream: BufReader::new(read),
            pending: Vec::new(),
            ended: false,
            cancel: cancel.clone(),
        },
        NdjsonWriter {
            stream: write,
            closed: false,
            cancel,
        },
    )
}

impl<R: AsyncRead + Unpin> NdjsonReader<R> {
    /// Cancellation-safe for caller select/timeout: consumed bytes remain owned
    /// by this reader. Explicit token cancellation is terminal.
    pub async fn read(&mut self) -> Result<Option<Value>> {
        let token = self.cancel.clone();
        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(TransportError::Closed),
            result = self.read_next() => result,
        };
        if result.is_err() {
            token.cancel();
        }
        result
    }

    async fn read_next(&mut self) -> Result<Option<Value>> {
        if self.ended {
            return Ok(None);
        }
        loop {
            let bytes = self.stream.fill_buf().await?;
            if bytes.is_empty() {
                self.ended = true;
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Err(invalid("Runtime Host stream ended with a partial frame"))
                };
            }
            let newline = bytes.iter().position(|b| *b == b'\n');
            let end = newline.unwrap_or(bytes.len());
            if self.pending.len() + end > MAX_MESSAGE_BYTES {
                return Err(too_large());
            }
            self.pending.extend_from_slice(&bytes[..end]);
            self.stream.consume(end + usize::from(newline.is_some()));
            if newline.is_some() {
                let bytes = std::mem::take(&mut self.pending);
                if bytes.is_empty() {
                    return Err(invalid("Runtime Host frame is empty"));
                }
                let trimmed = bytes.strip_suffix(b"\r").unwrap_or(&bytes);
                // A CR-only line reaches JSON parsing in the original decoder.
                let value = if trimmed.is_empty() {
                    maka_protocol::decode_message(trimmed)?
                } else {
                    decode(trimmed)?
                };
                return Ok(Some(value));
            }
        }
    }
}

impl<W: AsyncWrite + Unpin> NdjsonWriter<W> {
    /// Writes are serialized by &mut self and complete only after flush.
    /// Cancelling a started write poisons the connection (partial outcome).
    pub async fn write(&mut self, value: &impl Serialize) -> Result<()> {
        if self.closed || self.cancel.is_cancelled() {
            return Err(TransportError::Closed);
        }
        let mut bytes = encode_message(value)?;
        bytes.push(b'\n');
        let guard = WriteGuard::new(&self.cancel);
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(TransportError::Closed),
            result = async { self.stream.write_all(&bytes).await?; self.stream.flush().await } => result?,
        }
        guard.complete();
        Ok(())
    }

    /// Complete all prior writes, then half-close. Read-side EOF is independent.
    pub async fn close_after_flush(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let guard = WriteGuard::new(&self.cancel);
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(TransportError::Closed),
            result = async { self.stream.flush().await?; self.stream.shutdown().await } => result?,
        }
        self.closed = true;
        guard.complete();
        Ok(())
    }
}
