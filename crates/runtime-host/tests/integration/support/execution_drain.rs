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

use super::execution_fixture;
pub use execution_fixture::fixture;

pub async fn fail_closure(database: &mut sqlx::SqliteConnection) {
    // A valid pending approval is independently durable at admission. Only its
    // canonical closure INSERT fails, with an ordinary SQL error before COMMIT.
    sqlx::raw_sql(
        r#"
        CREATE TRIGGER establish_approval AFTER INSERT ON event_log
        WHEN NEW.kind = 'invocation_opened' BEGIN
            INSERT INTO interaction_requests VALUES (
                'approval', json_extract(NEW.event_json, '$.invocation.session_id'), 1,
                json_object(
                    'requestId', 'approval', 'createdAt', 1,
                    'sessionId', json_extract(NEW.event_json, '$.invocation.session_id'),
                    'turnId', json_extract(NEW.event_json, '$.invocation.turn_id'),
                    'runId', json_extract(NEW.event_json, '$.invocation.run_id'),
                    'request', json('{"kind":"client_capability","toolUseId":"pending","target":{
                        "providerId":"provider","contractId":"contract","serverId":"browser",
                        "toolName":"navigate","capability":"browser",
                        "scope":{"kind":"browser_origin","origin":"https://example.com"}}}'),
                    'outcome', NULL));
        END;
        CREATE TRIGGER fail_closure BEFORE INSERT ON interaction_outcomes
        BEGIN SELECT RAISE(ABORT, 'injected ordinary closure failure'); END;
    "#,
    )
    .execute(database)
    .await
    .unwrap();
}

pub fn reject_model(provider: &std::net::TcpListener) -> tokio::task::JoinHandle<()> {
    let provider = tokio::net::TcpListener::from_std(provider.try_clone().unwrap()).unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = provider.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        // A non-retryable provider refusal reaches normal failed finalization;
        // all execution commits before the interaction closure remain valid.
        stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    })
}
