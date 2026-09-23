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
use maka_protocol::{Operation, ProtocolError, session::*};

mod copy;

impl Client {
    pub async fn session_catalog(
        &self,
        input: SessionCatalogQueryInput,
    ) -> Result<SessionCatalogQueryResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionCatalogQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output =
            decode_session_catalog_query_result(&value).map_err(|e| self.invalid_session(e))?;
        let valid = match (&input, &output) {
            (
                SessionCatalogQueryInput::Get { session_id },
                SessionCatalogQueryResult::Session { session },
            ) => session.as_ref().is_none_or(|s| s.id == *session_id),
            (
                SessionCatalogQueryInput::ListStart | SessionCatalogQueryInput::PendingStart,
                SessionCatalogQueryResult::Page { .. },
            ) => true,
            (
                SessionCatalogQueryInput::ListContinue { revision, .. }
                | SessionCatalogQueryInput::PendingContinue { revision, .. },
                SessionCatalogQueryResult::Page {
                    revision: actual, ..
                },
            ) => revision == actual,
            (
                SessionCatalogQueryInput::ListContinue { revision, .. }
                | SessionCatalogQueryInput::PendingContinue { revision, .. },
                SessionCatalogQueryResult::RevisionChanged {
                    expected_revision,
                    actual_revision,
                },
            ) => revision == expected_revision && expected_revision != actual_revision,
            _ => false,
        };
        if !valid {
            return Err(self.invalid_session(ProtocolError::invalid(
                "Session catalog result does not match request",
            )));
        }
        if let SessionCatalogQueryResult::Page {
            sessions,
            next_cursor,
            ..
        } = &output
        {
            let mut ids = std::collections::HashSet::new();
            if sessions.iter().any(|session| !ids.insert(&session.id))
                || next_cursor.is_some() && sessions.is_empty()
                || matches!(&input, SessionCatalogQueryInput::ListContinue { cursor, .. } | SessionCatalogQueryInput::PendingContinue { cursor, .. } if next_cursor.as_ref() == Some(cursor))
                || matches!(
                    input,
                    SessionCatalogQueryInput::PendingStart
                        | SessionCatalogQueryInput::PendingContinue { .. }
                ) && sessions
                    .iter()
                    .any(|session| session.status != SessionStatus::WaitingForUser)
            {
                return Err(self.invalid_session(ProtocolError::invalid(
                    "Session catalog page does not advance",
                )));
            }
        }
        Ok(output)
    }

    pub async fn session(
        &self,
        id: &str,
    ) -> Result<Option<Box<SessionCatalogProjection>>, RequestFailure> {
        match self
            .session_catalog(SessionCatalogQueryInput::Get {
                session_id: id.into(),
            })
            .await?
        {
            SessionCatalogQueryResult::Session { session } => Ok(session),
            _ => unreachable!("session_catalog enforces the response variant"),
        }
    }

    pub async fn create_session(
        &self,
        input: SessionCreateInput,
    ) -> Result<SessionCatalogProjection, RequestFailure> {
        let value = self
            .request(
                Operation::SessionCreate,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output =
            decode_session_catalog_projection(&value).map_err(|e| self.invalid_session(e))?;
        assert_create_output_for_input(&input, &output).map_err(|e| self.invalid_session(e))?;
        Ok(output)
    }

    pub async fn update_session_metadata(
        &self,
        input: SessionMetadataUpdateInput,
    ) -> Result<SessionUpdateResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionMetadataUpdate,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output = decode_session_update_result(&value).map_err(|e| self.invalid_session(e))?;
        assert_metadata_update_output_for_input(&input, &output)
            .map_err(|e| self.invalid_session(e))?;
        Ok(output)
    }

    pub async fn relocate_session_workspace(
        &self,
        input: SessionWorkspaceRelocateInput,
    ) -> Result<SessionUpdateResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionWorkspaceRelocate,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output = decode_session_update_result(&value).map_err(|e| self.invalid_session(e))?;
        assert_workspace_relocate_output_for_input(&input, &output)
            .map_err(|e| self.invalid_session(e))?;
        Ok(output)
    }

    pub async fn update_session_configuration(
        &self,
        input: SessionConfigurationUpdateInput,
    ) -> Result<SessionUpdateResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionConfigurationUpdate,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output = decode_session_update_result(&value).map_err(|e| self.invalid_session(e))?;
        assert_configuration_update_output_for_input(&input, &output)
            .map_err(|e| self.invalid_session(e))?;
        Ok(output)
    }

    pub async fn set_session_lifecycle(
        &self,
        input: SessionLifecycleSetInput,
    ) -> Result<SessionCatalogProjection, RequestFailure> {
        let value = self
            .request(
                Operation::SessionLifecycleSet,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output =
            decode_session_catalog_projection(&value).map_err(|e| self.invalid_session(e))?;
        assert_lifecycle_output_for_input(&input, &output).map_err(|e| self.invalid_session(e))?;
        Ok(output)
    }

    pub async fn preview_session_removal(
        &self,
        input: SessionRemovePreviewInput,
    ) -> Result<SessionRemovePreviewResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionRemovePreview,
                serde_json::to_value(input).expect("wire input"),
            )
            .await?;
        decode_session_remove_preview_result(&value).map_err(|e| self.invalid_session(e))
    }

    pub async fn remove_session(
        &self,
        input: SessionRemoveInput,
    ) -> Result<SessionRemoveResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionRemove,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output = decode_session_remove_result(&value).map_err(|e| self.invalid_session(e))?;
        assert_remove_output_for_input(&input, &output).map_err(|e| self.invalid_session(e))?;
        Ok(output)
    }

    /// Read the durable receipt without resubmitting a possibly admitted removal.
    /// `Missing` alone does not prove that an earlier request was not admitted.
    pub async fn query_session_removal(
        &self,
        input: SessionRemoveQueryInput,
    ) -> Result<SessionRemoveQueryResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionRemoveQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output =
            decode_session_remove_query_result(&value).map_err(|e| self.invalid_session(e))?;
        if let SessionRemoveQueryResult::Removed { session_id, .. } = &output
            && *session_id != input.session_id
        {
            return Err(self.invalid_session(ProtocolError::invalid(
                "Session removal receipt does not match request",
            )));
        }
        Ok(output)
    }

    fn invalid_session(&self, error: ProtocolError) -> RequestFailure {
        self.disconnect();
        RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
    }
}
