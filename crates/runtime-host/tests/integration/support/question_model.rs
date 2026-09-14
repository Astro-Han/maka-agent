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

use http_body_util::{BodyExt, Full};
use hyper::{
    Request, Response,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::convert::Infallible;

pub fn ask_question(provider: &std::net::TcpListener) -> tokio::task::JoinHandle<()> {
    let provider = tokio::net::TcpListener::from_std(provider.try_clone().unwrap()).unwrap();
    tokio::spawn(async move {
        let (stream, _) = provider.accept().await.unwrap();
        http1::Builder::new()
            .serve_connection(
                TokioIo::new(stream),
                service_fn(|request: Request<Incoming>| async {
                    let body: Value = serde_json::from_slice(
                        &request.into_body().collect().await.unwrap().to_bytes(),
                    )
                    .unwrap();
                    let tools = body["tools"].as_array().unwrap();
                    assert!(
                        tools
                            .iter()
                            .any(|t| t["function"]["name"] == "AskUserQuestion")
                    );
                    let arguments = json!({"questions":[{"question":"Choose",
                    "options":[{"label":"A"},{"label":"B"}]}]})
                    .to_string();
                    let event = json!({"id":"completion","created":1,"model":"fixture-model",
                    "choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[
                        {"index":0,"id":"provider-question","type":"function",
                         "function":{"name":"AskUserQuestion","arguments":arguments}}]},
                        "finish_reason":"tool_calls"}],
                    "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}});
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(200)
                            .header("content-type", "text/event-stream")
                            .header("connection", "close")
                            .body(Full::new(Bytes::from(format!(
                                "data: {event}\n\ndata: [DONE]\n\n"
                            ))))
                            .unwrap(),
                    )
                }),
            )
            .await
            .unwrap();
    })
}
