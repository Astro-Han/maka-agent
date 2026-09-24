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
use maka_plugins::provider::Binding;
use maka_protocol::{Operation, OperationError, OperationErrorCode, Outcome};
use maka_runtime::configuration::{ConnectionEffectFailureClass, onboarding::*};
use maka_runtime::oauth::Target;
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
    mut input: OnboardingInput,
    enabled: Vec<String>,
    save: bool,
) -> Result<Value, OperationError> {
    let lane = match &input.target {
        Target::Existing { expected, .. } => expected.connection_id.clone(),
        Target::Create { slug, .. } => format!("onboarding:create:{slug}"),
    };
    let _lane = host.connection_effects.lane(&lane).await;
    let admission = host.executions.lock_admission().await;
    if let Target::Create {
        provider,
        configuration: value,
        ..
    } = &mut input.target
    {
        let provider = Binding::resolve(provider, &host.executions.plugin_catalog)
            .map_err(provider_failure)?;
        *value = provider
            .definition()
            .configure(value.clone())
            .map_err(provider_failure)?;
    }
    let mut prepared = match host
        .configuration
        .prepare_onboarding(input.target)
        .await
        .map_err(configuration::failure)?
    {
        OnboardingPreparation::Ready(prepared) => prepared,
        OnboardingPreparation::Rejected(reason) => {
            return output(OnboardingVerifyResult::Rejected { reason });
        }
    };
    let provider = super::connection_effects::ProviderOperation::prepare(
        host,
        prepared.connection(),
        prepared
            .provider_credential()
            .map_err(configuration::failure)?,
        prepared.network_configuration(),
    )?;
    drop(admission);
    let credential = provider.credential().await?;
    if let Some(credential) = &credential {
        prepared
            .accept_credential(credential.clone())
            .map_err(configuration::failure)?;
    }
    let models = match provider
        .discover(credential.as_ref(), prepared.request_headers())
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
fn provider_failure(error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code: OperationErrorCode::OperationUnavailable,
        message: error.to_string().chars().take(1024).collect(),
    }
}
fn output(value: impl serde::Serialize) -> Result<Value, OperationError> {
    serde_json::to_value(value).map_err(|_| OperationError {
        code: OperationErrorCode::InternalFailure,
        message: "Invalid onboarding projection".into(),
    })
}
