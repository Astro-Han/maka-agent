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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly nominated TS dev root and authorized Luna subscription"]
async fn original_client_live_luna_uses_rust_host_and_resumes_after_reopen() {
    use std::io::Read;
    let root = std::path::PathBuf::from(
        std::env::var_os("MAKA_CODEX_TEST_STATE_ROOT").expect("explicit root required"),
    );
    let read = |name: &str| -> Vec<u8> {
        let mut bytes = Vec::new();
        std::fs::File::open(root.join(name))
            .unwrap()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes.len() <= 4 * 1024 * 1024);
        bytes
    };
    let source = [
        "connection-catalog.json",
        "credential-vault.json",
        "runtime-policy.json",
    ]
    .map(|name| (name, read(name)));
    let catalog: Value = serde_json::from_slice(&source[0].1).unwrap();
    let vault: Value = serde_json::from_slice(&source[1].1).unwrap();
    let policy: Value = serde_json::from_slice(&source[2].1).unwrap();
    let row = catalog["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["providerType"] == "openai-codex"
                && row["enabled"] == true
                && row["enabledModelIds"]
                    .as_array()
                    .is_some_and(|ids| ids.contains(&json!("gpt-5.6-luna")))
        })
        .expect("enabled Luna subscription required");
    let entries = vault["entries"].as_array().unwrap();
    let credential = entries
        .iter()
        .find(|entry| {
            entry["locator"]["scope"] == "connection"
                && entry["locator"]["connectionId"] == row["connectionId"]
                && entry["locator"]["kind"] == "oauth_token"
        })
        .expect("subscription credential required");
    let tokens: Value = serde_json::from_str(credential["secret"].as_str().unwrap()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(
        tokens["expires_at"]
            .as_u64()
            .is_some_and(|expiry| expiry > now + 10 * 60 * 1000),
        "refresh through the original owner before this read-only acceptance"
    );
    // Only the already-authorized ACCESS token enters the private disposable
    // fixture. Never copy or spend the source root's rotating refresh grant.
    let secret = json!({"access_token":tokens["access_token"],"refresh_token":"read-only-acceptance-no-refresh",
        "expires_at":tokens["expires_at"]}).to_string();
    let network = NetworkConfiguration {
        proxy: serde_json::from_value(policy["policy"]["networkProxy"].clone()).unwrap(),
        password: entries
            .iter()
            .find(|entry| entry["locator"]["scope"] == "network_proxy")
            .and_then(|entry| entry["secret"].as_str())
            .map(str::to_owned),
    };
    run(secret, network).await;
    for (name, before) in source {
        assert!(read(name) == before, "source root must remain untouched");
    }
}
