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
use maka_protocol::{
    Operation,
    session::{copy, decode_session_create_input, sources},
    turn::{TurnQueryInput, decode_turn_batch_start_input},
};
use serde_json::{Value, json};

#[test]
fn revision_edits_ordered_inputs_preserves_attachments_and_reopens_without_resubmission() {
    let directory = tempfile::tempdir().unwrap();
    let mut host = super::super::candidate::CandidateFixture::new(directory.path().join("root"));
    host.child = Some(
        Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["host", "serve", "--root"])
            .arg(&host.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    host.wait_for_registration();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (client,model)=runtime.block_on(async {
        use tokio::{net::TcpListener,io::AsyncWriteExt};
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client=support::model_client(&host.root,&format!("http://{}/v1",listener.local_addr().unwrap())).await;
        client.create_session(decode_session_create_input(&json!({
            "sessionId":"revision-source","name":"Revision source",
            "workspace":{"kind":"host_path","path":directory.path()},"modelTarget":{"kind":"default"}
        })).unwrap()).await.unwrap();
        for input in [
            json!({"kind":"begin","sessionId":"revision-source","uploadId":"file","name":"note.txt","mimeType":"text/plain","totalBytes":1,
                "contentSha256":maka_runtime::artifact::content_digest(b"x")}),
            json!({"kind":"chunk","sessionId":"revision-source","uploadId":"file","offset":0,"chunkBase64":"eA=="}),
        ] { client.request(Operation::ArtifactIngest,input).await.unwrap(); }
        let artifact=client.request(Operation::ArtifactIngest,json!({"kind":"commit","sessionId":"revision-source","uploadId":"file"})).await.unwrap();
        let model=tokio::spawn(async move {
            let mut bodies=Vec::new();
            for content in ["Original reply.","Revised reply."] {
                let (mut stream,body)=model_request(&listener).await;
                bodies.push(body);
                let frame=json!({"id":"revision-fixture","object":"chat.completion.chunk","model":"fixture-model",
                    "choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]});
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {frame}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
            }
            bodies
        });
        client.start_turn_batch(decode_turn_batch_start_input(&json!({
            "sessionId":"revision-source","turnId":"original-turn","maxSteps":1,"messages":[
                {"content":{"text":"🦀 @a.rs first","inlineReferences":[
                    {"kind":"workspace_file","value":"@a.rs","label":"a.rs","start":3}]}},
                {"content":{"text":"second original","quotes":[{"text":"Keep quotation"}],"attachments":[artifact["attachment"]]}}
            ]
        })).unwrap()).await.unwrap();
        wait_completed(&client,"revision-source","original-turn").await;
        (client,model)
    });
    let original = runtime
        .block_on(client.session("revision-source"))
        .unwrap()
        .unwrap();
    let checkpoint = directory
        .path()
        .join("tui-state")
        .join(&client.identity.root_id)
        .join("default/state.json");
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Revision source");
    tui.click_text("Revision source");
    tui.wait_for("Original reply.");
    tui.send(b"Composer stays here");
    tui.wait_for("Composer stays here");
    tui.click_text("Original reply.");
    tui.send(b"\x10");
    tui.wait_for("Revise this turn");
    tui.click_text("Revise this turn");
    tui.wait_for("Input  1 / 2");
    tui.send("\x1b[200~中文 \x1b[201~".as_bytes());
    tui.wait_for("中文 🦀 @a.rs first");
    tui.send(b"\x1b[6~"); // PageDown selects the next original input.
    tui.wait_for("Input  2 / 2");
    tui.send(b"\x1b[200~edited \x1b[201~");
    tui.wait_for("edited second original");
    tui.resize(80, 24);
    tui.wait_for("edited second original");
    tui.send(b"\x1b[<0;1;1M\x1b[<0;1;1m");
    tui.wait_until(|s| !s.contains("Revise this turn"));
    tui.send(b"\x11");
    tui.finish();
    let saved: Value = serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();
    assert_eq!(saved["version"], 8);
    assert_eq!(saved["revision"]["stage"], "draft");
    assert_eq!(
        saved["revision"]["inputs"][1]["content"]["text"],
        "edited second original"
    );
    let request: copy::Input = serde_json::from_value(saved["revision"]["copy"].clone()).unwrap();
    assert!(
        runtime
            .block_on(client.query_session_copy(request.clone()))
            .unwrap()
            .receipt
            .is_none(),
        "editing has no copy effect"
    );
    let turn = saved["revision"]["turn_id"].as_str().unwrap().to_owned();
    let mut reopened = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    reopened.wait_for("Composer stays here");
    reopened.wait_for("Original reply.");
    reopened.send(b"\x10");
    reopened.wait_for("Continue revision");
    reopened.click_text("Continue revision");
    reopened.wait_for("中文 🦀 @a.rs first");
    reopened.click_text("Run revision");
    reopened.wait_for("Revision accepted.");
    reopened.send(b"\x11");
    reopened.finish();
    runtime.block_on(wait_completed(&client, &request.target_session_id, &turn));
    let bodies = runtime.block_on(model).unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].to_string().contains("edited second original"));
    let revised = runtime
        .block_on(client.session_turn_sources(sources::Input {
            session_id: request.target_session_id.clone(),
            turn_id: turn,
        }))
        .unwrap();
    assert_eq!(revised.messages.len(), 2);
    assert_eq!(revised.messages[0].content.text, "中文 🦀 @a.rs first");
    assert_eq!(
        revised.messages[0]
            .content
            .inline_references
            .as_ref()
            .unwrap()[0]
            .start,
        6
    );
    assert_eq!(revised.messages[1].content.text, "edited second original");
    assert_eq!(
        revised.messages[1].content.quotes.as_ref().unwrap()[0].text,
        "Keep quotation"
    );
    assert!(
        matches!(&revised.messages[1].content.attachments.as_ref().unwrap()[0].storage_ref,
        maka_protocol::turn::StorageRef::SessionFile{session_id,..} if *session_id==request.target_session_id)
    );
    assert_eq!(
        runtime
            .block_on(client.session("revision-source"))
            .unwrap()
            .unwrap()
            .revision,
        original.revision
    );
    let mut recovered = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    recovered.wait_for("Original reply.");
    recovered.send(b"\x10");
    recovered.wait_for("Continue revision");
    recovered.click_text("Continue revision");
    recovered.wait_for("Result not confirmed.");
    recovered.click_text("Check result");
    recovered.wait_for("Revision accepted.");
    recovered.click_text("Discard revision");
    recovered.wait_for("Discard these edits?");
    recovered.click_text("Discard revision");
    recovered.wait_for("This revision has been used and was kept.");
    recovered.click_text("Open revised session");
    recovered.wait_for("Revised reply.");
    assert!(
        !recovered
            .screen
            .snapshot()
            .unwrap()
            .screen
            .contains("Composer stays here")
    );
    recovered.send(b"\x11");
    recovered.finish();
    let saved: Value = serde_json::from_slice(&std::fs::read(checkpoint).unwrap()).unwrap();
    assert!(saved["revision"].is_null());
    assert_eq!(
        saved["drafts"]["revision-source"]["text"],
        "Composer stays here"
    );
    assert!(
        runtime
            .block_on(client.query_session_copy(request))
            .unwrap()
            .receipt
            .is_some()
    );
    client.disconnect();
    host.retire_registered();
}

async fn wait_completed(client: &maka_client::Client, session: &str, turn: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let snapshot = client
                .query_turn(TurnQueryInput {
                    session_id: session.into(),
                    turn_id: turn.into(),
                })
                .await
                .unwrap();
            let value = serde_json::to_value(snapshot).unwrap();
            match value["status"].as_str() {
                Some("completed") => break,
                Some("failed" | "cancelled") => panic!("fixture turn failed: {value}"),
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .unwrap();
}
