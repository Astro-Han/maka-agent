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

use crate::{Error, Repository, invalid, repository::digest};
use maka_plugins::execution::{Commands, Enqueue, MessageReceipt};
use maka_runtime::{input::MessageInput, message::Placement};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    expected_turn_id: String,
    message_id: String,
    content: MessageInput,
    placement: Placement,
}
#[derive(Deserialize, Serialize)]
struct Intent {
    fingerprint: String,
    request: Enqueue,
}

pub(super) async fn submit(
    repository: &Repository,
    commands: &dyn Commands,
    session: String,
    input: Request,
) -> Result<MessageReceipt, Error> {
    let fingerprint = digest(&(&session, &input))?;
    let key = format!("queue/{}", digest(&input.message_id)?);
    let intent = match repository.read::<Intent>(&key).await? {
        Some((_, intent)) => intent,
        None => {
            let invocation = commands
                .activity(session)
                .await?
                .execution
                .filter(|execution| execution.invocation.turn_id == input.expected_turn_id)
                .ok_or(Error::Conflict)?
                .invocation;
            let request = Enqueue {
                operation_id: input.message_id.clone(),
                message_id: input.message_id,
                invocation,
                content: input.content,
                placement: input.placement,
            };
            request.validate().map_err(invalid)?;
            let intent = Intent {
                fingerprint: fingerprint.clone(),
                request,
            };
            match repository.put(&key, None, &intent).await {
                Ok(()) => intent,
                Err(Error::Contended) => repository.read(&key).await?.ok_or(Error::Conflict)?.1,
                Err(error) => return Err(error),
            }
        }
    };
    if intent.fingerprint != fingerprint {
        return Err(Error::Conflict);
    }
    Ok(commands.enqueue(intent.request).await?)
}
