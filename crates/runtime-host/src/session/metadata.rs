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

use super::{SessionConfiguration, name};
use maka_protocol::{Result, session::SessionMetadataPatch};

/// Apply a decoded patch inside the store's CAS closure: stale revisions must
/// conflict before normalization or no-op detection, including manual renames.
pub fn apply_metadata_patch(
    configuration: &mut SessionConfiguration,
    patch: SessionMetadataPatch,
) -> Result<()> {
    if let Some(requested) = patch.name {
        configuration.name = name::normalize(&requested)?;
        configuration.title_is_manual = true;
    }
    if let Some(requested) = patch.labels {
        configuration.labels = replace_user_owned_labels(&configuration.labels, requested);
    }
    if let Some(flagged) = patch.is_flagged {
        configuration.is_flagged = flagged;
    }
    Ok(())
}

fn execution_owned(label: &str) -> bool {
    label == "mode:bot"
}

fn replace_user_owned_labels(current: &[String], requested: Vec<String>) -> Vec<String> {
    let mut users = requested
        .into_iter()
        .filter(|label| !execution_owned(label));
    let mut labels = Vec::new();
    for label in current {
        if execution_owned(label) {
            labels.push(label.clone());
        } else if let Some(user) = users.next() {
            labels.push(user);
        }
    }
    labels.extend(users);
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{PreparedSession, SessionModel};
    use maka_protocol::session::{SandboxMode, SessionCreateInput};
    use serde_json::json;

    fn configuration() -> SessionConfiguration {
        let input: SessionCreateInput = serde_json::from_value(json!({
            "sessionId": "metadata-policy",
            "workspace": {"kind": "host_path", "path": "/tmp"},
            "modelTarget": {"kind": "default"},
            "name": "café"
        }))
        .unwrap();
        PreparedSession::new(input).unwrap().bind(
            maka_protocol::session::WorkspaceProjection {
                target: maka_protocol::session::WorkspaceTarget::HostPath {
                    path: "/tmp".into(),
                },
                host_cwd: "/tmp".into(),
            },
            crate::session::SessionTarget::Model {
                model: SessionModel {
                    connection_id: "connection".into(),
                    connection_slug: "connection".into(),
                    model: "model".into(),
                },
            },
            SandboxMode::ReadOnly,
        )
    }

    #[test]
    fn normalized_same_name_becomes_manual_once() {
        let mut config = configuration();
        let original = config.clone();
        let patch = SessionMetadataPatch {
            name: Some("cafe\u{301}\u{200b}".into()),
            labels: None,
            is_flagged: None,
        };
        assert!(!config.title_is_manual);
        apply_metadata_patch(&mut config, patch.clone()).unwrap();
        assert_eq!(config.name, original.name);
        assert!(config.title_is_manual);
        assert_ne!(config, original);
        let manual = config.clone();
        apply_metadata_patch(&mut config, patch).unwrap();
        assert_eq!(config, manual);
    }

    #[test]
    fn replacement_keeps_execution_slots_and_ignores_injection() {
        let labels = |values: &[&str]| -> Vec<String> {
            values.iter().map(|value| (*value).into()).collect()
        };
        let current = labels(&["old", "mode:bot", "old2"]);
        assert_eq!(
            replace_user_owned_labels(&current, labels(&["new", "mode:bot", "next", "last"])),
            labels(&["new", "mode:bot", "next", "last"])
        );
        assert_eq!(
            replace_user_owned_labels(&current, vec![]),
            labels(&["mode:bot"])
        );
        assert!(replace_user_owned_labels(&[], labels(&["mode:bot"])).is_empty());
    }

    #[test]
    fn invalid_name_leaves_entire_patch_unapplied_and_false_clears_flag() {
        let mut config = configuration();
        config.is_flagged = true;
        let before = config.clone();
        assert!(
            apply_metadata_patch(
                &mut config,
                SessionMetadataPatch {
                    name: Some("\u{200b}".into()),
                    labels: Some(vec!["replacement".into()]),
                    is_flagged: Some(false),
                }
            )
            .is_err()
        );
        assert_eq!(config, before);
        apply_metadata_patch(
            &mut config,
            SessionMetadataPatch {
                name: None,
                labels: None,
                is_flagged: Some(false),
            },
        )
        .unwrap();
        assert!(!config.is_flagged);
        assert_eq!(config.name, before.name);
        assert!(!config.title_is_manual);
    }
}
