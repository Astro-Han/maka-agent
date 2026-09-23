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

use crate::{Client, ClientError, RequestFailure};
use maka_protocol::{Operation, ProtocolError, project::*};

impl Client {
    pub async fn mutate_project(&self, input: Mutation) -> Result<Project, RequestFailure> {
        let value = self
            .request(
                Operation::ProjectCatalogMutate,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let invalid = |error: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
        };
        let MutationResult::Project { project } =
            decode_mutation_result(&value).map_err(invalid)?;
        let id = match &input {
            Mutation::Rename { project_id, .. }
            | Mutation::Archive { project_id }
            | Mutation::Restore { project_id }
            | Mutation::Relink { project_id, .. } => Some(project_id),
            Mutation::Register { .. } | Mutation::RegisterDirectory { .. } => None,
        };
        if id.is_some_and(|id| *id != project.id && !project.aliases.contains(id))
            || matches!(input, Mutation::Archive { .. }) && project.archived_at.is_none()
            || matches!(input, Mutation::Restore { .. }) && project.archived_at.is_some()
        {
            return Err(invalid(ProtocolError::invalid(
                "Project mutation reply does not match request",
            )));
        }
        Ok(project)
    }

    pub async fn project_catalog(&self, input: Query) -> Result<QueryResult, RequestFailure> {
        let value = self
            .request(
                Operation::ProjectCatalogQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let invalid = |error: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
        };
        let output = decode_query_result(&value).map_err(invalid)?;
        assert_query_output(&input, &output).map_err(invalid)?;
        let valid = match (&input, &output) {
            (Query::ListStart { .. }, QueryResult::Page { .. }) => true,
            (
                Query::ListContinue {
                    revision, cursor, ..
                },
                QueryResult::Page {
                    revision: actual,
                    next_cursor,
                    ..
                },
            ) => revision == actual && next_cursor.as_ref() != Some(cursor),
            (
                Query::ListContinue { revision, .. },
                QueryResult::RevisionChanged {
                    expected, actual, ..
                },
            ) => revision == expected && expected != actual,
            (Query::DirectoryRoots, QueryResult::DirectoryRoots { .. }) => true,
            (Query::DirectoryResolve { .. }, QueryResult::DirectoryPath { .. }) => true,
            (
                Query::DirectoryListStart { segments, .. },
                QueryResult::DirectoryPage {
                    segments: actual, ..
                },
            ) => segments == actual,
            (
                Query::DirectoryListContinue {
                    segments, cursor, ..
                },
                QueryResult::DirectoryPage {
                    segments: actual,
                    next_cursor,
                    ..
                },
            ) => segments == actual && next_cursor.as_ref() != Some(cursor),
            _ => false,
        };
        if !valid {
            return Err(invalid(ProtocolError::invalid(
                "Project reply does not match request",
            )));
        }
        let directory_valid = match &output {
            QueryResult::DirectoryRoots { roots } => {
                let mut ids = std::collections::HashSet::new();
                roots.iter().all(|root| ids.insert(&root.id))
            }
            QueryResult::DirectoryPage {
                entries,
                next_cursor,
                ..
            } => {
                let after = match &input {
                    Query::DirectoryListContinue { cursor, .. } => Some(cursor.as_str()),
                    _ => None,
                };
                let mut previous = after;
                entries.iter().all(|entry| {
                    let advances = previous.is_none_or(|previous| {
                        entry
                            .name
                            .encode_utf16()
                            .cmp(previous.encode_utf16())
                            .is_gt()
                    });
                    previous = Some(&entry.name);
                    advances
                }) && next_cursor
                    .as_ref()
                    .is_none_or(|cursor| entries.last().is_some_and(|entry| entry.name == *cursor))
            }
            _ => true,
        };
        if !directory_valid {
            return Err(invalid(ProtocolError::invalid(
                "Directory page does not advance",
            )));
        }
        if let QueryResult::Page {
            items, next_cursor, ..
        } = &output
        {
            let mut ids = std::collections::HashSet::new();
            if (items.is_empty() && next_cursor.is_some())
                || items
                    .iter()
                    .any(|item| matches!(item, PageItem::Project { id, .. } if !ids.insert(id)))
            {
                return Err(invalid(ProtocolError::invalid(
                    "Project page does not advance",
                )));
            }
        }
        Ok(output)
    }
}
