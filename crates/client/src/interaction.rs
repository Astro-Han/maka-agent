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
use maka_protocol::{
    Operation,
    interaction::{
        self, InteractionAnswer, InteractionAnswerInput, InteractionQueryInput, InteractionSnapshot,
    },
};

impl Client {
    /// Read the original interaction, including its terminal outcome. Never retries an answer.
    pub async fn interaction(
        &self,
        expected: &InteractionSnapshot,
    ) -> Result<InteractionSnapshot, RequestFailure> {
        let value = self
            .request(
                Operation::InteractionQuery,
                serde_json::to_value(InteractionQueryInput {
                    session_id: expected.session_id().into(),
                    interaction_id: expected.interaction_id().into(),
                })
                .expect("wire input"),
            )
            .await?;
        self.checked_interaction(expected, interaction::decode_snapshot(&value))
    }

    pub async fn answer_interaction(
        &self,
        expected: &InteractionSnapshot,
        answer: InteractionAnswer,
    ) -> Result<InteractionSnapshot, RequestFailure> {
        if !expected.is_pending() {
            return Err(RequestFailure::NotDispatched(ClientError::Protocol(
                "Interaction is no longer pending".into(),
            )));
        }
        answer
            .validate_for_request(expected.request())
            .map_err(|error| RequestFailure::NotDispatched(ClientError::Protocol(error.into())))?;
        let value = self
            .request(
                Operation::InteractionAnswer,
                serde_json::to_value(InteractionAnswerInput {
                    session_id: expected.session_id().into(),
                    interaction_id: expected.interaction_id().into(),
                    answer: answer.clone(),
                })
                .expect("wire input"),
            )
            .await?;
        let result =
            self.checked_interaction(expected, interaction::decode_answered_snapshot(&value))?;
        if result
            .outcome()
            .is_some_and(|outcome| answer.matches_outcome(outcome))
        {
            return Ok(result);
        }
        self.disconnect();
        Err(RequestFailure::Unknown(ClientError::Protocol(
            "Interaction receipt does not match the submitted answer".into(),
        )))
    }

    fn checked_interaction(
        &self,
        expected: &InteractionSnapshot,
        result: maka_protocol::Result<InteractionSnapshot>,
    ) -> Result<InteractionSnapshot, RequestFailure> {
        if let Ok(snapshot) = result
            && snapshot.session_id() == expected.session_id()
            && snapshot.interaction_id() == expected.interaction_id()
            && snapshot.turn_id() == expected.turn_id()
            && snapshot.run_id() == expected.run_id()
            && snapshot.request() == expected.request()
        {
            return Ok(snapshot);
        }
        self.disconnect();
        Err(RequestFailure::Unknown(ClientError::Protocol(
            "Interaction reply changed the original identity or request".into(),
        )))
    }
}
