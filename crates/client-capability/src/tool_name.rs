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

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Matches the existing model-facing MCP name, including its collision domain.
pub fn proxy_tool_name(server_id: &str, tool_name: &str) -> String {
    fn part(value: &str) -> String {
        let mut output = String::new();
        let mut invalid_run = false;
        for c in value.nfkd() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                output.push(c);
                invalid_run = false;
            } else if !invalid_run {
                output.push('_');
                invalid_run = true;
            }
        }
        let output = output.trim_matches('_');
        if output.is_empty() {
            "unnamed".into()
        } else {
            output.into()
        }
    }
    let raw = format!("mcp__{}__{}", part(server_id), part(tool_name));
    if raw.len() <= 64 {
        return raw;
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(format!("{server_id}\0{tool_name}").as_bytes())
    );
    format!("{}__{}", &raw[..52], &hash[..10])
}
