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

use super::{GovernanceItem, invalid, text};
use crate::Result;

pub(super) fn validate(item: &GovernanceItem) -> Result<()> {
    identity(&item.reference, 512)?;
    identity(&item.id, 256)?;
    if let Some(path) = &item.path {
        text(path, 4096)?;
        if !crate::codec::absolute_host_path(path) {
            return Err(invalid("Invalid Skill display path"));
        }
    }
    if let Some(reference) = &item.shadowed_by {
        identity(reference, 512)?;
    }
    if item.name.len() > 256
        || item.description.len() > 4096
        || item.declared_tools.len() > 64
        || item.validation_codes.len() > 64
        || item
            .context_rank
            .is_some_and(|rank| !(1..=9_007_199_254_740_991).contains(&rank))
    {
        return Err(invalid("Invalid Skill governance projection"));
    }
    for tool in &item.declared_tools {
        text(tool, 256)?;
    }
    Ok(())
}
fn identity(value: &str, limit: usize) -> Result<()> {
    text(value, limit)?;
    if value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(invalid("Invalid Skill identity"));
    }
    Ok(())
}
