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
use maka_protocol::{Operation, turn::*};

impl Client {
    /// Submit a frozen batch once. Unknown outcomes require an explicit query;
    /// this method never retries or allocates another Turn identity.
    pub async fn start_turn_batch(
        &self,
        input: TurnBatchStartInput,
    ) -> Result<TurnStartResult, RequestFailure> {
        let output = self
            .request(
                Operation::TurnBatchStart,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        if let Ok(output) = decode_turn_start_result(&output)
            && assert_batch_start_output_for_input(&input, &output).is_ok()
        {
            return Ok(output);
        }
        self.disconnect();
        Err(RequestFailure::Unknown(ClientError::Protocol(
            "Batch receipt does not match the requested Turn".into(),
        )))
    }

    /// NotFound is not proof of non-delivery and must not trigger automatic replay.
    pub async fn query_turn(&self, input: TurnQueryInput) -> Result<TurnSnapshot, RequestFailure> {
        let output = self
            .request(
                Operation::TurnQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        if let Ok(output) = decode_turn_snapshot(&output)
            && output.session_id == input.session_id
            && output.turn_id == input.turn_id
        {
            return Ok(output);
        }
        self.disconnect();
        Err(RequestFailure::Unknown(ClientError::Protocol(
            "Turn query returned another Turn".into(),
        )))
    }
}
