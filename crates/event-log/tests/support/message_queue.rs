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

use maka_event_log::message_queue::{QueueCommand, QueueCommandKind};
use maka_event_log::{EventLog, message_admissions::PendingMessageAdmission};
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent},
    input::DeliveredMessage,
    message::{MessageDisposition as Disposition, Placement, RootSourceMessage},
};

// Some queue tests share only the event/admission fixtures below.
#[allow(dead_code)]
pub fn command(id: &str, kind: QueueCommandKind) -> QueueCommand {
    QueueCommand {
        host_epoch: "epoch".into(),
        command_id: id.into(),
        kind,
        fingerprint: format!("sha256:{}", "a".repeat(64)),
    }
}

pub fn invocation(turn: &str) -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: turn.into(),
        run_id: format!("run-{turn}"),
        invocation_id: format!("invocation-{turn}"),
    }
}
pub async fn append(log: &EventLog, invocation: &Invocation, fact: Fact) {
    log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
        .await
        .unwrap();
}
pub fn opening() -> Fact {
    Fact::InvocationOpened {
        configuration: None,
        input: InvocationInput::Message {
            content: "opening".into(),
            request_fingerprint: None,
            source_messages: Vec::new(),
        },
    }
}
pub fn admission(
    invocation: &Invocation,
    id: &str,
    disposition: Disposition,
) -> PendingMessageAdmission {
    PendingMessageAdmission {
        invocation: invocation.clone(),
        steering_invocation: None,
        required_tools: Default::default(),
        admitted_at: 1,
        source: RootSourceMessage {
            message: DeliveredMessage {
                message_id: id.into(),
                content: id.into(),
                submitted_content_digest: format!("sha256:{}", "c".repeat(64)),
            },
            submitted_placement: if disposition == Disposition::Followup {
                Placement::NextTurn
            } else {
                Placement::CurrentTurn
            },
            disposition,
            submitted_intent: None,
        },
    }
}
