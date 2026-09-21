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

use maka_plugins::{call, http};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use url::Url;

#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Query {
    #[schemars(length(min = 1, max = 200))]
    pub query: String,
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 10))]
    pub limit: usize,
}
fn default_limit() -> usize {
    5
}
impl Query {
    pub fn validate(&self) -> Result<(), Error> {
        if self.query.trim().is_empty()
            || self.query.chars().count() > 200
            || !(1..=10).contains(&self.limit)
        {
            Err(Error::Input)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Results {
    pub query: String,
    pub rows: Vec<Row>,
    pub truncated: bool,
    pub omitted_results: usize,
}
#[derive(Debug, Serialize)]
pub struct Row {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "WebSearch requires a nonempty query of at most 200 characters and a limit from 1 to 10"
    )]
    Input,
    #[error("WebSearch credentials are not configured")]
    NotConfigured,
    #[error("WebSearch credentials were rejected")]
    InvalidCredentials,
    #[error("WebSearch rate limit exceeded")]
    RateLimited,
    #[error("WebSearch request timed out")]
    Timeout,
    #[error("WebSearch request was cancelled")]
    Cancelled,
    #[error("WebSearch returned HTTP {0}")]
    Status(u16),
    #[error("WebSearch returned an invalid or oversized response")]
    Response,
    #[error("WebSearch network request failed")]
    Network,
    #[error("WebSearch network access was denied")]
    Denied,
    #[error(transparent)]
    Settlement(#[from] maka_runtime::tools::ToolError),
}

#[derive(Clone)]
pub struct Search {
    http: Arc<dyn http::Client>,
}
impl Search {
    pub fn new(http: Arc<dyn http::Client>) -> Self {
        Self { http }
    }

    pub async fn query(
        &self,
        parent: &call::Scope,
        key: &str,
        input: &Query,
    ) -> Result<Results, Error> {
        input.validate()?;
        if key.trim().is_empty() {
            return Err(Error::NotConfigured);
        }
        let owned = call::Owned::new(parent.child()?);
        let scope = owned.scope();
        let result = tokio::select! {
            biased;
            _ = scope.cancellation.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(10), self.request(&scope, key, input)) =>
                result.unwrap_or(Err(Error::Timeout)),
        };
        owned.finish().await?;
        result
    }

    async fn request(
        &self,
        scope: &call::Scope,
        key: &str,
        input: &Query,
    ) -> Result<Results, Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            api_key: &'a str,
            query: &'a str,
            max_results: usize,
            search_depth: &'static str,
        }
        let response = self
            .http
            .request(
                scope.clone(),
                http::Request {
                    url: "https://api.tavily.com/search".into(),
                    method: http::Method::Post,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: serde_json::to_vec(&Request {
                        api_key: key,
                        query: &input.query,
                        max_results: input.limit,
                        search_depth: "basic",
                    })
                    .map_err(|_| Error::Input)?,
                },
            )
            .await
            .map_err(network_error)?;
        let result = async {
            match response.head.status {
                200..300 => {}
                401 | 403 => return Err(Error::InvalidCredentials),
                429 => return Err(Error::RateLimited),
                status => return Err(Error::Status(status)),
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.body.next().await.map_err(network_error)? {
                if chunk.len() > 1024 * 1024 - bytes.len() {
                    return Err(Error::Response);
                }
                bytes.extend_from_slice(&chunk);
            }
            decode(&bytes, input)
        }
        .await;
        // Never follow redirects with a search credential.
        response.body.close().await.map_err(network_error)?;
        result
    }
}
fn network_error(error: http::Error) -> Error {
    match error {
        http::Error::Denied => Error::Denied,
        http::Error::CleanupUnconfirmed => {
            Error::Settlement(maka_runtime::tools::ToolError::CleanupUnconfirmed(
                "WebSearch HTTP cleanup is unconfirmed".into(),
            ))
        }
        _ => Error::Network,
    }
}

fn decode(bytes: &[u8], input: &Query) -> Result<Results, Error> {
    #[derive(Deserialize)]
    struct Response {
        results: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Entry {
        title: String,
        url: String,
        #[serde(default)]
        content: String,
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| Error::Response)?;
    let total = response.results.len();
    let rows = response
        .results
        .into_iter()
        .filter_map(|entry| {
            let entry: Entry = serde_json::from_value(entry).ok()?;
            let url = Url::parse(&entry.url).ok()?;
            if entry.url.len() > 2048
                || !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return None;
            }
            let (title, title_truncated) = clip(entry.title, 240);
            let (snippet, snippet_truncated) = clip(entry.content, 400);
            Some(Row {
                title,
                snippet,
                url: url.to_string(),
                truncated: title_truncated || snippet_truncated,
            })
        })
        .take(input.limit)
        .collect::<Vec<_>>();
    let omitted_results = total - rows.len();
    Ok(Results {
        query: input.query.clone(),
        truncated: omitted_results > 0 || rows.iter().any(|row| row.truncated),
        omitted_results,
        rows,
    })
}

fn clip(value: String, chars: usize) -> (String, bool) {
    let Some((end, _)) = value.char_indices().nth(chars) else {
        return (value, false);
    };
    (value[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn results_disclose_omissions_and_clipping_without_returning_unsafe_urls() {
        let bytes = serde_json::to_vec(&serde_json::json!({"results":[
            {"title":"bad","url":"javascript:alert(1)"},
            {"title":"credential","url":"https://secret@example.com/"},
            {"title":"useful","url":"https://example.com/a","content":"字".repeat(401)},
            {"title":"b","url":"https://example.com/b"},
            {"title":"c","url":"https://example.com/c"}
        ]}))
        .unwrap();
        let result = decode(
            &bytes,
            &Query {
                query: "topic".into(),
                limit: 2,
            },
        )
        .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.omitted_results, 3);
        assert!(result.truncated);
        assert!(result.rows[0].truncated);
        assert_eq!(result.rows[0].snippet.chars().count(), 400);
        assert!(
            decode(
                br#"{"error":"provider rejected request"}"#,
                &Query {
                    query: "topic".into(),
                    limit: 2
                }
            )
            .is_err()
        );
    }
}
