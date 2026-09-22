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

use super::*;

enum Input {
    Search(Query),
    More(More),
    Material(material::Input),
}
impl ToolPreparer for Recall {
    fn names(&self) -> Vec<String> {
        vec![
            "Recall".into(),
            "RecallMore".into(),
            "RecallMaterial".into(),
        ]
    }
    fn prepare(
        &self,
        name: String,
        input: Value,
        _: ToolCallContext,
        cancellation: CancellationToken,
    ) -> PreparationFuture {
        let recall = self.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(ToolRejection::Cancelled);
            }
            let input = match name.as_str() {
                "Recall" => {
                    let mut query: Query = serde_json::from_value(input).map_err(invalid)?;
                    query.validate().map_err(invalid)?;
                    Input::Search(query)
                }
                "RecallMore" => {
                    let more: More = serde_json::from_value(input).map_err(invalid)?;
                    more.validate().map_err(invalid)?;
                    Input::More(more)
                }
                "RecallMaterial" => {
                    let material: material::Input =
                        serde_json::from_value(input).map_err(invalid)?;
                    material.validate().map_err(invalid)?;
                    Input::Material(material)
                }
                _ => return Err(ToolRejection::Unavailable),
            };
            Ok(PreparedEffect::new(move |_| {
                Box::pin(async move {
                    let call = maka_plugins::call::current()
                        .ok_or_else(|| failed("Recall requires an admitted call"))?;
                    recall.check_privacy().await?;
                    if let Input::Material(input) = input {
                        return recall.material(call, input).await;
                    }
                    let permit = tokio::select! {
                        biased;
                        _ = call.cancellation.cancelled() => return Err(failed("Recall cancelled")),
                        permit = recall.workers.clone().acquire_owned() => Arc::new(permit.map_err(failed)?),
                    };
                    let mut result = match input {
                        Input::Material(_) => unreachable!("material handled before search"),
                        Input::Search(query) => recall.search(&call, &query, permit).await?,
                        Input::More(more) => {
                            let (catalog, _) = reader::sessions(
                                recall.history.as_ref(),
                                &call,
                                Some(&more.session_id),
                            )
                            .await?;
                            let summary = catalog
                                .into_iter()
                                .next()
                                .ok_or_else(|| failed("Recall source Session was not found"))?;
                            let mut reader = Reader::new(
                                recall.history.clone(),
                                call.clone(),
                                more.session_id.clone(),
                                None,
                            );
                            let mut position = 0;
                            let hit = loop {
                                let message = reader
                                    .next()
                                    .await?
                                    .ok_or_else(|| failed("Recall anchor was not found"))?;
                                if message.message_id == more.anchor_message_id {
                                    if rank::excluded(&call, &more.session_id, &message) {
                                        return Err(failed("Recall cannot expand its active Turn"));
                                    }
                                    break rank::Hit {
                                        session: 0,
                                        position,
                                        sequence: message.sequence,
                                        message_id: message.message_id,
                                        turn_id: message.turn_id,
                                        timestamp: message.timestamp,
                                        role: message.role,
                                        length: 0,
                                        frequencies: vec![],
                                        offset: more.offset.unwrap_or(0),
                                        score: 0.0,
                                    };
                                }
                                position += 1;
                            };
                            let sources = [rank::Source {
                                summary,
                                through: reader.through(),
                            }];
                            let passages = passages::build(
                                recall.history.clone(),
                                &call,
                                &sources,
                                &[hit],
                                &[],
                                (more.before.unwrap_or(8), more.after.unwrap_or(8)),
                                permit,
                            )
                            .await?;
                            ResultSet {
                                passages,
                                searched_every_session: true,
                                gaps: vec![],
                            }
                        }
                    };
                    recall.check_privacy().await?;
                    // Check the source call again even for an empty result.
                    recall
                        .history
                        .list(call, Default::default())
                        .await
                        .map_err(failed)?;
                    if result.passages.is_empty() && result.searched_every_session {
                        result
                            .gaps
                            .push("No matching passage was found in the scanned history.".into());
                    }
                    if result.render().len() > 96 * 1024 {
                        result.gaps.push(
                            "The 96 KiB response budget omitted lower-ranked passages.".into(),
                        );
                        while result.render().len() > 96 * 1024 && !result.passages.is_empty() {
                            result.passages.pop();
                        }
                    }
                    let text = result.render();
                    Ok(ToolSuccess::projected(
                        ToolOutput::Json(serde_json::to_value(result).map_err(failed)?),
                        DurableToolProjection::Text { text },
                    ))
                })
            }))
        })
    }
}

impl Recall {
    pub(super) async fn search(
        &self,
        call: &maka_plugins::call::Scope,
        query: &Query,
        permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<ResultSet, ToolError> {
        let scan = rank::search(self.history.clone(), call, query, permit.clone()).await?;
        let mut hits = scan.hits;
        for hit in &mut hits {
            hit.offset = hit.offset.saturating_sub(512);
        }
        let passages = passages::build(
            self.history.clone(),
            call,
            &scan.sources,
            &hits,
            &query.terms,
            (4, 4),
            permit,
        )
        .await?;
        Ok(ResultSet {
            passages,
            searched_every_session: scan.complete,
            gaps: scan.gaps,
        })
    }
}

fn invalid(error: impl std::fmt::Display) -> ToolRejection {
    ToolRejection::InvalidInput {
        message: error.to_string(),
    }
}
