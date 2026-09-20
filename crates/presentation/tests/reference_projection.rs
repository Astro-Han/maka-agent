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

use maka_presentation::{InvocationView, ProjectionError};
use maka_runtime::{
    event::{Fact, Invocation, RuntimeEvent, StoredEvent},
    input::{InvocationInput, MessageInput},
};
use serde_json::json;

#[test]
fn reference_text_is_bounded_before_presentation_and_poison_does_not_publish_it() {
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    for (field, references) in [
        (
            "attachments",
            json!([{"kind":"other","name":"😀".repeat(33),"mimeType":"x","bytes":0,
            "ref":{"kind":"session_file","sessionId":"session","relativePath":"artifact"}}]),
        ),
        ("quotes", json!([{"text":"short","label":"😀".repeat(33)}])),
        (
            "directory_references",
            json!([{"hostId":"root","path":format!("/{}", "x".repeat(128))}]),
        ),
        (
            "inline_references",
            json!([{"kind":"workspace_file","value":"@file","label":"x".repeat(129),"start":0}]),
        ),
    ] {
        let mut input = json!({"text":"@file","display_text":null});
        input[field] = references;
        let input: MessageInput = serde_json::from_value(input).unwrap();
        let event = StoredEvent {
            sequence: 1,
            event: RuntimeEvent::new(
                invocation.clone(),
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        source_messages: Vec::new(),
                        content: input,
                        request_fingerprint: None,
                    },
                },
            ),
        };
        let mut view = InvocationView::new(128).unwrap();
        assert!(
            matches!(view.push(&event), Err(ProjectionError::TooLarge)),
            "{field}"
        );
        assert!(view.overlay().is_empty());
        assert!(
            view.push(&event).is_err(),
            "cannot resume a poisoned projection"
        );
    }
}
