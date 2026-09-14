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

use maka_event_log::{
    EventLog,
    transcript::{TranscriptDirection, TranscriptRead},
};
use serde_json::Value;

pub async fn transcript(log: &EventLog, session: &str) -> Vec<Value> {
    let through = log.prefix(100, 1024 * 1024).await.unwrap().high_water;
    while !log.prepare_transcript(session, through, 32).await.unwrap() {}
    let headers = log
        .transcript_headers(
            session,
            &TranscriptRead {
                through: maka_presentation::watermark(through).unwrap(),
                position: 0,
                direction: TranscriptDirection::Newer,
                limit: 100,
            },
        )
        .await
        .unwrap();
    let mut rows = Vec::new();
    for header in headers {
        rows.push(
            serde_json::from_slice(
                &log.transcript_fragment(session, header.sequence, 0, header.total_bytes)
                    .await
                    .unwrap(),
            )
            .unwrap(),
        );
    }
    rows
}
