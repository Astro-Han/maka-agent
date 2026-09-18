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

use super::{Output, failure, parsed};
use maka_config::ConfigurationStore;
use maka_protocol::{Operation, OperationError, OperationErrorCode, runtime_policy};
use maka_runtime::configuration::policy::RuntimePolicyMutation;
use serde_json::Value;

pub(super) async fn test_network(
    store: &ConfigurationStore,
    value: &Value,
) -> Result<Output, OperationError> {
    use maka_runtime::configuration::policy::{network_test, network_update::CredentialTarget};
    let input = maka_protocol::network_proxy::decode_input(value).map_err(|_| OperationError {
        code: OperationErrorCode::InvalidRequest,
        message: "Invalid network proxy test input".into(),
    })?;
    let saved = store
        .network_configuration()
        .await
        .map_err(|_| OperationError {
            code: OperationErrorCode::InternalFailure,
            message: "Cannot read network proxy configuration".into(),
        })?;
    let proxy = input.network_proxy.as_ref().unwrap_or(&saved.proxy);
    // A draft may not redirect the saved password to a different proxy account.
    // The existing desktop commits credentials before testing; no new wire field is needed.
    if proxy.enabled
        && proxy.auth_enabled
        && CredentialTarget::from_proxy(proxy) != CredentialTarget::from_proxy(&saved.proxy)
    {
        return Ok(Output::NetworkProxyTest(network_test::Output::failed(
            "Save the proxy target before testing with stored credentials",
        )));
    }
    Ok(Output::NetworkProxyTest(
        maka_network::probe(
            proxy,
            saved.password.as_deref(),
            input.url.as_deref(),
            input.timeout_ms,
        )
        .await,
    ))
}

pub(super) async fn execute(
    store: &ConfigurationStore,
    operation: Operation,
    value: &Value,
) -> Result<Output, OperationError> {
    match operation {
        Operation::RuntimePolicyQuery => {
            let snapshot = store.runtime_policy().await.map_err(failure)?;
            snapshot.validate().map_err(|_| OperationError {
                code: OperationErrorCode::InternalFailure,
                message: format!(
                    "Stored policy exceeds the wire snapshot budget; reduce settings with runtime.policy.mutate at expectedRevision {}",
                    snapshot.revision
                ),
            })?;
            Ok(Output::Policy(snapshot))
        }
        Operation::RuntimePolicyMutate => {
            let input = parsed(runtime_policy::decode_mutation_input(value)).map_err(failure)?;
            match input.operation {
                RuntimePolicyMutation::SetSubagents { value } => store
                    .set_subagents(input.expected_revision, value)
                    .await
                    .map(Output::PolicyMutation)
                    .map_err(failure),
                RuntimePolicyMutation::SetNetworkProxy { value } => store
                    .set_network_proxy(input.expected_revision, value)
                    .await
                    .map(Output::PolicyMutation)
                    .map_err(failure),
                RuntimePolicyMutation::SetWorkspaceInstructions { value } => store
                    .set_workspace_instructions(input.expected_revision, value)
                    .await
                    .map(Output::PolicyMutation)
                    .map_err(failure),
                RuntimePolicyMutation::SetPersonalization { value } => store
                    .set_personalization(input.expected_revision, value)
                    .await
                    .map(Output::PolicyMutation)
                    .map_err(failure),
                RuntimePolicyMutation::SetChatDefaults { value } => store
                    .set_chat_defaults(input.expected_revision, value)
                    .await
                    .map(Output::PolicyMutation)
                    .map_err(failure),
                _ => Err(OperationError {
                    code: OperationErrorCode::OperationUnavailable,
                    message: "This policy setting has no native execution consumer yet".into(),
                }),
            }
        }
        _ => unreachable!("policy dispatch is restricted by configuration operations"),
    }
}
