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
use super::{encode, failure};
use crate::{
    api::*,
    plugin::{Skills, Snapshot},
};
use maka_plugins::remote::{Error, SessionView};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Request {
    ResolvePath {
        #[serde(rename = "ref")]
        reference: String,
        target: PathTarget,
    },
    Invocable {
        page: Option<Page>,
    },
    Catalog {
        view: CatalogView,
        page: Option<Page>,
    },
    Mutate {
        expected_revision: String,
        mutation: Mutation,
    },
    Preview {
        expected_revision: String,
        #[serde(rename = "ref")]
        reference: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    revision: String,
    cursor: String,
}

impl Request {
    pub(super) async fn execute(
        self,
        skills: &Skills,
        view: SessionView,
        target: InvocableTarget,
    ) -> Result<Value, Error> {
        let context = WorkspaceContext {
            workspace: view.workspace.target.clone(),
        };
        match self {
            Request::ResolvePath { reference, target } => encode(
                skills
                    .resolve_path(
                        &ResolvePathInput {
                            context,
                            reference,
                            target,
                        },
                        &view.workspace,
                    )
                    .await
                    .map_err(failure)?,
            ),
            Request::Invocable { page } => {
                let input = match page {
                    None => InvocableInput::Start { target },
                    Some(Page { revision, cursor }) => InvocableInput::Continue {
                        target,
                        revision,
                        cursor,
                    },
                };
                let snapshot = if view.tools.contains("Skill") {
                    skills
                        .capture(&view.workspace.host_cwd, view.tools)
                        .await
                        .map_err(failure)?
                } else {
                    Snapshot::empty()
                };
                encode(
                    snapshot
                        .invocable(&input, &view.workspace.host_cwd)
                        .map_err(failure)?,
                )
            }
            Request::Catalog {
                view: catalog,
                page,
            } => {
                let input = match page {
                    None => CatalogInput::Start {
                        context,
                        view: catalog,
                    },
                    Some(Page { revision, cursor }) => CatalogInput::Continue {
                        context,
                        view: catalog,
                        revision,
                        cursor,
                    },
                };
                encode(
                    skills
                        .query(&input, view.workspace)
                        .await
                        .map_err(failure)?,
                )
            }
            Request::Mutate {
                expected_revision,
                mutation,
            } => encode(
                skills
                    .mutate(
                        MutateInput {
                            context,
                            expected_revision,
                            mutation,
                        },
                        view.workspace,
                    )
                    .await
                    .map_err(failure)?,
            ),
            Request::Preview {
                expected_revision,
                reference,
            } => encode(
                skills
                    .preview_update(
                        &PreviewInput {
                            context,
                            expected_revision,
                            reference,
                        },
                        view.workspace,
                    )
                    .await
                    .map_err(failure)?,
            ),
        }
    }
}
