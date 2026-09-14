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

use crate::{ProtocolError, Result, codec};
use maka_runtime::configuration::{
    policy::network_test::{Input, Output},
    validation,
};
use serde_json::Value;

pub const ERRORS: &[crate::OperationErrorCode] = &[
    crate::OperationErrorCode::HostNotReady,
    crate::OperationErrorCode::HostDraining,
    crate::OperationErrorCode::OperationUnavailable,
    crate::OperationErrorCode::InvalidRequest,
    crate::OperationErrorCode::InternalFailure,
];

fn integer(record: &mut Value, key: &str, min: u64, max: u64) -> Result<()> {
    if let Some(v) = record.get_mut(key) {
        let n = codec::count(v, key)?;
        if !(min..=max).contains(&n) {
            return Err(ProtocolError::invalid("Invalid proxy diagnostic integer"));
        }
        *v = n.into();
    }
    Ok(())
}
pub fn decode_input(value: &Value) -> Result<Input> {
    let mut value = value.clone();
    integer(&mut value, "timeoutMs", 1, 30_000)?;
    if let Some(proxy) = value.get_mut("networkProxy") {
        integer(proxy, "port", 1, 65_535)?;
    }
    let mut input: Input = serde_json::from_value(value)
        .map_err(|_| ProtocolError::invalid("Invalid network proxy diagnostic input"))?;
    if let Some(proxy) = &input.network_proxy {
        validation::text(&proxy.host, 255, false).map_err(ProtocolError::invalid)?;
        validation::text(&proxy.username, 256, false).map_err(ProtocolError::invalid)?;
        for list in [&proxy.bypass_list, &proxy.auto_bypass_domains] {
            if list.len() > 256 {
                return Err(ProtocolError::invalid("Too many bypass patterns"));
            }
            for pattern in list {
                validation::text(pattern, 512, false).map_err(ProtocolError::invalid)?;
            }
        }
    }
    if let Some(raw) = &mut input.url {
        if raw.is_empty() || raw.len() > 2048 {
            return Err(ProtocolError::invalid("Proxy probe URL exceeds limit"));
        }
        let url =
            url::Url::parse(raw).map_err(|_| ProtocolError::invalid("Invalid proxy probe URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ProtocolError::invalid("Invalid proxy probe URL scheme"));
        }
        *raw = url.into();
    }
    Ok(input)
}
pub fn decode_output(value: &Value) -> Result<Output> {
    if serde_json::to_vec(value)
        .map_err(|_| ProtocolError::invalid("Invalid proxy diagnostic result"))?
        .len()
        > 8192
    {
        return Err(ProtocolError::invalid(
            "Proxy diagnostic result exceeds limit",
        ));
    }
    let mut normalized = value.clone();
    integer(&mut normalized, "latencyMs", 0, 300_000)?;
    integer(&mut normalized, "status", 100, 599)?;
    let output: Output = serde_json::from_value(normalized)
        .map_err(|_| ProtocolError::invalid("Invalid proxy diagnostic result"))?;
    for (text, limit) in [
        (&output.ip, 256),
        (&output.country_code, 16),
        (&output.country_flag, 32),
        (&output.error, 2048),
    ] {
        if text
            .as_ref()
            .is_some_and(|text| text.is_empty() || text.len() > limit)
        {
            return Err(ProtocolError::invalid(
                "Proxy diagnostic text exceeds limit",
            ));
        }
    }
    Ok(output)
}
