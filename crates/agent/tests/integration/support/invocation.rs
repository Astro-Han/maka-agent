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

use maka_runtime::execution::{
    CollaborationMode, InvocationConfiguration, OrchestrationMode, PermissionMode, ToolMode,
};

pub fn configuration(tool_mode: ToolMode) -> InvocationConfiguration {
    InvocationConfiguration {
        workspace_identity: None,
        system_prompt: None,
        cwd: std::env::current_dir()
            .expect("test working directory")
            .into_os_string()
            .into_string()
            .expect("UTF-8 test working directory"),
        permission_mode: PermissionMode::Bypass,
        collaboration_mode: CollaborationMode::Agent,
        orchestration_mode: OrchestrationMode::Default,
        tool_mode,
        model: None,
        thinking_level: None,
    }
}
