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

use super::support::client_probe::ClientFixture;
use maka_runtime_host::{
    server::{DirectoryRootSpec, HostOptions},
    session::SessionConfiguration,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_projects_relink_sessions_and_reopen_without_changing_runtime_facts() {
    let fixture = ClientFixture::new("maka-project-client-");
    let options = || HostOptions {
        project_directory_roots: Some(vec![DirectoryRootSpec {
            label: "Fixture".into(),
            path: fixture.workspace.clone(),
        }]),
        ..HostOptions::default()
    };
    fixture
        .run_with_options("--project-workspace", false, "project-passed", options())
        .await;
    let log = fixture.log().await;
    let projects = log.list_projects().await.unwrap();
    assert_eq!(projects.len(), 36);
    let mut sessions = Vec::new();
    for id in [
        "project-session-a",
        "project-session-b",
        "project-session-alias",
        "project-session-relocated",
    ] {
        sessions.push(
            log.get_session::<SessionConfiguration>(id)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(
        log.get_session::<SessionConfiguration>("archived-project-session")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        log.get_session::<SessionConfiguration>("missing-project-session")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        log.prefix(8, 4096).await.unwrap().events.is_empty(),
        "Project/Session metadata is not execution history"
    );
    log.close().await.unwrap();
    fixture
        .run_with_options("--project-workspace", true, "project-reopened", options())
        .await;
    let log = fixture.log().await;
    assert_eq!(log.list_projects().await.unwrap(), projects);
    for before in sessions {
        assert_eq!(
            log.get_session::<SessionConfiguration>(&before.id)
                .await
                .unwrap()
                .unwrap(),
            before
        );
    }
    assert!(log.prefix(8, 4096).await.unwrap().events.is_empty());
    log.close().await.unwrap();
}
