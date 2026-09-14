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

use super::*;
use std::io::{self, Write};
use tokio::sync::OwnedSemaphorePermit;

pub(super) struct Overlay {
    pub rows: Vec<Vec<u8>>,
    _permit: OwnedSemaphorePermit,
}
impl Overlay {
    pub fn new(messages: Vec<Message>, budget: Arc<Semaphore>) -> Result<Self> {
        if messages.len() as u64 > SESSION_TRANSCRIPT_OVERLAY_MAX_MESSAGES {
            return Err(TranscriptError::Capacity);
        }
        // Count with a capped sink before allocating payload buffers or reserving
        // shared capacity. This also bounds transient serialization memory.
        let mut counter = Counter(0);
        for message in &messages {
            serde_json::to_writer(&mut counter, message).map_err(|_| TranscriptError::Capacity)?;
        }
        let permit = budget
            .try_acquire_many_owned(counter.0 as u32)
            .map_err(|_| TranscriptError::Capacity)?;
        let rows = messages
            .iter()
            .map(serde_json::to_vec)
            .collect::<std::result::Result<_, _>>()?;
        Ok(Self {
            rows,
            _permit: permit,
        })
    }
}
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let size = self
            .0
            .checked_add(bytes.len())
            .filter(|size| *size <= 16 * 1024 * 1024)
            .ok_or_else(|| io::Error::other("overlay exceeds capacity"))?;
        self.0 = size;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
