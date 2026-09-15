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

use crate::{ProtocolError, Result, codec, handshake::Lifecycle};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status<'a> {
    pub host_epoch: &'a str,
    pub composition_id: &'a str,
    pub composition_revision: &'a str,
    pub state: Lifecycle,
    pub connections: usize,
    pub active_operations: usize,
    pub active_residencies: usize,
}

#[derive(Debug, Serialize)]
pub struct Residency<'a> {
    pub label: &'a str,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum Platform {
    #[serde(rename = "linux")]
    Linux,
    #[serde(rename = "darwin")]
    MacOs,
    #[serde(rename = "win32")]
    Windows,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics<'a> {
    #[serde(flatten)]
    pub status: Status<'a>,
    pub composition_modules: &'a [&'a str],
    pub residencies: Vec<Residency<'a>>,
    pub upgrade_blocking_activity: bool,
    pub protocol_version: u64,
    pub compatibility_epoch: u64,
    pub pid: u32,
    pub process_uptime_seconds: u64,
    /// Legacy wire name. A native Host explicitly reports that Node is absent.
    pub node_version: &'a str,
    pub platform: Platform,
    pub arch: &'a str,
    pub os_release: String,
    pub logs: Vec<String>,
}

const STATUS_FIELDS: &[&str] = &[
    "hostEpoch",
    "compositionId",
    "compositionRevision",
    "state",
    "connections",
    "activeOperations",
    "activeResidencies",
];
const DIAGNOSTIC_FIELDS: &[&str] = &[
    "compositionModules",
    "residencies",
    "upgradeBlockingActivity",
    "protocolVersion",
    "compatibilityEpoch",
    "pid",
    "processUptimeSeconds",
    "nodeVersion",
    "platform",
    "arch",
    "osRelease",
    "logs",
];

pub fn decode_status(value: &Value) -> Result<()> {
    codec::exact(codec::record(value, "Host status")?, STATUS_FIELDS)?;
    status_fields(value)
}

pub fn decode_diagnostics(value: &Value) -> Result<()> {
    if serde_json::to_vec(value)
        .map_err(|e| ProtocolError::invalid(e.to_string()))?
        .len()
        > 72 * 1024
    {
        return Err(ProtocolError::invalid("Host diagnostics exceed byte limit"));
    }
    let fields = codec::record(value, "Host diagnostics")?;
    codec::shaped(fields, STATUS_FIELDS, DIAGNOSTIC_FIELDS)?;
    if DIAGNOSTIC_FIELDS
        .iter()
        .any(|key| !fields.contains_key(*key))
    {
        return Err(ProtocolError::invalid("Missing Host diagnostics field"));
    }
    status_fields(value)?;
    for entry in entries(&value["compositionModules"], 64)? {
        codec::string(entry, "composition module", 64)?;
    }
    for entry in entries(&value["residencies"], 128)? {
        codec::exact(codec::record(entry, "residency")?, &["label", "count"])?;
        codec::string(&entry["label"], "residency label", 128)?;
        codec::count(&entry["count"], "residency count")?;
    }
    if !value["upgradeBlockingActivity"].is_boolean() {
        return Err(ProtocolError::invalid("Invalid upgrade blocking activity"));
    }
    for key in [
        "protocolVersion",
        "compatibilityEpoch",
        "pid",
        "processUptimeSeconds",
    ] {
        codec::count(&value[key], key)?;
    }
    for (key, limit) in [("nodeVersion", 64), ("arch", 64), ("osRelease", 256)] {
        codec::string(&value[key], key, limit)?;
    }
    if !matches!(
        value["platform"].as_str(),
        Some(
            "aix"
                | "android"
                | "darwin"
                | "freebsd"
                | "haiku"
                | "linux"
                | "openbsd"
                | "sunos"
                | "win32"
                | "cygwin"
                | "netbsd"
        )
    ) {
        return Err(ProtocolError::invalid("Invalid Host platform"));
    }
    for entry in entries(&value["logs"], 256)? {
        if !entry
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 10 * 1024)
        {
            return Err(ProtocolError::invalid("Invalid Host diagnostic log entry"));
        }
    }
    Ok(())
}

fn status_fields(value: &Value) -> Result<()> {
    for key in ["hostEpoch", "compositionId", "compositionRevision"] {
        codec::string(&value[key], key, 128)?;
    }
    for key in ["connections", "activeOperations", "activeResidencies"] {
        codec::count(&value[key], key)?;
    }
    if !matches!(
        value["state"].as_str(),
        Some("starting" | "containing" | "recovering" | "ready" | "draining")
    ) {
        return Err(ProtocolError::invalid("Invalid Host lifecycle"));
    }
    Ok(())
}

fn entries(value: &Value, limit: usize) -> Result<&[Value]> {
    value
        .as_array()
        .filter(|a| a.len() <= limit)
        .map(Vec::as_slice)
        .ok_or_else(|| ProtocolError::invalid("Invalid Host diagnostic entries"))
}
