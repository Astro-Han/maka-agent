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

use super::*;

pub(super) fn identity(turn: &str) -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: turn.into(),
        run_id: format!("run-{turn}"),
        invocation_id: format!("inv-{turn}"),
    }
}

pub(super) async fn append(log: &EventLog, invocation: &Invocation, fact: Fact) -> StoredEvent {
    let event = RuntimeEvent::new(invocation.clone(), fact);
    let sequence = log
        .append(&EventWrite::plain(event.clone()).unwrap())
        .await
        .unwrap();
    StoredEvent { sequence, event }
}

pub(super) async fn hidden(
    log: &EventLog,
    view: &mut InvocationView,
    invocation: &Invocation,
    fact: Fact,
) -> StoredEvent {
    let stored = append(log, invocation, fact).await;
    assert!(view.push(&stored).unwrap().is_empty());
    assert!(view.overlay().is_empty());
    stored
}

pub(super) fn observation(event: ModelEvent) -> Fact {
    Fact::ModelObserved {
        step_id: "summary".into(),
        event,
    }
}

pub(super) fn accepted(text: &str) -> Fact {
    Fact::ModelCompleted {
        step_id: "summary".into(),
        output: ModelStep {
            parts: vec![ModelPart::Text {
                text_kind: TextKind::Text,
                text: text.into(),
                provider_options: None,
            }],
            finish_reason: ModelFinishReason::Stop,
            usage: Default::default(),
            provider_options: None,
            response_id: None,
            model: None,
            timestamp: None,
        },
    }
}

pub(super) fn transcript(db: &Connection) -> Vec<(i64, Vec<u8>, String)> {
    db.prepare("SELECT sequence, payload, digest FROM transcript_rows ORDER BY sequence")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
