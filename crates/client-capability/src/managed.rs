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

use maka_runtime::{
    capability::{AdmissionEvidence, ToolDescriptor},
    interaction::{GrantCapability, GrantScope},
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManagedAdmissionError {
    #[error("Managed Client Capability requires a trusted provider")]
    UntrustedProvider,
    #[error("Resolved registration does not match the frozen capability binding")]
    WrongRegistration,
    #[error("Tool is not present in the frozen capability offer")]
    UnknownTool,
    #[error("Client Capability has no managed admission policy")]
    UnknownPolicy,
    #[error("Managed Client Capability admission evidence does not match policy")]
    InvalidEvidence,
    #[error("Desktop Browser admission requires a valid absolute HTTP(S) URL")]
    InvalidBrowserUrl,
}

pub(crate) fn scope(
    offer_id: &str,
    tool: &ToolDescriptor,
    evidence: &AdmissionEvidence,
) -> Result<Option<(GrantCapability, GrantScope)>, ManagedAdmissionError> {
    if offer_id == "desktop_settings"
        && tool.server_id == "desktop_settings"
        && matches!(
            tool.name.as_str(),
            "MakaClientSettingsGet" | "MakaClientSettingsUpdate"
        )
    {
        return match evidence {
            AdmissionEvidence::None => Ok(None),
            _ => Err(ManagedAdmissionError::InvalidEvidence),
        };
    }
    if offer_id == "desktop_browser"
        && tool.server_id == "desktop_browser"
        && matches!(
            tool.name.as_str(),
            "browser_navigate"
                | "browser_snapshot"
                | "browser_click"
                | "browser_type"
                | "browser_wait"
                | "browser_extract"
        )
    {
        let AdmissionEvidence::BrowserUrl { url } = evidence else {
            return Err(ManagedAdmissionError::InvalidEvidence);
        };
        let url = url::Url::parse(url).map_err(|_| ManagedAdmissionError::InvalidBrowserUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ManagedAdmissionError::InvalidBrowserUrl);
        }
        return Ok(Some((
            GrantCapability::Browser,
            GrantScope::BrowserOrigin {
                origin: url.origin().ascii_serialization(),
            },
        )));
    }
    if offer_id == "desktop_mcp" || offer_id.starts_with("desktop_mcp_") {
        if !matches!(evidence, AdmissionEvidence::None) {
            return Err(ManagedAdmissionError::InvalidEvidence);
        }
        return Ok(Some((
            GrantCapability::DesktopMcp,
            GrantScope::McpTool {
                server_id: tool.server_id.clone(),
                tool_name: tool.name.clone(),
            },
        )));
    }
    Err(ManagedAdmissionError::UnknownPolicy)
}
