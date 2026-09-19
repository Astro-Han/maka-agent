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
use super::{ImportSourceInput, ImportSourceResult, invalid, text};
use crate::Result;
use serde_json::Value;

pub fn decode_import_input(value: &Value) -> Result<ImportSourceInput> {
    let input: ImportSourceInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    text(&input.source_path, 4096)?;
    if !crate::codec::absolute_host_path(&input.source_path) {
        return Err(invalid("Invalid Skill import path"));
    }
    Ok(input)
}
pub fn decode_import_output(value: &Value) -> Result<ImportSourceResult> {
    let result: ImportSourceResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    if let ImportSourceResult::Imported { source } = &result {
        if !maka_skills::safe_source_id(&source.id) {
            return Err(invalid("Invalid imported Skill identity"));
        }
        text(&source.name, 256)?;
        text(&source.description, 4096)?;
        text(&source.category, 128)?;
    }
    Ok(result)
}
