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

use super::{SUBSCRIPTION_FRAME_MAX_BYTES, TrueFlag, decode, ensure, entity, id};
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceChange {
    pub source_session_id: String,
    #[serde(rename = "ref")]
    pub resource_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInterestInput {
    pub subscription_id: String,
    pub refs: Vec<String>,
}

pub fn decode_pty_interest_input(value: &Value) -> Result<PtyInterestInput> {
    let input: PtyInterestInput = decode(value)?;
    id(&input.subscription_id)?;
    ensure(input.refs.len() <= 16, "Too many PTY interests")?;
    let mut seen = std::collections::HashSet::new();
    for reference in &input.refs {
        resource_ref(reference)?;
        ensure(seen.insert(reference), "Duplicate PTY interest")?;
    }
    Ok(input)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceDomain {
    #[serde(rename = "runtime_resource")]
    RuntimeResource,
}

/// The implemented resource-domain subset of Session observation frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum ResourceObservationFrame {
    #[serde(rename = "subscription.session_domain_changed")]
    DomainChanged {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        session_id: String,
        domain: ResourceDomain,
        resources: Vec<ResourceChange>,
    },
    #[serde(rename = "subscription.runtime_resource_pty_data")]
    PtyData {
        host_epoch: String,
        subscription_id: String,
        session_id: String,
        #[serde(rename = "ref")]
        resource_ref: String,
        pty_sequence: u64,
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reset: Option<TrueFlag>,
    },
}

pub fn decode_resource_observation_frame(value: &Value) -> Result<ResourceObservationFrame> {
    ensure(
        serde_json::to_vec(value)
            .map_err(|error| crate::ProtocolError::invalid(error.to_string()))?
            .len()
            <= SUBSCRIPTION_FRAME_MAX_BYTES,
        "Resource observation frame exceeds byte limit",
    )?;
    let frame: ResourceObservationFrame = decode(value)?;
    match &frame {
        ResourceObservationFrame::DomainChanged {
            host_epoch,
            subscription_id,
            sequence,
            session_id,
            resources,
            ..
        } => {
            envelope(host_epoch, subscription_id, session_id)?;
            ensure(*sequence > 0, "Invalid subscription sequence")?;
            ensure(
                !resources.is_empty() && resources.len() <= 64,
                "Invalid resource change count",
            )?;
            let mut seen = std::collections::HashSet::new();
            for change in resources {
                entity(&change.source_session_id)?;
                resource_ref(&change.resource_ref)?;
                ensure(
                    seen.insert((&change.source_session_id, &change.resource_ref)),
                    "Duplicate resource change",
                )?;
            }
        }
        ResourceObservationFrame::PtyData {
            host_epoch,
            subscription_id,
            session_id,
            resource_ref: reference,
            pty_sequence,
            data,
            ..
        } => {
            envelope(host_epoch, subscription_id, session_id)?;
            resource_ref(reference)?;
            ensure(*pty_sequence > 0, "Invalid PTY sequence")?;
            ensure(data.len() <= 48 * 1024, "PTY data exceeds byte limit")?;
        }
    }
    Ok(frame)
}

fn envelope(epoch: &str, subscription: &str, session: &str) -> Result<()> {
    id(epoch)?;
    id(subscription)?;
    entity(session)
}

fn resource_ref(reference: &str) -> Result<()> {
    ensure(
        !reference.is_empty() && reference.len() <= 256,
        "Invalid resource ref",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pty_uses_an_independent_sequence_strict_interest_and_encoded_frame_budget() {
        let frame = json!({"kind":"subscription.runtime_resource_pty_data", "hostEpoch":"epoch",
            "subscriptionId":"observer", "sessionId":"session", "ref":"resource", "ptySequence":1.0,
            "data":"\u{1b}[31m中文\u{0}", "reset":true});
        decode_resource_observation_frame(&frame).unwrap();
        for (field, value) in [
            ("sequence", json!(1)),
            ("ptySequence", json!(0)),
            ("reset", json!(false)),
            ("reset", Value::Null),
            ("data", json!("\0".repeat(11_000))),
        ] {
            let mut invalid = frame.clone();
            invalid[field] = value;
            assert!(
                decode_resource_observation_frame(&invalid).is_err(),
                "{field}"
            );
        }
        decode_pty_interest_input(&json!({"subscriptionId":"observer", "refs":[]})).unwrap();
        assert!(
            decode_pty_interest_input(&json!({"subscriptionId":"observer", "refs":["r","r"]}))
                .is_err()
        );
        assert!(decode_pty_interest_input(&json!({"subscriptionId":"observer", "refs":(0..17).map(|n|n.to_string()).collect::<Vec<_>>()})).is_err());
    }
}
