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

use super::error;
use deno_error::JsErrorBox;
use futures_util::{SinkExt, StreamExt};
use maka_network::Socket;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

pub(super) enum ReadError {
    Cancelled,
    Incomplete,
    Transport(&'static str),
}

impl ReadError {
    pub fn into_js(self) -> JsErrorBox {
        error(match self {
            Self::Cancelled => "Responses WebSocket cancelled",
            Self::Incomplete => "Responses WebSocket response incomplete",
            Self::Transport(message) => message,
        })
    }
}

pub(super) async fn receive(
    socket: &mut Socket,
) -> std::result::Result<(String, bool, bool, Value), ReadError> {
    use ReadError::Transport;
    loop {
        let frame = socket
            .next()
            .await
            .ok_or(Transport("Responses WebSocket ended before completion"))?
            .map_err(|_| Transport("Responses WebSocket frame failed or exceeded 8 MiB"))?;
        let Message::Text(text) = frame else {
            match frame {
                Message::Ping(_) => {
                    socket
                        .flush()
                        .await
                        .map_err(|_| Transport("Responses WebSocket ping failed"))?;
                    continue;
                }
                Message::Pong(_) => continue,
                _ => return Err(Transport("Responses WebSocket expected a JSON text frame")),
            }
        };
        let event: Value = serde_json::from_str(&text)
            .map_err(|_| Transport("Responses WebSocket returned invalid JSON"))?;
        let kind = event["type"]
            .as_str()
            .ok_or(Transport("Responses WebSocket event has no type"))?;
        let reusable = matches!(kind, "response.completed" | "response.done")
            || (kind == "response.incomplete"
                && event["response"]["incomplete_details"]["reason"] == "max_output_tokens");
        let terminal =
            reusable || matches!(kind, "response.incomplete" | "response.failed" | "error");
        if kind == "response.incomplete" && !reusable {
            return Err(ReadError::Incomplete);
        }
        // JSON whitespace may contain newlines, which must not become SSE
        // record boundaries when the frame crosses the SDK fetch bridge.
        let text = event.to_string();
        return Ok((text, terminal, reusable, event));
    }
}
