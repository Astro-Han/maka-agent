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

use crate::{ModelError, ModelRequest, ProviderKind};
use serde::Serialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};

/// Root-owned credential resolution happens after model admission, outside the
/// global control gate. Dropping a waiter must not abandon a spent refresh grant.
pub trait AuthResolver: Send + Sync {
    fn resolve(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderAuth, ModelError>> + Send + '_>>;
}

/// Execution credentials stay in the trusted provider boundary, never the log.
/// These are authentication profiles, independent of provider catalog brands.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ProviderAuth {
    ApiKey(String),
    Bound {
        /// Stable private route identity, not access/refresh token material.
        identity: String,
        #[serde(skip)]
        resolver: Arc<dyn AuthResolver>,
    },
    Codex {
        access_token: String,
        session_id: String,
    },
}

pub(crate) async fn resolve(auth: &mut ProviderAuth) -> Result<(), ModelError> {
    if let ProviderAuth::Bound { resolver, .. } = auth {
        let resolved = resolver.resolve().await?;
        if matches!(resolved, ProviderAuth::Bound { .. }) {
            return Err(ModelError::Adapter(
                "credential resolver returned another binding".into(),
            ));
        }
        *auth = resolved;
    }
    Ok(())
}

pub(crate) fn prepare(request: &mut ModelRequest) -> Result<(), ModelError> {
    let ProviderAuth::Codex { session_id, .. } = &request.provider.auth else {
        return Ok(());
    };
    if !matches!(request.provider.kind, ProviderKind::OpenaiResponses)
        || session_id.is_empty()
        || session_id.len() > 1024
    {
        return Err(ModelError::Adapter(
            "invalid Codex Responses authentication profile".into(),
        ));
    }
    // Resolve instructions from the full canonical prompt BEFORE a confirmed
    // lane removes its prefix. Otherwise a tool-only delta changes instructions.
    if request.provider_options.is_null() {
        request.provider_options = json!({});
    }
    let options = request
        .provider_options
        .as_object_mut()
        .ok_or_else(|| ModelError::Adapter("provider options must be an object".into()))?;
    let options = options.entry("openai").or_insert_with(|| json!({}));
    let options = options
        .as_object_mut()
        .ok_or_else(|| ModelError::Adapter("OpenAI options must be an object".into()))?;
    let explicit = options
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    if !explicit {
        let instructions = request
            .prompt
            .iter()
            .find_map(|message| match message {
                crate::prompt::Message::System { content, .. } if !content.trim().is_empty() => {
                    Some(content.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| "You are Maka, a helpful AI assistant.".into());
        options.insert("instructions".into(), Value::String(instructions));
    }
    options.insert("store".into(), Value::Bool(false));
    if !options.get("textVerbosity").is_some_and(Value::is_string) {
        options.insert("textVerbosity".into(), json!("medium"));
    }
    Ok(())
}
