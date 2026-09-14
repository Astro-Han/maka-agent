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

use crate::codec::{count, exact, record, string};
use crate::{ProtocolError, Result};
use maka_runtime::configuration::validation;
pub use maka_runtime::configuration::{
    ConnectionEffectChangedDomain, ConnectionEffectFailureClass, ConnectionEffectRejectionReason,
    ConnectionModelFetchInput, ConnectionModelFetchResult, ConnectionTestProjection,
    ConnectionTestRunInput, ConnectionTestRunResult,
};
use maka_runtime::configuration::{ConnectionVersionBasis, ModelDiscoverySource};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub fn decode_connection_model_fetch_input(value: &Value) -> Result<ConnectionModelFetchInput> {
    let input = record(value, "connection model fetch input")?;
    exact(input, &["connectionId"])?;
    // The protocol input permits entity IDs; the committed domain basis requires a UUID.
    let connection_id = entity_id(&input["connectionId"])?;
    Ok(ConnectionModelFetchInput { connection_id })
}

fn entity_id(value: &Value) -> Result<String> {
    let connection_id = string(value, "connectionId", 128)?;
    if !connection_id
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(ProtocolError::invalid("Invalid connectionId"));
    }
    Ok(connection_id)
}

pub fn decode_connection_model_fetch_result(value: &Value) -> Result<ConnectionModelFetchResult> {
    let result = record(value, "connection model fetch result")?;
    match value["kind"].as_str() {
        Some("committed") => {
            exact(
                result,
                &[
                    "kind",
                    "catalogRevision",
                    "connection",
                    "modelCount",
                    "source",
                    "fetchedAt",
                ],
            )?;
            let connection = connection_basis(&result["connection"])?;
            let model_count = count(&result["modelCount"], "model count")?;
            if !(1..=2048).contains(&model_count) {
                return Err(ProtocolError::invalid("Invalid model count"));
            }
            Ok(ConnectionModelFetchResult::Committed {
                catalog_revision: count(&result["catalogRevision"], "connection catalog revision")?,
                connection,
                model_count,
                source: closed::<ModelDiscoverySource>(&result["source"])?,
                fetched_at: count(&result["fetchedAt"], "models fetched at")?,
            })
        }
        Some("rejected") => {
            exact(result, &["kind", "reason"])?;
            Ok(ConnectionModelFetchResult::Rejected {
                reason: closed(&result["reason"])?,
            })
        }
        Some("superseded") => {
            exact(result, &["kind", "changed"])?;
            let changed = changed_domains(&result["changed"])?;
            Ok(ConnectionModelFetchResult::Superseded { changed })
        }
        Some("failed") => {
            exact(result, &["kind", "errorClass"])?;
            Ok(ConnectionModelFetchResult::Failed {
                error_class: closed(&result["errorClass"])?,
            })
        }
        _ => Err(ProtocolError::invalid(
            "Invalid connection model fetch result",
        )),
    }
}

pub fn decode_connection_test_run_input(value: &Value) -> Result<ConnectionTestRunInput> {
    let input = record(value, "connection test input")?;
    exact(input, &["connectionId", "modelId"])?;
    Ok(ConnectionTestRunInput {
        connection_id: entity_id(&input["connectionId"])?,
        model_id: nullable(&input["modelId"], |v| string(v, "model id", 512))?,
    })
}

pub fn decode_connection_test_run_result(value: &Value) -> Result<ConnectionTestRunResult> {
    let result = record(value, "connection test result")?;
    match value["kind"].as_str() {
        Some("committed") => {
            exact(result, &["kind", "catalogRevision", "connection", "test"])?;
            Ok(ConnectionTestRunResult::Committed {
                catalog_revision: count(&result["catalogRevision"], "connection catalog revision")?,
                connection: connection_basis(&result["connection"])?,
                test: decode_connection_test_projection(&result["test"])?,
            })
        }
        Some("rejected") => {
            exact(result, &["kind", "reason"])?;
            Ok(ConnectionTestRunResult::Rejected {
                reason: closed(&result["reason"])?,
            })
        }
        Some("superseded") => {
            exact(result, &["kind", "changed"])?;
            Ok(ConnectionTestRunResult::Superseded {
                changed: changed_domains(&result["changed"])?,
            })
        }
        _ => Err(ProtocolError::invalid("Invalid connection test result")),
    }
}

pub fn decode_connection_test_projection(value: &Value) -> Result<ConnectionTestProjection> {
    let projection = record(value, "connection test projection")?;
    match value["kind"].as_str() {
        Some("verified") => {
            exact(projection, &["kind", "checkedAt", "modelId", "latencyMs"])?;
            Ok(ConnectionTestProjection::Verified {
                checked_at: string(&projection["checkedAt"], "connection test checkedAt", 128)?,
                model_id: string(&projection["modelId"], "model id", 512)?,
                latency_ms: count(&projection["latencyMs"], "connection test latency")?,
            })
        }
        Some("failed") => {
            exact(
                projection,
                &[
                    "kind",
                    "checkedAt",
                    "modelId",
                    "latencyMs",
                    "statusCode",
                    "errorClass",
                ],
            )?;
            Ok(ConnectionTestProjection::Failed {
                checked_at: string(&projection["checkedAt"], "connection test checkedAt", 128)?,
                model_id: nullable(&projection["modelId"], |v| string(v, "model id", 512))?,
                latency_ms: nullable(&projection["latencyMs"], |v| {
                    count(v, "connection test latency")
                })?,
                status_code: nullable(&projection["statusCode"], |v| {
                    let code = count(v, "connection test status code")?;
                    if !(100..=599).contains(&code) {
                        return Err(ProtocolError::invalid(
                            "Invalid connection test status code",
                        ));
                    }
                    Ok(code)
                })?,
                error_class: closed(&projection["errorClass"])?,
            })
        }
        _ => Err(ProtocolError::invalid("Invalid connection test projection")),
    }
}

fn nullable<T>(value: &Value, decode: impl FnOnce(&Value) -> Result<T>) -> Result<Option<T>> {
    if value.is_null() {
        Ok(None)
    } else {
        decode(value).map(Some)
    }
}

fn connection_basis(value: &Value) -> Result<ConnectionVersionBasis> {
    let basis = record(value, "connection basis")?;
    exact(basis, &["connectionId", "revision"])?;
    let connection = ConnectionVersionBasis {
        connection_id: string(&basis["connectionId"], "connection id", 128)?,
        revision: count(&basis["revision"], "connection revision")?,
    };
    validation::basis(&connection).map_err(ProtocolError::invalid)?;
    Ok(connection)
}

fn changed_domains(value: &Value) -> Result<Vec<ConnectionEffectChangedDomain>> {
    let values = value
        .as_array()
        .filter(|v| (1..=3).contains(&v.len()))
        .ok_or_else(|| ProtocolError::invalid("Invalid connection effect changed domains"))?;
    let mut changed = Vec::with_capacity(values.len());
    for value in values {
        let domain = closed::<ConnectionEffectChangedDomain>(value)?;
        if changed.contains(&domain) {
            return Err(ProtocolError::invalid(
                "Duplicate connection effect changed domain",
            ));
        }
        changed.push(domain);
    }
    Ok(changed)
}

fn closed<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid connection effect enum value"))
}
