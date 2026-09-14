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
    access, artifacts, bootstrap, capabilities, configuration, context, interactions, sessions,
    subscriptions, turns,
};
use maka_protocol::OperationErrorCode;
use maka_protocol::{Operation, OperationRegistry, Result};
use serde_json::Value;

pub(super) struct Operations;

impl OperationRegistry for Operations {
    fn decode_input(&self, operation: Operation, value: &Value) -> Result<Value> {
        if maka_protocol::workhub::supports(operation) {
            return maka_protocol::workhub::decode_input(operation, value);
        }
        if operation == Operation::SkillCatalogQuery {
            maka_protocol::skills::decode_catalog_input(value)?;
            return Ok(value.clone());
        }
        if operation == Operation::SkillCatalogInvocableQuery {
            maka_protocol::skills::decode_invocable_input(value)?;
            return Ok(value.clone());
        }
        if maka_protocol::navigation::supports(operation) {
            return maka_protocol::navigation::decode_input(operation, value);
        }
        if maka_protocol::oauth::supports(operation) {
            return maka_protocol::oauth::decode_input(operation, value);
        }
        if operation == Operation::SessionExecutionBoundaryQuery {
            maka_protocol::execution_boundary::decode_input(value)?;
            return Ok(value.clone());
        }
        if super::onboarding::supports(operation) {
            maka_protocol::onboarding::decode_input(
                value,
                operation == Operation::ConnectionOnboardingSave,
            )?;
            return Ok(value.clone());
        }
        if super::projects::supports(operation) {
            return super::projects::decode_input(operation, value);
        }
        if super::messages::supports(operation) {
            maka_protocol::message::decode_input(operation, value)?;
            return Ok(value.clone());
        }
        if maka_protocol::resource::is_controller(operation) {
            if operation == Operation::RuntimeResourceControllerControl {
                maka_protocol::resource::decode_controller_control(value)?;
            } else {
                maka_protocol::resource::decode_controller_identity(value)?;
            }
            return Ok(value.clone());
        }
        if operation == Operation::RuntimeResourceStart {
            maka_protocol::resource::decode_start_input(value)?;
            return Ok(value.clone());
        }
        if operation == Operation::RuntimeResourceStop {
            maka_protocol::resource::decode_stop_input(value)?;
            return Ok(value.clone());
        }
        if operation == Operation::RuntimeResourceQuery {
            return serde_json::to_value(maka_protocol::resource::decode_query_input(value)?)
                .map_err(|e| maka_protocol::ProtocolError::invalid(e.to_string()));
        }
        if context::supports(operation) {
            return context::decode_input(operation, value);
        }
        if artifacts::supports(operation) {
            return artifacts::decode_input(operation, value);
        }
        if interactions::supports(operation) {
            return interactions::decode_input(operation, value);
        }
        if capabilities::supports(operation) {
            return capabilities::decode_input(operation, value);
        }
        if access::supports(operation) {
            return access::decode_input(operation, value);
        }
        if subscriptions::errors(operation).is_some() {
            return subscriptions::decode_input(operation, value);
        }
        if configuration::supports(operation) {
            configuration::decode_input(operation, value)
        } else if sessions::supports(operation) {
            sessions::decode_input(operation, value)
        } else if turns::supports(operation) {
            turns::decode_input(operation, value)
        } else {
            bootstrap::Operations.decode_input(operation, value)
        }
    }
    fn decode_output(&self, operation: Operation, value: &Value) -> Result<Value> {
        if maka_protocol::workhub::supports(operation) {
            return maka_protocol::workhub::decode_output(operation, value);
        }
        if operation == Operation::SkillCatalogQuery {
            maka_protocol::skills::decode_catalog_output(value)?;
            return Ok(value.clone());
        }
        if operation == Operation::SkillCatalogInvocableQuery {
            maka_protocol::skills::decode_invocable_output(value)?;
            return Ok(value.clone());
        }
        if maka_protocol::navigation::supports(operation) {
            return maka_protocol::navigation::decode_output(operation, value);
        }
        if maka_protocol::oauth::supports(operation) {
            return maka_protocol::oauth::decode_output(operation, value);
        }
        if operation == Operation::SessionExecutionBoundaryQuery {
            maka_protocol::execution_boundary::decode_output(value)?;
            return Ok(value.clone());
        }
        if super::onboarding::supports(operation) {
            return super::onboarding::decode_output(operation, value);
        }
        if super::projects::supports(operation) {
            return super::projects::decode_output(operation, value);
        }
        if super::messages::supports(operation) {
            maka_protocol::message::decode_output(operation, value)?;
            return Ok(value.clone());
        }
        if operation == Operation::SubscriptionPtyInterestSet {
            maka_protocol::subscription::decode_subscription_close_result(value)?;
            return Ok(value.clone());
        }
        if maka_protocol::resource::is_controller(operation) {
            maka_protocol::resource::validate_controller_output(operation, value)?;
            return Ok(value.clone());
        }
        if matches!(
            operation,
            Operation::RuntimeResourceStart | Operation::RuntimeResourceStop
        ) {
            maka_protocol::resource::decode_mutation_result(value)?;
            return Ok(value.clone());
        }
        if operation == Operation::RuntimeResourceQuery {
            return serde_json::to_value(maka_protocol::resource::decode_query_result(value)?)
                .map_err(|e| maka_protocol::ProtocolError::invalid(e.to_string()));
        }
        if context::supports(operation) {
            return context::decode_output(operation, value);
        }
        if artifacts::supports(operation) {
            return artifacts::decode_output(operation, value);
        }
        if interactions::supports(operation) {
            return interactions::decode_output(operation, value);
        }
        if capabilities::supports(operation) {
            return capabilities::decode_output(operation, value);
        }
        if access::supports(operation) {
            return access::decode_output(operation, value);
        }
        if configuration::supports(operation) {
            configuration::decode_output(operation, value)
        } else if sessions::supports(operation) {
            sessions::decode_output(operation, value)
        } else if turns::supports(operation) {
            turns::decode_output(operation, value)
        } else {
            bootstrap::Operations.decode_output(operation, value)
        }
    }
    fn error_codes(&self, operation: Operation) -> Option<&[OperationErrorCode]> {
        if maka_protocol::workhub::supports(operation) {
            if matches!(
                operation,
                Operation::WorkhubCoordinationActFromTurn
                    | Operation::WorkhubCoordinationSelectAndDelegate
            ) {
                return Some(super::workhub::ACTION_ERRORS);
            }
            if operation == Operation::WorkhubCoordinationCandidates {
                return Some(super::workhub::CANDIDATE_ERRORS);
            }
            if operation == Operation::WorkhubCoordinationAnswer {
                return Some(super::workhub::TURN_ERRORS);
            }
            return Some(
                if operation == Operation::WorkhubCoordinationConfigureModel {
                    sessions::configuration::ERRORS
                } else {
                    super::workhub::ERRORS
                },
            );
        }
        if operation == Operation::SkillCatalogQuery {
            return Some(super::skills::sources::ERRORS);
        }
        if operation == Operation::SkillCatalogInvocableQuery {
            return Some(super::skills::ERRORS);
        }
        if maka_protocol::navigation::supports(operation) {
            return Some(super::navigation::ERRORS);
        }
        if let Some(errors) = maka_protocol::oauth::errors(operation) {
            return Some(errors);
        }
        if operation == Operation::SessionExecutionBoundaryQuery {
            return Some(super::execution_boundary::ERRORS);
        }
        if super::onboarding::supports(operation) {
            return Some(configuration::MUTATION_ERRORS);
        }
        if super::projects::supports(operation) {
            return Some(if operation == Operation::ProjectCatalogQuery {
                super::projects::QUERY_ERRORS
            } else {
                super::projects::MUTATION_ERRORS
            });
        }
        if super::messages::supports(operation) {
            return Some(super::messages::ERRORS);
        }
        if maka_protocol::resource::is_controller(operation) {
            return Some(maka_protocol::resource::MUTATION_ERRORS);
        }
        if matches!(
            operation,
            Operation::RuntimeResourceStart | Operation::RuntimeResourceStop
        ) {
            return Some(maka_protocol::resource::MUTATION_ERRORS);
        }
        if operation == Operation::RuntimeResourceQuery {
            return Some(maka_protocol::resource::QUERY_ERRORS);
        }
        if operation == Operation::RuntimePolicyQuery {
            return Some(maka_protocol::runtime_policy::QUERY_ERRORS);
        }
        if operation == Operation::NetworkProxyTest {
            return Some(maka_protocol::network_proxy::ERRORS);
        }
        if let Some(errors) = context::errors(operation) {
            return Some(errors);
        }
        if let Some(errors) = artifacts::errors(operation) {
            return Some(errors);
        }
        if let Some(errors) = interactions::errors(operation) {
            return Some(errors);
        }
        if capabilities::supports(operation) {
            return Some(capabilities::ERRORS);
        }
        if access::supports(operation) {
            return Some(configuration::MUTATION_ERRORS);
        }
        if let Some(errors) = subscriptions::errors(operation) {
            return Some(errors);
        }
        if configuration::supports(operation) {
            Some(if operation.mode() == maka_protocol::OperationMode::Query {
                configuration::QUERY_ERRORS
            } else {
                configuration::MUTATION_ERRORS
            })
        } else {
            match operation {
                Operation::SessionCreate => Some(sessions::CREATE_ERRORS),
                Operation::SessionCatalogQuery => Some(sessions::QUERY_ERRORS),
                Operation::SessionLifecycleSet => Some(sessions::LIFECYCLE_ERRORS),
                Operation::SessionMetadataUpdate => Some(sessions::mutation::METADATA_ERRORS),
                Operation::SessionReadMarkerSet => Some(sessions::mutation::READ_MARKER_ERRORS),
                Operation::SessionConfigurationUpdate | Operation::SessionWorkspaceRelocate => {
                    Some(sessions::configuration::ERRORS)
                }
                _ => turns::errors(operation)
                    .or_else(|| bootstrap::Operations.error_codes(operation)),
            }
        }
    }
}
