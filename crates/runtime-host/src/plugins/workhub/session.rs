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

use super::{BROWSER_TOOLS, CLIENT_TOOLS, Control, ID, PROMPT, tools};
use futures_util::future::BoxFuture;
use maka_plugins::{
    contributions::Staged,
    prompt::{Format, Section, SectionMode, Text},
    session::{Behavior, ClientTools, Preparation, SessionBehavior},
};
use maka_runtime::{execution::NativeToolSet, workhub::COORDINATION_SESSION_ID};
use std::sync::Arc;

pub(super) fn publish(staged: &mut Staged, control: Control) -> Result<(), String> {
    staged
        .insert(ID, SessionBehavior(Arc::new(control.clone())))
        .map_err(|error| error.to_string())?;
    staged
        .insert(
            tools::NAME,
            maka_tools::plugins::PluginTool::new(tools::registration(control))
                .map_err(|error| error.to_string())?
                .always_visible(),
        )
        .map_err(|error| error.to_string())?;
    staged
        .insert(
            ID,
            Section {
                format: Format::Plain,
                order: 0,
                mode: SectionMode::Complete,
                text: Text::Literal(PROMPT.into()),
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

impl Behavior for Control {
    fn prepare(&self, session_id: String) -> BoxFuture<'_, Result<Preparation, String>> {
        Box::pin(async move {
            if session_id != COORDINATION_SESSION_ID {
                return Err("WorkHub behavior requires its managed Session".into());
            }
            self.commands
                .coordinator(self.caller.clone())
                .await
                .map_err(|error| error.message)?
                .ok_or("WorkHub Session is unavailable")?;
            Ok(Preparation {
                native_tools: NativeToolSet::Attachments,
                required_clients: Some(ClientTools {
                    required: CLIENT_TOOLS.iter().map(|name| (*name).into()).collect(),
                    optional: BROWSER_TOOLS.iter().map(|name| (*name).into()).collect(),
                    private: [tools::desktop::CONTEXT_TOOL.into()].into(),
                }),
                tool_ceiling: Some(tool_ceiling()),
                ..Default::default()
            })
        })
    }
}

pub(super) fn tool_ceiling() -> std::collections::BTreeSet<String> {
    ["Read", "AskUserQuestion", tools::NAME]
        .into_iter()
        .chain(CLIENT_TOOLS)
        .chain(BROWSER_TOOLS)
        .map(str::to_owned)
        .collect()
}
