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

use super::Host;
use maka_protocol::host::{Diagnostics, Platform, Residency, Status};
use std::collections::VecDeque;

pub(super) struct Activity {
    connections: usize,
    commands: usize,
    executions: usize,
    shells: usize,
    oauth: usize,
}

impl Activity {
    pub(super) fn blocks_cooperation(&self, own_commands: usize) -> bool {
        self.connections > 1 || self.commands > own_commands || self.shells != 0 || self.oauth != 0
    }
    pub(super) fn blocks_retirement(&self, own_commands: usize) -> bool {
        self.connections > 1 || self.commands > own_commands || self.resident_count() != 0
    }

    pub(super) fn allowing_idle_connections(mut self, allow: bool) -> Self {
        if allow {
            self.connections = 0;
        }
        self
    }

    pub(super) fn resident_count(&self) -> usize {
        self.executions + self.shells + self.oauth
    }

    pub(super) fn residencies(&self) -> Vec<Residency<'static>> {
        [
            ("execution", self.executions),
            ("shell", self.shells),
            ("oauth", self.oauth),
        ]
        .into_iter()
        .filter(|(_, count)| *count != 0)
        .map(|(label, count)| Residency { label, count })
        .collect()
    }
}

impl Host {
    pub(super) fn activity(&self) -> Activity {
        self.activity_except(None)
    }

    pub(super) fn activity_except(&self, handoff_connection: Option<uuid::Uuid>) -> Activity {
        let connections = self.accepted_connections.lock().unwrap();
        Activity {
            connections: connections.len()
                - usize::from(handoff_connection.is_some_and(|id| connections.contains(&id))),
            commands: self.commands.len(),
            executions: self.executions.active_count(),
            shells: self.shells.active_count(),
            oauth: self.oauth.active_count(),
        }
    }

    pub(super) fn status(&self, activity: &Activity) -> Status<'_> {
        Status {
            host_epoch: &self.epoch,
            composition_id: maka_protocol::COMPOSITION_ID,
            composition_revision: "3",
            state: self.lifecycle(),
            connections: activity.connections,
            active_operations: activity.commands,
            active_residencies: activity.resident_count(),
        }
    }

    pub(super) fn diagnostics(&self) -> std::io::Result<Diagnostics<'_>> {
        let activity = self.activity();
        Ok(Diagnostics {
            status: self.status(&activity),
            // Identifies installed native modules, not future plugin capabilities.
            composition_modules: &[
                "sessions",
                "execution",
                "tools",
                "shell",
                "client-capability",
                "skills",
                "workhub",
                "configuration",
                "oauth",
            ],
            residencies: activity.residencies(),
            upgrade_blocking_activity: activity.blocks_retirement(0),
            protocol_version: 0,
            compatibility_epoch: maka_protocol::COMPATIBILITY_EPOCH,
            pid: std::process::id(),
            process_uptime_seconds: self.started.elapsed().as_secs(),
            node_version: "not applicable (Rust)",
            #[cfg(target_os = "linux")]
            platform: Platform::Linux,
            #[cfg(target_os = "macos")]
            platform: Platform::MacOs,
            #[cfg(windows)]
            platform: Platform::Windows,
            arch: match std::env::consts::ARCH {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                arch => arch,
            },
            os_release: maka_process::system::os_release()?,
            logs: self
                .diagnostic_log
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entries
                .iter()
                .map(|(entry, _)| entry.clone())
                .collect(),
        })
    }

    pub(super) fn record_diagnostic(&self, message: impl std::fmt::Display) {
        let entry = format!("+{:.3}s {message}", self.started.elapsed().as_secs_f64());
        eprintln!("{entry}");
        self.diagnostic_log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(entry);
    }
}

/// Bounded recent Host lifecycle/transport diagnostics, not the execution log.
#[derive(Default)]
pub(super) struct Log {
    entries: VecDeque<(String, usize)>,
    encoded_bytes: usize,
}

impl Log {
    fn push(&mut self, mut entry: String) {
        const BUDGET: usize = 48 * 1024;
        // Worst-case JSON escaping takes six bytes per input byte. Reserving
        // 8 KiB per entry also leaves room for status/residency metadata.
        let limit = entry.floor_char_boundary(8 * 1024 / 6);
        entry.truncate(limit);
        let bytes = serde_json::to_vec(&entry).expect("string serializes").len() + 1;
        while self.entries.len() >= 256 || self.encoded_bytes + bytes > BUDGET {
            let (_, removed) = self.entries.pop_front().expect("nonempty bounded log");
            self.encoded_bytes -= removed;
        }
        self.encoded_bytes += bytes;
        self.entries.push_back((entry, bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_log_bounds_encoded_escaped_unicode_and_keeps_recent_entries() {
        let mut log = Log::default();
        for _ in 0..300 {
            log.push(format!("{}{}", "\u{0}".repeat(1300), "🦀".repeat(3000)));
        }
        log.push("last".into());
        let entries: Vec<_> = log.entries.iter().map(|(entry, _)| entry).collect();
        assert_eq!(entries.last().unwrap().as_str(), "last");
        assert!(entries.len() <= 256);
        assert!(entries.iter().all(|entry| entry.len() <= 10 * 1024));
        assert!(serde_json::to_vec(&entries).unwrap().len() <= 48 * 1024 + 2);
    }
}
