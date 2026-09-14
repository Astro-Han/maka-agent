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

//! Message admission and queue wire contracts, epoch 141.
//! These codecs do not register handlers or own admission state.
mod input;
mod output;
mod queue;

use crate::{ProtocolError, Result, turn};
pub use input::*;
pub use output::*;
pub use queue::*;
use serde::Serialize;
use std::collections::HashSet;

pub const MAX_ENTRIES: usize = 64;
pub const MAX_PROJECTION_BYTES: usize = 52 * 1024;
pub const MAX_RESULT_BYTES: usize = 56 * 1024;

fn ensure(valid: bool, message: &str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::invalid(message))
    }
}

fn epoch(value: &str) -> Result<()> {
    ensure(
        !value.is_empty() && value.encode_utf16().count() <= 128,
        "Invalid host epoch",
    )
}

fn identities(values: &[String]) -> Result<()> {
    ensure(values.len() <= MAX_ENTRIES, "Too many message identities")?;
    let mut seen = HashSet::new();
    for value in values {
        turn::entity(value)?;
        ensure(seen.insert(value), "Duplicate message identity")?;
    }
    Ok(())
}

fn encoded(value: &impl Serialize, limit: usize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| ProtocolError::invalid(e.to_string()))?;
    ensure(
        bytes.len() <= limit,
        "Message projection exceeds byte limit",
    )
}
