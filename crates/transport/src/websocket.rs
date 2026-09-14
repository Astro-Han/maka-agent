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
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use maka_protocol::{ErrorCode, MAX_MESSAGE_BYTES, ProtocolError, encode_message};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

/// Pass this to the HTTP upgrade after authenticating the request. No listener
/// or unauthenticated upgrade is provided by this crate.
pub fn config() -> WebSocketConfig {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_MESSAGE_BYTES);
    config.max_frame_size = Some(MAX_MESSAGE_BYTES);
    config.max_write_buffer_size = 2 * 1024 * 1024;
    config
}

pub struct WebSocketReader<S> {
    stream: SplitStream<WebSocketStream<S>>,
    cancel: CancellationToken,
}
pub struct WebSocketWriter<S> {
    sink: SplitSink<WebSocketStream<S>, Message>,
    closed: bool,
    cancel: CancellationToken,
}

pub type WebSocketHalves<S> = (WebSocketReader<S>, WebSocketWriter<S>);

/// Accepts only a completed, caller-authenticated upgrade whose decoder already
/// enforces allocation bounds. Reassembly of fragmented messages is tungstenite's job.
pub fn split<S: AsyncRead + AsyncWrite + Unpin>(
    socket: WebSocketStream<S>,
    cancel: CancellationToken,
) -> Result<WebSocketHalves<S>> {
    let limits = socket.get_config();
    if limits
        .max_message_size
        .is_none_or(|n| n > MAX_MESSAGE_BYTES)
        || limits.max_frame_size.is_none_or(|n| n > MAX_MESSAGE_BYTES)
        || limits.max_write_buffer_size > 2 * 1024 * 1024
    {
        return Err(invalid(
            "WebSocket must use bounded Runtime Host configuration",
        ));
    }
    let (sink, stream) = socket.split();
    Ok((
        WebSocketReader {
            stream,
            cancel: cancel.clone(),
        },
        WebSocketWriter {
            sink,
            closed: false,
            cancel,
        },
    ))
}

impl<S: AsyncRead + AsyncWrite + Unpin> WebSocketReader<S> {
    /// Read cancellation preserves tungstenite's buffered state. Ping/pong and
    /// close handshakes are driven while this future is polled.
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
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    if text.len() > MAX_MESSAGE_BYTES {
                        return Err(too_large());
                    }
                    return decode(text.as_bytes()).map(Some);
                }
                Some(Ok(Message::Binary(_))) => {
                    return Err(invalid("Runtime Host WebSocket messages must be text"));
                }
                // Continue polling so tungstenite flushes its automatic pong/close reply.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Close(_))) => continue,
                Some(Ok(Message::Frame(_))) => {
                    return Err(invalid("Unexpected raw WebSocket frame"));
                }
                Some(Err(tungstenite::Error::Capacity(_))) => return Err(too_large()),
                Some(Err(tungstenite::Error::Utf8(message))) => {
                    return Err(ProtocolError {
                        code: ErrorCode::InvalidUtf8,
                        message,
                    }
                    .into());
                }
                Some(Err(error)) => return Err(error.into()),
                None => return Ok(None),
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> WebSocketWriter<S> {
    pub async fn write(&mut self, value: &impl Serialize) -> Result<()> {
        if self.closed || self.cancel.is_cancelled() {
            return Err(TransportError::Closed);
        }
        let bytes = encode_message(value)?;
        let text = String::from_utf8(bytes).expect("serde_json always writes valid UTF-8");
        let guard = WriteGuard::new(&self.cancel);
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(TransportError::Closed),
            result = self.sink.send(Message::Text(text.into())) => result?,
        }
        guard.complete();
        Ok(())
    }

    /// Flushes accepted writes and starts the WebSocket close handshake. Keep
    /// polling the reader (with a caller deadline) to observe peer closure.
    pub async fn close_after_flush(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let guard = WriteGuard::new(&self.cancel);
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(TransportError::Closed),
            result = async {
                self.sink.send(Message::Close(Some(tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                    reason: "".into(),
                }))).await?;
                self.sink.close().await
            } => result?,
        }
        self.closed = true;
        guard.complete();
        Ok(())
    }
}
