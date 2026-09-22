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

use super::error::ModelError;
use serde::Serialize;
use std::io;

/// Count serialized bytes without making another full copy of model/image data.
/// Stop serialization at the boundary instead of allocating then rejecting.
pub fn bytes(value: &impl Serialize, maximum: u32) -> Result<u32, ModelError> {
    struct Counter {
        used: u32,
        maximum: u32,
    }
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = u32::try_from(bytes.len()).map_err(io::Error::other)?;
            self.used = self
                .used
                .checked_add(count)
                .filter(|used| *used <= self.maximum)
                .ok_or_else(|| io::Error::other("serialized value exceeds byte budget"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { used: 0, maximum };
    serde_json::to_writer(&mut counter, value)
        .map_err(|error| ModelError::Adapter(error.to_string()))?;
    Ok(counter.used.max(1))
}
