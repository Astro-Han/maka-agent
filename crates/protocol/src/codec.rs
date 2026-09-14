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
use serde_json::{Map, Value, json};

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(crate) fn absolute_host_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let slash = |b: u8| b == b'/' || b == b'\\';
    if text.starts_with('/')
        || bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && slash(bytes[2])
    {
        return true;
    }
    if bytes.len() < 5 || !slash(bytes[0]) || !slash(bytes[1]) {
        return false;
    }
    let mut parts = text[2..].split(['/', '\\']);
    parts.next().is_some_and(|s| !s.is_empty()) && parts.next().is_some_and(|s| !s.is_empty())
}

pub fn record<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| ProtocolError::invalid(format!("Invalid {label}")))
}

pub fn shaped(record: &Map<String, Value>, required: &[&str], optional: &[&str]) -> Result<()> {
    if record
        .keys()
        .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(ProtocolError::invalid("Unknown record field"));
    }
    if required.iter().any(|key| !record.contains_key(*key)) {
        return Err(ProtocolError::invalid("Invalid record fields"));
    }
    Ok(())
}

pub fn exact(record: &Map<String, Value>, keys: &[&str]) -> Result<()> {
    shaped(record, keys, &[])
}

/// JavaScript String.length counts UTF-16 code units, not UTF-8 bytes or scalars.
pub fn string(value: &Value, label: &str, max_length: usize) -> Result<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.encode_utf16().count() <= max_length)
        .map(str::to_owned)
        .ok_or_else(|| ProtocolError::invalid(format!("Invalid {label}")))
}

/// Accept JSON 1.0 and 1e0 as well as 1, exactly as Number.isSafeInteger does.
pub fn count(value: &Value, label: &str) -> Result<u64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= MAX_SAFE_INTEGER as f64 && n.fract() == 0.0)
        .map(|n| n as u64)
        .ok_or_else(|| ProtocolError::invalid(format!("Invalid {label}")))
}

pub fn composition(value: Option<&Value>) -> Result<String> {
    let Some(value) = value else {
        return Ok(crate::COMPOSITION_ID.into());
    };
    let id = string(value, "compositionId", 128)?;
    let mut segments = id.split(['.', '-']);
    let first = segments.next().unwrap_or_default();
    if !first.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        || !first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || segments.any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return Err(ProtocolError::invalid("Invalid compositionId"));
    }
    Ok(id)
}

pub fn epoch(value: Option<&Value>) -> Result<u64> {
    let epoch = value
        .map(|v| count(v, "compatibilityEpoch"))
        .transpose()?
        .unwrap_or(0);
    if epoch > 1_000_000 {
        return Err(ProtocolError::invalid("Invalid compatibilityEpoch"));
    }
    Ok(epoch)
}

pub(crate) fn decode_activity(value: &Value) -> Result<Value> {
    let fields = [
        "connections",
        "activeOperations",
        "processUptimeSeconds",
        "residencies",
    ];
    let frame = record(value, "Runtime Host activity")?;
    shaped(frame, &fields, &["drainResidencies", "cooperativeHandoff"])?;
    let mut result = Map::new();
    for field in &fields[..3] {
        result.insert((*field).into(), json!(count(&frame[*field], field)?));
    }
    if let Some(v) = frame.get("drainResidencies") {
        result.insert(
            "drainResidencies".into(),
            json!(count(v, "drainResidencies")?),
        );
    }
    if let Some(v) = frame.get("cooperativeHandoff") {
        if !v.is_boolean() {
            return Err(ProtocolError::invalid(
                "Invalid cooperative handoff capability",
            ));
        }
        result.insert("cooperativeHandoff".into(), v.clone());
    }
    let entries = frame["residencies"]
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or_else(|| ProtocolError::invalid("Invalid activity residencies"))?;
    let residencies = entries.iter().map(|v| {
        exact(record(v, "activity residency")?, &["label", "count"])?;
        Ok(json!({"label": string(&v["label"], "residency label", 128)?, "count": count(&v["count"], "residency count")?}))
    }).collect::<Result<Vec<_>>>()?;
    result.insert("residencies".into(), Value::Array(residencies));
    Ok(Value::Object(result))
}
