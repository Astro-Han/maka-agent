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
    message::{ExecutionResolution, Output, QueryInput, SubmitInput, SubmitResult, decode_output},
};
impl Client {
    pub async fn stop_turn(
        &self,
        input: maka_protocol::turn::TurnStopInput,
    ) -> Result<maka_protocol::turn::TurnSnapshot, RequestFailure> {
        let value = self
            .request(
                Operation::TurnStop,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        if let Ok(output) = maka_protocol::turn::decode_turn_snapshot(&value)
            && output.session_id == input.session_id
            && output.turn_id == input.turn_id
            && output.run_id == input.run_id
        {
            return Ok(output);
        }
        self.disconnect();
        Err(RequestFailure::Unknown(ClientError::Protocol(
            "Stop receipt does not match the requested run".into(),
        )))
    }
    /// A missing resolution is not proof of non-delivery. This read never retries a submit.
    pub async fn message_execution(
        &self,
        session: &str,
        message: &str,
    ) -> Result<Option<ExecutionResolution>, RequestFailure> {
        let value = self
            .request(
                Operation::TurnMessageExecutionQuery,
                serde_json::to_value(QueryInput {
                    session_id: session.into(),
                    message_ids: vec![message.into()],
                })
                .expect("wire input"),
            )
            .await?;
        if let Ok(Output::Executions(mut output)) =
            decode_output(Operation::TurnMessageExecutionQuery, &value)
            && output
                .resolutions
                .iter()
                .all(|resolution| match resolution {
                    ExecutionResolution::NotAdmitted { message_id }
                    | ExecutionResolution::Pending { message_id }
                    | ExecutionResolution::Cancelled { message_id }
                    | ExecutionResolution::Owned { message_id, .. } => message_id == message,
                })
        {
            return Ok(output.resolutions.pop());
        }
        self.disconnect();
        Err(RequestFailure::Unknown(ClientError::Protocol(
            "Execution resolution does not match message query".into(),
        )))
    }

    pub async fn submit_message(&self, input: SubmitInput) -> Result<SubmitResult, RequestFailure> {
        let value = self
            .request(
                Operation::TurnMessageSubmit,
                serde_json::to_value(input).expect("wire input"),
            )
            .await?;
        match decode_output(Operation::TurnMessageSubmit, &value) {
            Ok(Output::Submit(output)) => Ok(output),
            _ => {
                self.disconnect();
                Err(RequestFailure::Unknown(ClientError::Protocol(
                    "Invalid submit result".into(),
                )))
            }
        }
    }
}
