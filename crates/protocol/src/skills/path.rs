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
use super::{ResolvePathInput, ResolvePathResult, invalid, text};
use crate::Result;
use serde_json::Value;

pub fn decode_path_input(value: &Value) -> Result<ResolvePathInput> {
    let input: ResolvePathInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    super::validate_workspace(&input.context.workspace)?;
    text(&input.reference, 512)?;
    Ok(input)
}
pub fn decode_path_output(value: &Value) -> Result<ResolvePathResult> {
    let result: ResolvePathResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    if let ResolvePathResult::Resolved { path, .. } = &result {
        text(path, 4096)?;
        if !crate::codec::absolute_host_path(path) {
            return Err(invalid("Invalid Skill path"));
        }
    }
    Ok(result)
}
