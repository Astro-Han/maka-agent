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

use super::{Recall, reader::failed};
use maka_plugins::{
    call::Scope,
    filesystem::{Operation, Output},
    session::history::CopyMaterial,
};
use maka_runtime::{
    read::ReadInput,
    tool_output::{ImageOutput, ToolOutput, ToolSuccess},
    tools::ToolError,
};

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    pub session_id: String,
    pub artifact_id: String,
    pub offset: Option<usize>,
    pub limit: Option<std::num::NonZeroUsize>,
}
impl Input {
    pub fn validate(&self) -> Result<(), String> {
        for id in [&self.session_id, &self.artifact_id] {
            maka_runtime::interaction::entity_id(id).map_err(str::to_owned)?;
        }
        ReadInput {
            path: self.artifact_id.clone(),
            offset: self.offset,
            limit: self.limit,
        }
        .resolve()
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}
impl Recall {
    pub(super) async fn material(
        &self,
        call: Scope,
        input: Input,
    ) -> Result<ToolSuccess, ToolError> {
        let target_session_id = call
            .identity
            .agent()
            .ok_or_else(|| failed("RecallMaterial requires an Agent call"))?
            .session_id
            .clone();
        let target = self
            .executions
            .acquire(call.clone())
            .await
            .map_err(failed)?;
        let attachment = self
            .history
            .copy_material(
                call.clone(),
                target,
                CopyMaterial {
                    session_id: input.session_id,
                    artifact_id: input.artifact_id,
                    target_session_id,
                },
            )
            .await
            .map_err(failed)?;
        let path = attachment
            .storage_ref
            .resource_ref()
            .ok_or_else(|| failed("material is not a Session attachment"))?;
        let read = ReadInput {
            path,
            offset: input.offset,
            limit: input.limit,
        };
        let request = read.resolve().map_err(failed)?;
        let output = self.files.invoke(call, Operation::Read(read)).await?;
        self.check_privacy().await?;
        match output {
            Output::Value(mut value) if value["kind"] == "image" => {
                value.as_object_mut().expect("image object").remove("kind");
                let image: ImageOutput = serde_json::from_value(value).map_err(failed)?;
                Ok(ToolOutput::Image(image).into())
            }
            Output::Value(value) => {
                let text = value
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| failed("unexpected material read result"))?;
                let page = request.page(text).map_err(failed)?;
                // Files exposes raw evidence for programs; models receive a bounded
                // Read page, including a continuation into this Session's own copy.
                Ok(serde_json::to_value(page).map_err(failed)?.into())
            }
            Output::Image { bytes, mime_type } => {
                ToolSuccess::image(bytes, mime_type).map_err(failed)
            }
            Output::Entries(_) => Err(failed("unexpected material read result")),
        }
    }
}
