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

use maka_client::Client;
use maka_protocol::plugin::{RemoteBinding, RemoteKind, RemoteRequest, RemoteResult};
use maka_skills::api::InvocableResult;
use serde_json::json;

use super::Request;

pub async fn execute(
    client: &Client,
    request: &Request,
) -> Result<(RemoteResult, InvocableResult), String> {
    let binding = RemoteBinding::Package {
        package_id: super::PROVIDER.into(),
        method: "request".into(),
        session_id: Some(request.session.clone()),
    };
    let bound = match &request.bound {
        Some(bound) => bound.clone(),
        None => client
            .plugin_remote(RemoteRequest::Bind {
                binding: binding.clone(),
            })
            .await
            .map_err(|e| e.to_string())?,
    };
    let RemoteResult::Bound {
        target,
        handler: RemoteKind::Method,
    } = &bound
    else {
        return Err("Skills did not publish a method".into());
    };
    let RemoteResult::Document { document } = client
        .plugin_remote(RemoteRequest::OpenDocument)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("Missing Remote document".into());
    };
    // One finite read owns its document. Closing the picker never aborts this cleanup;
    // connection teardown cancels the job and Host releases that connection's documents.
    let result = client
        .plugin_remote(RemoteRequest::Call {
            binding,
            target: target.clone(),
            document,
            input: json!({"kind":"invocable","page":request.page.as_ref().map(|(revision,cursor)|
            json!({"revision":revision,"cursor":cursor}))}),
        })
        .await;
    let closed = client
        .plugin_remote(RemoteRequest::CloseDocument { document })
        .await;
    let RemoteResult::Value { value } = result.map_err(|e| e.to_string())? else {
        return Err("Missing Skills page".into());
    };
    closed.map_err(|e| e.to_string())?;
    let result: InvocableResult = serde_json::from_value(value).map_err(|e| e.to_string())?;
    validate(&result, request.page.as_ref())?;
    Ok((bound, result))
}

fn validate(result: &InvocableResult, page: Option<&(String, String)>) -> Result<(), String> {
    match result {
        InvocableResult::Page {
            revision,
            items,
            next_cursor,
        } => {
            let mut ids = std::collections::HashSet::new();
            if revision.is_empty()
                || items.len() > maka_skills::api::MAX_ITEMS
                || page.is_some_and(|(expected, _)| expected != revision)
                || next_cursor.as_ref().is_some_and(|cursor| {
                    cursor.is_empty() || page.is_some_and(|(_, previous)| previous == cursor)
                })
                || (items.is_empty() && next_cursor.is_some())
                || items.iter().any(|item| {
                    !ids.insert(&item.id)
                        || super::validate(&[super::Picked {
                            id: item.id.clone(),
                            name: item.name.clone(),
                        }])
                        .is_err()
                })
            {
                return Err("Invalid Skills candidate page".into());
            }
        }
        InvocableResult::RevisionChanged {
            expected_revision,
            actual_revision,
        } => {
            if page.is_none_or(|(expected, _)| expected != expected_revision)
                || actual_revision.is_empty()
                || actual_revision == expected_revision
            {
                return Err("Invalid Skills revision change".into());
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use maka_skills::api::InvocableItem;

    #[test]
    fn pages_reject_mixed_revisions_repeated_cursors_and_unsubmittable_selectors() {
        let page = Some(("one".into(), "cursor".into()));
        let item = InvocableItem {
            reference: "project:review".into(),
            id: "review".into(),
            name: "Review".into(),
            description: "Review code".into(),
        };
        let valid = InvocableResult::Page {
            revision: "one".into(),
            items: vec![item.clone()],
            next_cursor: Some("next".into()),
        };
        assert!(validate(&valid, page.as_ref()).is_ok());
        for invalid in [
            InvocableResult::Page {
                revision: "two".into(),
                items: vec![item.clone()],
                next_cursor: None,
            },
            InvocableResult::Page {
                revision: "one".into(),
                items: vec![item.clone()],
                next_cursor: Some("cursor".into()),
            },
            InvocableResult::Page {
                revision: "one".into(),
                items: vec![item.clone(), item.clone()],
                next_cursor: None,
            },
            InvocableResult::Page {
                revision: "one".into(),
                items: vec![],
                next_cursor: Some("next".into()),
            },
            InvocableResult::Page {
                revision: "one".into(),
                items: vec![InvocableItem {
                    id: "".into(),
                    ..item
                }],
                next_cursor: None,
            },
            InvocableResult::RevisionChanged {
                expected_revision: "other".into(),
                actual_revision: "two".into(),
            },
        ] {
            assert!(validate(&invalid, page.as_ref()).is_err());
        }
        let changed = InvocableResult::RevisionChanged {
            expected_revision: "one".into(),
            actual_revision: "two".into(),
        };
        assert!(validate(&changed, page.as_ref()).is_ok());
        assert!(validate(&changed, None).is_err());
    }
}
