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

mod access_expiry;
mod artifact_boundary;
mod artifact_interop;
mod attachment_consumption;
mod auto_context;
mod client_access;
mod client_anthropic_options;
mod client_bash;
mod client_capability;
mod client_connection_test;
mod client_forms;
mod client_interactions;
mod client_interop;
mod client_live_provider;
mod client_model_fetch;
mod client_onboarding;
mod client_openai_options;
mod client_patch;
mod client_questions;
mod client_remote_access;
mod client_tools;
mod client_write;
mod compatible_chat;
mod connection_multiplex;
mod context_compaction;
mod execution_boundary;
mod execution_drain;
mod host_drain;
mod large_outputs;
mod live_pipes;
mod live_pty_stream;
mod live_shell;
mod message_interrupt;
mod message_queue;
mod message_recovery;
mod message_submit;
mod model_overrides;
mod oauth;
mod oauth_execution;
mod oauth_refresh;
mod projects;
mod pruning;
mod question_boundary;
mod read_pages;
mod relay_options;
mod remote_capacity;
mod resume;
mod retirement;
mod runtime_policy;
mod system_prompt;
mod transcript_limits;
mod transcript_pager;
mod workhub;
mod workspace_images;

mod support {
    pub(crate) mod client_probe;
    #[cfg(unix)]
    pub(crate) mod execution_drain;
    #[cfg(unix)]
    pub(crate) mod execution_fixture;
    pub(crate) mod message_recovery;
    pub(crate) mod peer;
    #[cfg(unix)]
    pub(crate) mod question_model;
    pub(crate) mod shell_resources;
}
