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

use crate::{ProtocolError, Result};

pub(crate) fn project(value: &str, limit: usize) -> Result<String> {
    let mut result = String::new();
    for c in value.chars() {
        if matches!(c as u32, 0..=31 | 127..=159 | 0x61c | 0x200e..=0x200f | 0x2028..=0x202e | 0x2066..=0x2069)
        {
            result.push_str(&format!("\\u{{{:X}}}", c as u32));
        } else {
            result.push(c);
        }
    }
    if result.len() > limit {
        return Err(ProtocolError::invalid(
            "Projected display text exceeds byte limit",
        ));
    }
    Ok(result)
}
