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

use super::{
    MAX_MANIFEST_BYTES, MAX_OFFERS, MAX_SERVICES, MAX_TOOLS, MAX_TOOLS_PER_OFFER, encoded_limit,
    entity, schema,
};
use crate::{
    ProtocolError, Result,
    codec::{count, exact, record, shaped, string},
};
use maka_runtime::capability::*;
use serde_json::{Map, Value};
use std::collections::HashSet;

pub fn decode_replace_input(value: &Value) -> Result<Manifest> {
    let frame = record(value, "Client Capability replacement")?;
    shaped(frame, &["registrationId", "offers"], &["services"])?;
    let values = array(&frame["offers"], MAX_OFFERS)?;
    let offers = values.iter().map(offer).collect::<Result<Vec<_>>>()?;
    // The source treats explicit null as an empty services array, preserving
    // field presence in its normalized value; omission remains omission.
    let services = match frame.get("services") {
        None => None,
        Some(Value::Null) => Some(Vec::new()),
        Some(value) => Some(
            array(value, MAX_SERVICES)?
                .iter()
                .map(service)
                .collect::<Result<Vec<_>>>()?,
        ),
    };
    if offers.is_empty() && services.as_ref().is_none_or(Vec::is_empty) {
        return Err(invalid("Client Capability registration is empty"));
    }
    let mut offer_ids = HashSet::new();
    let mut tool_ids = HashSet::new();
    for offer in &offers {
        if !offer_ids.insert(&offer.offer_id) {
            return Err(invalid("Duplicate Client Capability offer"));
        }
        for tool in &offer.tools {
            if !tool_ids.insert(format!("{}\0{}", tool.server_id, tool.name)) {
                return Err(invalid("Duplicate Client Capability tool"));
            }
        }
    }
    if tool_ids.len() > MAX_TOOLS {
        return Err(invalid("Too many Client Capability tools"));
    }
    let mut contracts = HashSet::new();
    for service in services.iter().flatten() {
        if !contracts.insert((&service.service_id, &service.version)) {
            return Err(invalid("Duplicate Client Capability service"));
        }
    }
    let manifest = Manifest {
        registration_id: entity(&frame["registrationId"], "registrationId")?,
        offers,
        services,
    };
    encoded_limit(&manifest, MAX_MANIFEST_BYTES)?;
    Ok(manifest)
}

pub fn decode_registration_result(value: &Value) -> Result<RegistrationResult> {
    let frame = record(value, "Client Capability registration result")?;
    exact(frame, &["registrationId", "revision"])?;
    Ok(RegistrationResult {
        registration_id: entity(&frame["registrationId"], "registrationId")?,
        revision: count(&frame["revision"], "revision")?,
    })
}

pub fn decode_unregister_input(value: &Value) -> Result<UnregisterInput> {
    let frame = record(value, "Client Capability unregister")?;
    exact(frame, &["registrationId"])?;
    Ok(UnregisterInput {
        registration_id: entity(&frame["registrationId"], "registrationId")?,
    })
}

fn offer(value: &Value) -> Result<Offer> {
    let frame = record(value, "Client Capability offer")?;
    shaped(
        frame,
        &[
            "offerId",
            "version",
            "affinity",
            "hostPathAccess",
            "label",
            "tools",
        ],
        &["description"],
    )?;
    let tools = array(&frame["tools"], MAX_TOOLS_PER_OFFER)?;
    if tools.is_empty() {
        return Err(invalid("Client Capability offer has no tools"));
    }
    Ok(Offer {
        offer_id: entity(&frame["offerId"], "offerId")?,
        version: string(&frame["version"], "version", 64)?,
        affinity: match frame["affinity"].as_str() {
            Some("call") => Affinity::Call,
            Some("turn") => Affinity::Turn,
            Some("session") => Affinity::Session,
            _ => return Err(invalid("Invalid Client Capability affinity")),
        },
        host_path_access: match frame["hostPathAccess"].as_str() {
            Some("none") => HostPathAccess::None,
            Some("cwd") => HostPathAccess::Cwd,
            _ => return Err(invalid("Invalid Client Capability host path access")),
        },
        label: string(&frame["label"], "label", 128)?,
        description: optional_string(frame, "description", 1024)?,
        tools: tools.iter().map(tool).collect::<Result<_>>()?,
    })
}

fn service(value: &Value) -> Result<ServiceOffer> {
    let frame = record(value, "Client Capability service")?;
    exact(frame, &["serviceId", "version"])?;
    Ok(ServiceOffer {
        service_id: entity(&frame["serviceId"], "serviceId")?,
        version: string(&frame["version"], "version", 64)?,
    })
}

fn tool(value: &Value) -> Result<ToolDescriptor> {
    let frame = record(value, "Client Capability tool")?;
    shaped(
        frame,
        &["serverId", "name", "inputSchema"],
        &["description", "annotations", "activityKind"],
    )?;
    schema::validate(&frame["inputSchema"])?;
    Ok(ToolDescriptor {
        server_id: string(&frame["serverId"], "serverId", 128)?,
        name: string(&frame["name"], "name", 128)?,
        input_schema: record(&frame["inputSchema"], "inputSchema")?.clone(),
        description: optional_string(frame, "description", 8192)?,
        annotations: frame.get("annotations").map(annotations).transpose()?,
        activity_kind: frame.get("activityKind").map(activity).transpose()?,
    })
}

fn annotations(value: &Value) -> Result<ToolAnnotations> {
    let frame = record(value, "Client Capability annotations")?;
    shaped(
        frame,
        &[],
        &[
            "title",
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ],
    )?;
    let boolean = |key| {
        frame
            .get(key)
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| invalid("Invalid Client Capability annotation flag"))
            })
            .transpose()
    };
    Ok(ToolAnnotations {
        title: optional_string(frame, "title", 128)?,
        read_only_hint: boolean("readOnlyHint")?,
        destructive_hint: boolean("destructiveHint")?,
        idempotent_hint: boolean("idempotentHint")?,
        open_world_hint: boolean("openWorldHint")?,
    })
}

fn activity(value: &Value) -> Result<ToolActivityKind> {
    Ok(match value.as_str() {
        Some("computer") => ToolActivityKind::Computer,
        Some("read") => ToolActivityKind::Read,
        Some("search") => ToolActivityKind::Search,
        Some("websearch") => ToolActivityKind::WebSearch,
        Some("webfetch") => ToolActivityKind::WebFetch,
        Some("edit") => ToolActivityKind::Edit,
        Some("command") => ToolActivityKind::Command,
        Some("explore") => ToolActivityKind::Explore,
        Some("browser") => ToolActivityKind::Browser,
        Some("tool") => ToolActivityKind::Tool,
        _ => return Err(invalid("Invalid Client Capability activity kind")),
    })
}

fn optional_string(frame: &Map<String, Value>, key: &str, max: usize) -> Result<Option<String>> {
    frame
        .get(key)
        .map(|value| string(value, key, max))
        .transpose()
}
fn array(value: &Value, max: usize) -> Result<&Vec<Value>> {
    value
        .as_array()
        .filter(|items| items.len() <= max)
        .ok_or_else(|| invalid("Invalid Client Capability array"))
}
fn invalid(message: &str) -> ProtocolError {
    ProtocolError::invalid(message)
}
