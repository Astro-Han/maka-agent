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

use super::FrameError;
use base64::{Engine, engine::general_purpose::STANDARD};
use maka_protocol::capability::{MAX_RESULT_BYTES, RESULT_CHUNK_MAX_BYTES, decode_result};
use maka_runtime::capability::CallResult;

pub(super) struct Chunks {
    length: usize,
    count: u64,
    next: u64,
    bytes: Vec<u8>,
}
impl Chunks {
    pub fn new(length: u64, count: u64) -> Result<Self, FrameError> {
        if length == 0
            || length > MAX_RESULT_BYTES
            || count != length.div_ceil(RESULT_CHUNK_MAX_BYTES)
        {
            return Err(FrameError("invalid chunk declaration"));
        }
        // Allocate only as actual validated chunks arrive, not from a promise.
        Ok(Self {
            length: length as usize,
            count,
            next: 0,
            bytes: Vec::new(),
        })
    }
    pub fn push(&mut self, index: u64, data: &str) -> Result<Option<CallResult>, FrameError> {
        if index != self.next || index >= self.count {
            return Err(FrameError("chunk out of sequence"));
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| FrameError("invalid chunk encoding"))?;
        let expected = (self.length - self.bytes.len()).min(RESULT_CHUNK_MAX_BYTES as usize);
        if bytes.len() != expected {
            return Err(FrameError("invalid chunk length"));
        }
        self.bytes.extend(bytes);
        self.next += 1;
        if self.next != self.count {
            return Ok(None);
        }
        // Source Buffer.toString uses replacement decoding, not strict UTF-8.
        let json = serde_json::from_str(&String::from_utf8_lossy(&self.bytes))
            .map_err(|_| FrameError("chunked result is not JSON"))?;
        decode_result(&json)
            .map(Some)
            .map_err(|_| FrameError("invalid chunked result"))
    }
}
