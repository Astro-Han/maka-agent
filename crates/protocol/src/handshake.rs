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

use crate::codec::{MAX_SAFE_INTEGER, composition, count, decode_activity, epoch, record, string};
use crate::{ProtocolError, Result};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolRange {
    pub min: u64,
    pub max: u64,
}

impl ProtocolRange {
    pub fn validate(self) -> Result<Self> {
        if self.min > self.max || self.max > MAX_SAFE_INTEGER {
            return Err(ProtocolError::invalid("Invalid protocol range"));
        }
        Ok(self)
    }
}

pub fn negotiate_protocol(client: ProtocolRange, host: ProtocolRange) -> Result<Option<u64>> {
    client.validate()?;
    host.validate()?;
    let selected = client.max.min(host.max);
    Ok((selected >= client.min.max(host.min)).then_some(selected))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", rename = "hello")]
pub struct ClientHello {
    pub client_instance_id: String,
    pub protocol_min: u64,
    pub protocol_max: u64,
    pub compatibility_epoch: u64,
    pub composition_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takeover: Option<Takeover>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_snapshot_version: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Takeover {
    pub expected_host_epoch: String,
}

pub fn decode_hello(value: &Value) -> Result<ClientHello> {
    let frame = record(value, "client frame")?;
    if value["kind"] != "hello" {
        return Err(ProtocolError::invalid("Expected hello"));
    }
    let range = decode_range(value)?;
    let generation = frame
        .get("generation")
        .map(|v| string(v, "generation", 128))
        .transpose()?;
    let takeover = frame
        .get("takeover")
        .map(|v| {
            record(v, "Runtime Host takeover")?;
            Ok(Takeover {
                expected_host_epoch: string(&v["expectedHostEpoch"], "expectedHostEpoch", 128)?,
            })
        })
        .transpose()?;
    if takeover.is_some() && generation.is_none() {
        return Err(ProtocolError::invalid(
            "Runtime Host takeover requires a generation",
        ));
    }
    Ok(ClientHello {
        client_instance_id: string(&value["clientInstanceId"], "clientInstanceId", 128)?,
        protocol_min: range.min,
        protocol_max: range.max,
        compatibility_epoch: epoch(frame.get("compatibilityEpoch"))?,
        composition_id: composition(frame.get("compositionId"))?,
        generation,
        takeover,
        activity_snapshot_version: (value["activitySnapshotVersion"].as_f64() == Some(2.0))
            .then_some(2),
    })
}

impl ClientHello {
    /// Compatibility selection only; authentication, generation/takeover and
    /// lifecycle admission must be checked by the host before sending accepted.
    pub fn negotiate(
        &self,
        host: ProtocolRange,
        epoch: u64,
        composition: &str,
    ) -> Result<Option<u64>> {
        let selected = negotiate_protocol(
            ProtocolRange {
                min: self.protocol_min,
                max: self.protocol_max,
            },
            host,
        )?;
        Ok(selected
            .filter(|_| self.compatibility_epoch == epoch && self.composition_id == composition))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Starting,
    Containing,
    Recovering,
    Ready,
    Draining,
}

fn lifecycle(value: &Value) -> Result<Lifecycle> {
    match value.as_str() {
        Some("starting") => Ok(Lifecycle::Starting),
        Some("containing") => Ok(Lifecycle::Containing),
        Some("recovering") => Ok(Lifecycle::Recovering),
        Some("ready") => Ok(Lifecycle::Ready),
        Some("draining") => Ok(Lifecycle::Draining),
        _ => Err(ProtocolError::invalid("Invalid Host state")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum HostHandshake {
    Accepted {
        root_id: String,
        host_epoch: String,
        connection_id: String,
        selected_protocol: u64,
        compatibility_epoch: u64,
        composition_id: String,
        composition_revision: String,
        state: Lifecycle,
        #[serde(skip_serializing_if = "Option::is_none")]
        cooperative_handoff: Option<bool>,
    },
    Incompatible {
        host_epoch: String,
        protocol_min: u64,
        protocol_max: u64,
        compatibility_epoch: u64,
        composition_id: String,
        composition_revision: String,
        state: Lifecycle,
        replacement: Replacement,
        #[serde(skip_serializing_if = "Option::is_none")]
        generation: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        activity: Option<Value>,
    },
    Draining {
        host_epoch: String,
        composition_id: String,
        composition_revision: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Replacement {
    BlockedByResidency,
    WaitForIdleExit,
}

pub fn decode_host_handshake(value: &Value) -> Result<HostHandshake> {
    let frame = record(value, "host frame")?;
    let host_epoch = string(&value["hostEpoch"], "hostEpoch", 128)?;
    let composition_id = composition(frame.get("compositionId"))?;
    let composition_revision = match frame.get("compositionRevision") {
        None => "legacy".into(),
        Some(v) => {
            let revision = string(v, "compositionRevision", 128)?;
            if revision.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
                return Err(ProtocolError::invalid("Invalid compositionRevision"));
            }
            revision
        }
    };
    match value["kind"].as_str() {
        Some("draining") => Ok(HostHandshake::Draining {
            host_epoch,
            composition_id,
            composition_revision,
        }),
        Some("accepted") => {
            let root_id = string(&value["rootId"], "rootId", 64)?;
            if root_id.len() != 64
                || !root_id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(ProtocolError::invalid("Invalid rootId"));
            }
            let state = lifecycle(&value["state"])?;
            if state == Lifecycle::Draining {
                return Err(ProtocolError::invalid("Accepted Host cannot be draining"));
            }
            let cooperative_handoff = frame
                .get("cooperativeHandoff")
                .map(|v| {
                    if v == true {
                        Ok(true)
                    } else {
                        Err(ProtocolError::invalid(
                            "Invalid cooperative handoff capability",
                        ))
                    }
                })
                .transpose()?;
            Ok(HostHandshake::Accepted {
                root_id,
                host_epoch,
                composition_id,
                composition_revision,
                state,
                cooperative_handoff,
                connection_id: string(&value["connectionId"], "connectionId", 128)?,
                selected_protocol: count(&value["selectedProtocol"], "selectedProtocol")?,
                compatibility_epoch: epoch(frame.get("compatibilityEpoch"))?,
            })
        }
        Some("incompatible") => {
            let range = decode_range(value)?;
            let replacement = match value["replacement"].as_str() {
                Some("blocked_by_residency") => Replacement::BlockedByResidency,
                Some("wait_for_idle_exit") => Replacement::WaitForIdleExit,
                _ => return Err(ProtocolError::invalid("Invalid replacement disposition")),
            };
            Ok(HostHandshake::Incompatible {
                host_epoch,
                composition_id,
                composition_revision,
                replacement,
                protocol_min: range.min,
                protocol_max: range.max,
                compatibility_epoch: epoch(frame.get("compatibilityEpoch"))?,
                state: lifecycle(&value["state"])?,
                generation: frame
                    .get("generation")
                    .map(|v| string(v, "generation", 128))
                    .transpose()?,
                activity: frame.get("activity").map(decode_activity).transpose()?,
            })
        }
        _ => Err(ProtocolError::invalid("Expected host handshake")),
    }
}

fn decode_range(value: &Value) -> Result<ProtocolRange> {
    ProtocolRange {
        min: count(&value["protocolMin"], "protocolMin")?,
        max: count(&value["protocolMax"], "protocolMax")?,
    }
    .validate()
}
