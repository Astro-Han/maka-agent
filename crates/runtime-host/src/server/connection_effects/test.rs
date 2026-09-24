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

use super::{ConnectionEffects, Host, ProviderOperation, configuration};
use maka_config::connection_test::ConnectionTestPreparation;
use maka_protocol::{OperationError, OperationErrorCode};
use maka_runtime::configuration::{
    ConnectionCatalogEntry, ConnectionEffectFailureClass as Failure, ConnectionTestProjection,
    ConnectionTestRunInput, ConnectionTestRunResult, ModelInfo,
};

impl ConnectionEffects {
    pub async fn test(
        &self,
        host: &Host,
        input: ConnectionTestRunInput,
    ) -> Result<ConnectionTestRunResult, OperationError> {
        let _lane = self.lane(&input.connection_id).await;
        let admission = host.executions.lock_admission().await;
        let mut prepared = match host
            .configuration
            .prepare_connection_test(input)
            .await
            .map_err(configuration::failure)?
        {
            ConnectionTestPreparation::Ready(prepared) => prepared,
            ConnectionTestPreparation::Rejected(reason) => {
                return Ok(ConnectionTestRunResult::Rejected { reason });
            }
        };
        let operation = ProviderOperation::prepare(
            host,
            prepared.connection(),
            prepared
                .provider_credential()
                .map_err(configuration::failure)?,
            prepared.network_configuration(),
        )?;
        drop(admission);
        let start = std::time::Instant::now();
        let credential = operation.credential().await?;
        if let Some(credential) = &credential {
            prepared
                .accept_credential(credential.clone())
                .map_err(configuration::failure)?;
        }
        let model = select_model(prepared.connection(), prepared.model_id());
        let model_id = model.as_ref().map(|model| model.id.clone());
        let result = match model {
            Some(model) => {
                operation
                    .verify(
                        prepared.connection(),
                        model,
                        credential.as_ref(),
                        prepared.request_headers(),
                    )
                    .await
            }
            None => Err(Failure::InvalidResponse.into()),
        };
        let latency_ms = u64::try_from(start.elapsed().as_millis())
            .expect("bounded connection verification duration");
        let checked_at = time::OffsetDateTime::now_utc()
            .format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
            ))
            .map_err(|_| OperationError {
                code: OperationErrorCode::InternalFailure,
                message: "Cannot format connection test timestamp".into(),
            })?;
        let test = match result {
            Ok(()) => ConnectionTestProjection::Verified {
                checked_at,
                model_id: model_id.expect("verified model"),
                latency_ms,
            },
            Err(failure) => ConnectionTestProjection::Failed {
                checked_at,
                model_id,
                latency_ms: Some(latency_ms),
                status_code: failure.status.map(u64::from),
                error_class: failure.class,
            },
        };
        prepared
            .complete(test)
            .await
            .map_err(configuration::failure)
    }
}

/// Prefer enabled inventory models, without hiding manually configured IDs.
fn select_model(row: &ConnectionCatalogEntry, explicit: Option<&str>) -> Option<ModelInfo> {
    let id = explicit
        .or_else(|| {
            row.enabled_model_ids
                .iter()
                .find(|id| row.models.iter().any(|model| model.id == **id))
                .map(String::as_str)
        })
        .or_else(|| row.enabled_model_ids.first().map(String::as_str))
        .or_else(|| row.models.first().map(|model| model.id.as_str()))?;
    Some(
        row.models
            .iter()
            .find(|model| model.id == id)
            .cloned()
            .unwrap_or_else(|| ModelInfo::new(id)),
    )
}
