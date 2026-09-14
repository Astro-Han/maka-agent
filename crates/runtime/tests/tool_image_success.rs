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

use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_runtime::{
    artifact::ArtifactSource,
    event::{CommitFuture, EventSink, EventWrite, Fact, Invocation, ToolOutcome},
    tool_call::ToolCallIdentity,
    tool_output::{ToolOutput, ToolSuccess, decode_raw_tool_result},
    tools::{ToolError, ToolJournal},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

struct Sink {
    events: Mutex<Vec<EventWrite>>,
    prepared: Notify,
    acknowledge: Semaphore,
}
impl EventSink for Sink {
    fn commit(self: Arc<Self>, write: EventWrite) -> CommitFuture {
        Box::pin(async move {
            let outcome = matches!(write.event().fact, Fact::ToolSettled { .. });
            self.events.lock().unwrap().push(write);
            if outcome {
                self.prepared.notify_one();
                self.acknowledge.acquire().await.unwrap().forget();
            }
            Ok(self.events.lock().unwrap().len() as u64)
        })
    }
}
fn invocation() -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    }
}
fn sink() -> Arc<Sink> {
    Arc::new(Sink {
        events: Mutex::new(Vec::new()),
        prepared: Notify::new(),
        acknowledge: Semaphore::new(0),
    })
}

#[tokio::test]
async fn pending_image_is_normalized_once_and_delivered_only_after_commit_ack() {
    let bytes = STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
        .unwrap();
    let output = ToolSuccess::image(bytes.clone(), " IMAGE/GIF ".into()).unwrap();
    let sink = sink();
    let journal = ToolJournal::new(sink.clone(), invocation());
    let handle = tokio::spawn(journal.invoke_call_with(
        "operation".into(),
        ToolCallIdentity::standalone("call".into()),
        "Read".into(),
        Value::Null,
        CancellationToken::new(),
        move |_| Box::pin(async move { Ok(output) }),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), sink.prepared.notified())
        .await
        .unwrap();
    assert!(!handle.is_finished());
    let delivered = {
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        let write = &events[1];
        let artifact = &write.projection_artifacts()[0];
        assert_eq!(artifact.bytes(), bytes);
        assert_eq!(
            artifact.artifact().source,
            ArtifactSource::ToolResultProjection
        );
        assert_eq!(artifact.artifact().session_id, "session");
        let Fact::ToolSettled {
            outcome: ToolOutcome::Succeeded { raw, .. },
            ..
        } = &write.event().fact
        else {
            panic!()
        };
        let output = decode_raw_tool_result(write.raw_payload().unwrap(), raw).unwrap();
        assert!(matches!(&output, ToolOutput::Image(image) if image.mime_type == "image/gif"));
        let delivered = output.into_json();
        assert_eq!(delivered["ref"]["relativePath"], artifact.artifact().id);
        assert!(!String::from_utf8_lossy(write.raw_payload().unwrap()).contains("R0lG"));
        delivered
    };
    sink.acknowledge.add_permits(1);
    assert_eq!(handle.await.unwrap().unwrap(), delivered);
}

#[tokio::test]
async fn raw_depth_rejection_after_effect_keeps_outcome_unknown() {
    let sink = sink();
    let mut output = Value::Null;
    for _ in 0..65 {
        output = json!([output]);
    }
    let result = ToolJournal::new(sink.clone(), invocation())
        .invoke_call_with(
            "operation".into(),
            ToolCallIdentity::standalone("call".into()),
            "Read".into(),
            Value::Null,
            CancellationToken::new(),
            move |_| Box::pin(async move { Ok(output) }),
        )
        .await;
    assert!(
        matches!(result, Err(ToolError::OutcomeUnknown(message)) if message.contains("depth limit"))
    );
    assert_eq!(sink.events.lock().unwrap().len(), 1);
}

#[test]
fn pending_image_representation_is_checked_before_success() {
    assert!(ToolSuccess::image(vec![0; 5 * 1024 * 1024], "image/png".into()).is_ok());
    assert!(ToolSuccess::image(vec![0; 5 * 1024 * 1024 + 1], "image/png".into()).is_err());
    assert!(ToolSuccess::image(vec![0], "image/svg+xml".into()).is_err());
}
