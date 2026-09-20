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

use maka_event_log::EventLog;
use maka_runtime::event::{Fact, Invocation, LogScope, RuntimeEvent};

pub fn invocation() -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    }
}

pub async fn append(log: &EventLog, fact: Fact) -> RuntimeEvent {
    let event = RuntimeEvent::new(invocation(), fact);
    log.append(&maka_runtime::event::EventWrite::plain(event.clone()).unwrap())
        .await
        .unwrap();
    event
}

pub async fn opening(log: &EventLog) -> RuntimeEvent {
    append(
        log,
        Fact::InvocationOpened {
            configuration: None,
            input: maka_runtime::input::InvocationInput::Message {
                source_messages: Vec::new(),
                content: "perform the operation".into(),
                request_fingerprint: Some("stable".into()),
            },
        },
    )
    .await
}

pub async fn request(log: &EventLog) {
    let prefix = log
        .scoped_prefix(
            LogScope::Session {
                id: "session".into(),
            },
            100,
            128 * 1024,
        )
        .await
        .unwrap();
    append(
        log,
        Fact::ModelRequested {
            effective_source_digest: None,
            purpose: maka_runtime::context::ModelPurpose::Main,
            context: None,
            checkpoint_event_id: None,
            step_id: "step".into(),
            model_id: "test".into(),
            source_scope: prefix.scope,
            source_high_water: prefix.high_water,
            source_digest: prefix.digest,
            input_digest: "input".into(),
            route_identity: "route".into(),
        },
    )
    .await;
}
