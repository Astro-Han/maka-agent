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

use maka_runtime::configuration::policy::SubagentProfile;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(super) enum Profile {
    General,
    LocalRead,
    WebResearch,
    Implementation,
}

impl Profile {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::LocalRead => "local-read",
            Self::WebResearch => "web-research",
            Self::Implementation => "implementation",
        }
    }
    pub(super) fn tools(self) -> Option<BTreeSet<String>> {
        match self {
            Self::General => None,
            Self::LocalRead => Some(
                ["Read", "Glob", "Grep"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            ),
            Self::WebResearch => Some(["WebSearch"].into_iter().map(str::to_owned).collect()),
            Self::Implementation => Some(
                [
                    "Read",
                    "Glob",
                    "Grep",
                    "Write",
                    "Edit",
                    "apply_patch",
                    "shell",
                    "WriteStdin",
                    "StopBackgroundTask",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
        }
    }
    pub(super) fn instructions(self) -> Option<String> {
        match self {
            Self::General => None,
            Self::LocalRead => Some("You are a local-read child agent. Use Read, Glob and Grep only. Return concise findings with concrete file and symbol evidence. Do not delegate or modify files.".into()),
            Self::WebResearch => Some("You are a web-research child agent. Use WebSearch only. Cite source titles and URLs for external claims, and distinguish inference from sourced facts. Do not delegate or access local files.".into()),
            Self::Implementation => Some("Implement the assigned task in your dedicated worktree. Return a patch-oriented summary and verification results.".into()),
        }
    }
}
impl From<SubagentProfile> for Profile {
    fn from(profile: SubagentProfile) -> Self {
        match profile {
            SubagentProfile::LocalRead => Self::LocalRead,
            SubagentProfile::WebResearch => Self::WebResearch,
            SubagentProfile::Implementation => Self::Implementation,
        }
    }
}
