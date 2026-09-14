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

use super::{Host, HostError, configuration};
use maka_config::onboarding::OnboardingPreparation;
use maka_model::connection::DiscoveryRequest;
use maka_protocol::{Operation, OperationError, OperationErrorCode, Outcome};
use maka_runtime::configuration::{ConnectionEffectFailureClass, onboarding::*};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ConnectionOnboardingVerify | Operation::ConnectionOnboardingSave
    )
}
pub(super) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    if operation == Operation::ConnectionOnboardingSave {
        maka_protocol::onboarding::decode_save_result(value)?;
    } else {
        maka_protocol::onboarding::decode_verify_result(value)?;
    }
    Ok(value.clone())
}
pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    input: &Value,
) -> Result<Outcome, HostError> {
    let save = operation == Operation::ConnectionOnboardingSave;
    let (input, enabled) = maka_protocol::onboarding::decode_input(input, save)?;
    let result = perform(host, input, enabled, save).await;
    match result {
        Ok(value) => {
            decode_output(operation, &value)?;
            if value["kind"] == "saved" {
                let revision = host.change_revision.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = host
                    .changes
                    .send(json!({"kind":"configuration.changed","revision":revision}));
            }
            Ok(Outcome::success(value))
        }
        Err(error) => Ok(Outcome::failure(error)),
    }
}
async fn perform(
    host: &Host,
    input: OnboardingInput,
    enabled: Vec<String>,
    save: bool,
) -> Result<Value, OperationError> {
    let lane = match &input.target {
        OnboardingTarget::Existing { connection_id } => connection_id.clone(),
        OnboardingTarget::Create { provider_type, .. } => {
            format!("onboarding:create:{provider_type}")
        }
    };
    let _lane = host.connection_effects.lane(&lane).await;
    let prepared = match host
        .configuration
        .prepare_onboarding(input)
        .await
        .map_err(configuration::failure)?
    {
        OnboardingPreparation::Ready(prepared) => prepared,
        OnboardingPreparation::Rejected(reason) => {
            return output(OnboardingVerifyResult::Rejected { reason });
        }
        OnboardingPreparation::Unsupported => {
            return Err(OperationError {
                code: OperationErrorCode::OperationUnavailable,
                message: "Provider or configured proxy onboarding is not installed".into(),
            });
        }
    };
    let kind = prepared.protocol();
    let headers = prepared
        .request_headers()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| OperationError {
            code: OperationErrorCode::InternalFailure,
            message: "Stored request headers are invalid".into(),
        })?
        .unwrap_or_default();
    let models = match super::connection_effects::client(prepared.network_configuration())?
        .discover(DiscoveryRequest {
            kind,
            base_url: prepared.endpoint(),
            credential: prepared.api_key(),
            headers: &headers,
        })
        .await
    {
        Ok(models) => models,
        Err(error_class) => return output(OnboardingVerifyResult::Failed { error_class }),
    };
    if save {
        let _admission = host.executions.lock_admission().await;
        if host.draining.is_cancelled() {
            return Err(OperationError {
                code: OperationErrorCode::HostDraining,
                message: "Runtime Host is draining".into(),
            });
        }
        output(
            prepared
                .complete(
                    models,
                    enabled,
                    configuration::now().map_err(configuration::failure)?,
                )
                .await
                .map_err(configuration::failure)?,
        )
    } else if models.is_empty() {
        output(OnboardingVerifyResult::Failed {
            error_class: ConnectionEffectFailureClass::InvalidResponse,
        })
    } else {
        output(OnboardingVerifyResult::Verified { models })
    }
}
fn output(value: impl serde::Serialize) -> Result<Value, OperationError> {
    serde_json::to_value(value).map_err(|_| OperationError {
        code: OperationErrorCode::InternalFailure,
        message: "Invalid onboarding projection".into(),
    })
}
