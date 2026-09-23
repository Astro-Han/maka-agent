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
use maka_protocol::{Operation, session::decode_session_create_input};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn recap_uses_native_remote_preserves_draft_and_reads_saved_result_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(directory.path())
            .status()
            .unwrap()
            .success()
    );
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
    let calls = Arc::new(AtomicUsize::new(0));
    let (client,model,finish)=runtime.block_on(async {
        use tokio::{net::TcpListener,io::AsyncWriteExt};
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client=support::model_client(&host.root,&format!("http://{}/v1",listener.local_addr().unwrap())).await;
        client.create_session(decode_session_create_input(&json!({
            "sessionId":"recap-session","name":"Recap fixture",
            "workspace":{"kind":"host_path","path":directory.path()},"modelTarget":{"kind":"default"}
        })).unwrap()).await.unwrap();
        let (finish,finished)=tokio::sync::oneshot::channel();
        let count=calls.clone();
        let model=tokio::spawn(async move {
            let mut finished=Some(finished);
            loop {
                let (mut stream,body,_)=model_http_request(&listener).await;
                let n=count.fetch_add(1,Ordering::SeqCst);
                assert!(n<2,"opening, refreshing and restarting must never generate again");
                let content=if n==0 {"Original verified answer."} else {
                    assert!(body.to_string().contains("Original verified answer."));
                    finished.take().unwrap().await.unwrap();
                    "Recap confirmed: original work is ready for the next step."
                };
                if body["stream"]==true {
                    let frame=json!({"id":"recap-fixture","object":"chat.completion.chunk","model":"fixture-model",
                        "choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":10,"completion_tokens":12,"total_tokens":22}});
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {frame}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
                } else {
                    support::json_response(stream,"200 OK",json!({"id":"recap-fixture","object":"chat.completion","model":"fixture-model",
                        "choices":[{"index":0,"message":{"role":"assistant","content":content},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":10,"completion_tokens":12,"total_tokens":22}})).await;
                }
            }
        });
        client.request(Operation::TurnStart,json!({"sessionId":"recap-session","turnId":"original","content":{"text":"Prepare a verified answer"},"maxSteps":1})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(15),async {
            loop {
                let turn=client.request(Operation::TurnQuery,json!({"sessionId":"recap-session","turnId":"original"})).await.unwrap();
                match turn["status"].as_str() {
                    Some("completed")=>break,
                    Some("failed"|"cancelled")=>panic!("fixture turn failed: {turn}"),
                    _=>tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        }).await.unwrap();
        (client,model,finish)
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Recap fixture");
    tui.click_text("Recap fixture");
    tui.wait_for("Original verified answer.");
    tui.click_text("Message…");
    tui.send(b"Keep this unsent draft");
    tui.wait_for("Keep this unsent draft");
    tui.filter_command("Session recap");
    tui.click_text("Session recap");
    tui.wait_for("No recap has been saved");
    tui.send(b"\r"); // default focus closes; it cannot charge the model.
    tui.wait_until(|s| !s.contains("No recap has been saved"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tui.filter_command("Session recap");
    tui.click_text("Session recap");
    tui.wait_for("No recap has been saved");
    tui.click_text("Generate new");
    tui.wait_for("Retry original");
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    });
    let checkpoint = directory
        .path()
        .join("tui-state")
        .join(&client.identity.root_id)
        .join("default/state.json");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(checkpoint).unwrap()).unwrap();
    assert_eq!(saved["recap"]["session"], "recap-session");
    assert!(saved["recap"]["operation"].as_str().is_some());
    tui.resize(32, 9);
    tui.wait_for("enlarge the terminal");
    tui.send(b"\t\t\r"); // hidden generation control must stay disabled.
    tui.resize(120, 40);
    tui.wait_for("Session recap");
    finish.send(()).unwrap();
    tui.wait_for("Recap confirmed:");
    tui.click_text("Refresh recap");
    tui.wait_for("Recap confirmed:");
    // Outside click dismisses only the popup, preserving the underlying composer.
    tui.send(b"\x1b[<0;2;2M\x1b[<0;2;2m");
    tui.wait_until(|s| !s.contains("Recap confirmed:"));
    tui.wait_for("Keep this unsent draft");
    tui.send(b"\x11");
    tui.finish();
    let mut reopened = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    reopened.wait_for("Keep this unsent draft");
    reopened.wait_for("Original verified answer.");
    reopened.filter_command("Session recap");
    reopened.click_text("Session recap");
    reopened.wait_for("Recap confirmed:");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    reopened.send(b"\x11");
    reopened.finish();
    model.abort();
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}
