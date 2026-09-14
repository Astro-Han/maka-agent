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

//! Root policy wire boundary; canonical values and mutation normalization are core-owned.
use crate::{OperationErrorCode, ProtocolError, Result, codec};
use maka_runtime::configuration::policy::{
    self, RuntimePolicyMutationInput, RuntimePolicyMutationResult, RuntimePolicySnapshot,
};
use maka_runtime::configuration::{policy::network_update, validation};
use serde_json::Value;

pub fn decode_network_proxy_update(value: &Value) -> Result<network_update::Update> {
    network_update::decode(value.clone()).map_err(ProtocolError::invalid)
}

pub fn decode_network_proxy_result(value: &Value) -> Result<network_update::UpdateResult> {
    use network_update::UpdateResult;
    let record = codec::record(value, "network proxy update result")?;
    let fields: &[&str] = match value["kind"].as_str() {
        Some("committed") => &["kind", "revision", "credentialStatus"],
        Some("revision_conflict") => &["kind", "expectedRevision", "actualRevision"],
        Some("credential_stale" | "proxy_target_mismatch") => &["kind", "expected", "actual"],
        _ => {
            return Err(ProtocolError::invalid(
                "Invalid network proxy update result",
            ));
        }
    };
    codec::exact(record, fields)?;
    let mut normalized = value.clone();
    for (path, fields) in [
        ("", &["revision", "expectedRevision", "actualRevision"][..]),
        ("/credentialStatus", &["revision", "updatedAt"][..]),
        ("/expected", &["revision", "port"][..]),
        ("/actual", &["revision", "port"][..]),
    ] {
        if let Some(record) = normalized.pointer_mut(path).and_then(Value::as_object_mut) {
            for field in fields {
                if let Some(v) = record.get_mut(*field).filter(|v| !v.is_null()) {
                    *v = Value::from(codec::count(v, field)?);
                }
            }
        }
    }
    let mut result: UpdateResult = serde_json::from_value(normalized)
        .map_err(|_| ProtocolError::invalid("Invalid network proxy update result"))?;
    let validate = |v: std::result::Result<(), String>| v.map_err(ProtocolError::invalid);
    match &mut result {
        UpdateResult::Committed {
            revision,
            credential_status,
        } => {
            validate(validation::revision(*revision, false))?;
            codec::exact(
                codec::record(&value["credentialStatus"], "credential status")?,
                &[
                    "locator",
                    "configured",
                    "credentialId",
                    "revision",
                    "updatedAt",
                ],
            )?;
            validate(validation::credential_status(credential_status))?;
        }
        UpdateResult::RevisionConflict {
            expected_revision,
            actual_revision,
        } => {
            validate(validation::revision(*expected_revision, false))?;
            validate(validation::revision(*actual_revision, false))?;
        }
        UpdateResult::CredentialStale { expected, actual } => {
            for basis in [expected, actual].into_iter().flatten() {
                validate(validation::credential_basis(basis))?;
            }
        }
        UpdateResult::ProxyTargetMismatch { expected, actual } => {
            validate(expected.normalize())?;
            validate(actual.normalize())?;
        }
    }
    Ok(result)
}

pub const QUERY_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InternalFailure,
    OperationErrorCode::PersistenceFailed,
];
pub const MUTATION_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::InternalFailure,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
];

pub fn decode_query_input(value: &Value) -> Result<()> {
    codec::exact(codec::record(value, "runtime policy query input")?, &[])
}

pub fn decode_query_result(value: &Value) -> Result<RuntimePolicySnapshot> {
    policy::decode_canonical_snapshot(value.clone()).map_err(ProtocolError::invalid)
}

pub fn decode_mutation_input(value: &Value) -> Result<RuntimePolicyMutationInput> {
    policy::normalize_mutation(value.clone()).map_err(ProtocolError::invalid)
}

pub fn decode_mutation_result(value: &Value) -> Result<RuntimePolicyMutationResult> {
    let record = codec::record(value, "runtime policy mutation result")?;
    match record.get("kind").and_then(Value::as_str) {
        Some("committed") => {
            codec::exact(record, &["kind", "revision"])?;
            Ok(RuntimePolicyMutationResult::Committed {
                revision: codec::count(&record["revision"], "runtime policy revision")?,
            })
        }
        Some("revision_conflict") => {
            codec::exact(record, &["kind", "expectedRevision", "actualRevision"])?;
            Ok(RuntimePolicyMutationResult::RevisionConflict {
                expected_revision: codec::count(&record["expectedRevision"], "expected revision")?,
                actual_revision: codec::count(&record["actualRevision"], "actual revision")?,
            })
        }
        _ => Err(ProtocolError::invalid(
            "Invalid runtime policy mutation result",
        )),
    }
}
