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

use super::{Host, HostError, failure};
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome, skills::*};
use serde_json::Value;

pub(in crate::server) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];

pub(in crate::server) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
) -> Result<Outcome, HostError> {
    if host.draining.is_cancelled() {
        return Ok(Outcome::failure(failure(
            Code::HostDraining,
            "Host is draining",
        )));
    }
    let result = match operation {
        Operation::SkillSourceImport => {
            let input = decode_import_input(value)?;
            let import = async {
                let result = service(host)?
                    .value
                    .import_source(input)
                    .await
                    .map_err(crate::execution::skills::skill_error)?;
                serde_json::to_value(result)
                    .map_err(|e| failure(Code::CommitOutcomeUnknown, &e.to_string()))
            };
            import.await
        }
        Operation::SkillCatalogQuery => {
            let input = decode_catalog_input(value)?;
            query(host, &input).await
        }
        Operation::SkillCatalogMutate => {
            let input = decode_mutate_input(value)?;
            mutate(host, input).await
        }
        Operation::SkillCatalogPreviewUpdate => {
            let input = decode_preview_input(value)?;
            preview(host, &input).await
        }
        Operation::SkillCatalogResolvePath => {
            let input = decode_path_input(value)?;
            let resolved = async {
                let workspace = resolve(host, &input.context).await?;
                let result = service(host)?
                    .value
                    .resolve_path(&input, &workspace)
                    .await
                    .map_err(crate::execution::skills::skill_error)?;
                serde_json::to_value(result)
                    .map_err(|e| failure(Code::InternalFailure, &e.to_string()))
            };
            resolved.await
        }
        _ => unreachable!("Skills catalog router"),
    };
    match result {
        Err(error) => Ok(Outcome::failure(error)),
        Ok(value) => {
            match operation {
                Operation::SkillSourceImport => {
                    decode_import_output(&value)?;
                }
                Operation::SkillCatalogQuery => {
                    decode_catalog_output(&value)?;
                }
                Operation::SkillCatalogMutate => {
                    decode_mutate_output(&value)?;
                }
                Operation::SkillCatalogPreviewUpdate => {
                    decode_preview_output(&value)?;
                }
                Operation::SkillCatalogResolvePath => {
                    decode_path_output(&value)?;
                }
                _ => unreachable!(),
            }
            Ok(Outcome::success(value))
        }
    }
}
async fn resolve(
    host: &Host,
    context: &WorkspaceContext,
) -> Result<maka_runtime::execution::WorkspaceProjection, OperationError> {
    super::super::sessions::workspace::resolve(host, &context.workspace)
        .await
        .map_err(|mut e| {
            if matches!(e.code, Code::OperationConflict | Code::NotFound) {
                e.code = Code::OperationUnavailable;
            }
            e
        })
}
fn service(
    host: &Host,
) -> Result<maka_plugins::contributions::Contribution<maka_skills::plugin::Skills>, OperationError>
{
    host.executions
        .skills()
        .ok_or_else(|| failure(Code::OperationUnavailable, "Skills plugin is not active"))
}
async fn query(host: &Host, input: &CatalogInput) -> Result<Value, OperationError> {
    let workspace = resolve(host, input.context()).await?;
    let result = service(host)?
        .value
        .query(input, workspace)
        .await
        .map_err(crate::execution::skills::skill_error)?;
    serde_json::to_value(result).map_err(|e| failure(Code::InternalFailure, &e.to_string()))
}
async fn mutate(host: &Host, input: MutateInput) -> Result<Value, OperationError> {
    let workspace = resolve(host, &input.context).await?;
    let result = service(host)?
        .value
        .mutate(input, workspace)
        .await
        .map_err(crate::execution::skills::skill_error)?;
    serde_json::to_value(result).map_err(|e| failure(Code::CommitOutcomeUnknown, &e.to_string()))
}
async fn preview(host: &Host, input: &PreviewInput) -> Result<Value, OperationError> {
    let workspace = resolve(host, &input.context).await?;
    let result = service(host)?
        .value
        .preview_update(input, workspace)
        .await
        .map_err(crate::execution::skills::skill_error)?;
    serde_json::to_value(result).map_err(|e| failure(Code::InternalFailure, &e.to_string()))
}
