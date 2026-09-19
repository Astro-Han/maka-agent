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

use super::{ID, InputPreparation, Skills};
use futures_util::future::BoxFuture;
use maka_plugins::{
    Error,
    input::{Outcome, Provider, Request},
};

impl Provider for Skills {
    fn prepare(&self, request: Request) -> BoxFuture<'static, Result<Outcome, Error>> {
        let skills = self.clone();
        Box::pin(async move {
            let ids = request.selections.get(ID).cloned().unwrap_or_default();
            if ids.is_empty()
                && !request.content.text.contains("/skill:")
                && !request
                    .content
                    .inline_references
                    .iter()
                    .flatten()
                    .any(|reference| {
                        reference.kind == maka_runtime::input::InlineReferenceKind::Skill
                    })
            {
                return Ok(Outcome::Unchanged);
            }
            let snapshot = skills
                .capture(&request.cwd, request.tools.into_iter().collect())
                .await
                .map_err(|error| Error::Invalid(error.to_string()))?;
            let mut content = request.content;
            let selection = snapshot
                .prepare(&mut content, &ids)
                .map_err(|error| Error::Invalid(error.to_string()))?;
            match selection {
                InputPreparation::Ready {
                    skill_invocation,
                    required_tools,
                } => {
                    if skill_invocation.is_empty() {
                        return Ok(Outcome::Unchanged);
                    }
                    Ok(Outcome::Ready {
                        content,
                        receipt: serde_json::to_value(skill_invocation)
                            .map_err(|error| Error::Invalid(error.to_string()))?,
                        required_tools,
                        basis: snapshot.input_basis,
                    })
                }
                InputPreparation::Blocked(result) => Ok(Outcome::Blocked {
                    message: "Requested Skills are unavailable".into(),
                    receipt: serde_json::to_value(result)
                        .map_err(|error| Error::Invalid(error.to_string()))?,
                }),
            }
        })
    }
}
