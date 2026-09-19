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

use super::{Error, Skills, catalog};
use crate::api::*;
use maka_runtime::execution::WorkspaceProjection;

impl Skills {
    /// A dropped RPC waiter must not cancel an accepted domain write.
    pub async fn mutate(
        &self,
        input: MutateInput,
        workspace: WorkspaceProjection,
    ) -> Result<MutationResult, Error> {
        let admitted = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let skills = self.clone();
        let receiver = self
            .basis
            .owner
            .spawn_resource("Skills mutation", move |_| async move {
                let _admitted = admitted;
                let _serial = skills.mutations.write().await;
                let _invalidation = skills.input_revision.invalidate().await;
                let _notice = skills.notify_on_exit();
                Ok(skills.mutate_inner(&input, workspace).await)
            })
            .map_err(|_| Error::Retired)?;
        receiver
            .await
            .map_err(|error| Error::OutcomeUnknown(error.to_string()))?
            .map_err(Error::OutcomeUnknown)?
    }

    async fn mutate_inner(
        &self,
        input: &MutateInput,
        workspace: WorkspaceProjection,
    ) -> Result<MutationResult, Error> {
        let (sources, preferences) = self.governance(&workspace.host_cwd).await?;
        let revision =
            catalog::revision(&input.context, &workspace, &sources, preferences.as_ref())?;
        let result = |outcome| {
            Ok(MutationResult {
                outcome,
                resolved_workspace: workspace.clone(),
            })
        };
        if revision != input.expected_revision {
            return result(MutationOutcome::RevisionConflict {
                expected_revision: input.expected_revision.clone(),
                actual_revision: revision,
            });
        }
        if !matches!(
            input.mutation,
            Mutation::SetEnabled { .. } | Mutation::SetPinned { .. }
        ) {
            return self.mutate_files(input, workspace, sources, revision).await;
        }
        let Some(preferences) = preferences else {
            return result(MutationOutcome::Rejected {
                reason: MutationRejection::StateError,
            });
        };
        let reference = input.mutation.reference().expect("preference mutation");
        let items = catalog::governance::items(&sources, Some(&preferences));
        let Some(item) = items.into_iter().find_map(|item| match item {
            CatalogItem::Skill(item) if item.reference == reference => Some(item),
            _ => None,
        }) else {
            return result(MutationOutcome::Rejected {
                reason: MutationRejection::NotFound,
            });
        };
        if item.needs_review {
            return result(MutationOutcome::Rejected {
                reason: MutationRejection::NeedsReview,
            });
        }
        let prior = preferences
            .entries
            .get(reference)
            .copied()
            .unwrap_or_default();
        let mut next = prior;
        match input.mutation {
            Mutation::SetEnabled { enabled, .. } => next.enabled = enabled,
            Mutation::SetPinned { pinned, .. } => next.pinned = pinned,
            _ => unreachable!("preference mutation"),
        }
        if prior == next {
            return result(MutationOutcome::Unchanged {
                revision,
                entry: Some(MutationEntry::Skill(item)),
            });
        }
        // The admitted worker retains its repository through retirement.
        let committed = self
            .basis
            .preferences
            .compare_exchange(preferences.revision, reference.into(), next)
            .await
            .map_err(Error::OutcomeUnknown)?;
        let preferences = Some(self.basis.preferences.read().await.map_err(|error| {
            if committed {
                Error::OutcomeUnknown(error)
            } else {
                Error::Source(error)
            }
        })?);
        let revision =
            catalog::revision(&input.context, &workspace, &sources, preferences.as_ref()).map_err(
                |error| {
                    if committed {
                        Error::OutcomeUnknown(error.to_string())
                    } else {
                        error
                    }
                },
            )?;
        if !committed {
            return result(MutationOutcome::RevisionConflict {
                expected_revision: input.expected_revision.clone(),
                actual_revision: revision,
            });
        }
        let entry = catalog::governance::items(&sources, preferences.as_ref())
            .into_iter()
            .find_map(|item| match item {
                CatalogItem::Skill(item) if item.reference == reference => {
                    Some(MutationEntry::Skill(item))
                }
                _ => None,
            });
        result(MutationOutcome::Committed { revision, entry })
    }
}

impl Skills {
    async fn mutate_files(
        &self,
        input: &MutateInput,
        workspace: WorkspaceProjection,
        sources: crate::SourceCatalog,
        current_revision: String,
    ) -> Result<MutationResult, Error> {
        let root = self.state_root.clone();
        let home = self.home.clone();
        let cwd = workspace.host_cwd.clone();
        let mutation = input.mutation.clone();
        let cancellation = self.basis.owner.stopping().map_err(|_| Error::Retired)?;
        let operation = self
            .data
            .run(move |data| {
                let change = super::files::apply(
                    &root,
                    data,
                    home.as_deref(),
                    &sources,
                    &mutation,
                    &cancellation,
                )?;
                let sources = if change.changed {
                    crate::governance_catalog(
                        std::path::Path::new(&cwd),
                        &root,
                        home.as_deref(),
                        &tokio_util::sync::CancellationToken::new(),
                    )
                    .map_err(|error| {
                        super::files::Failure::Fatal(Error::OutcomeUnknown(error.to_string()))
                    })?
                } else {
                    sources
                };
                Ok::<_, super::files::Failure>((change, sources))
            })
            .await
            .map_err(|error| match error {
                maka_plugins::storage::StoreError::Retired => Error::Retired,
                maka_plugins::storage::StoreError::OutcomeUnknown(message) => {
                    Error::OutcomeUnknown(message)
                }
                error => Error::Source(error.to_string()),
            })?;
        let outcome = match operation {
            Err(super::files::Failure::Rejected(reason)) => MutationOutcome::Rejected { reason },
            Err(super::files::Failure::Fatal(error)) => {
                if matches!(error, Error::OutcomeUnknown(_)) {
                    self.basis.owner.cleanup_failed(error.to_string());
                }
                return Err(error);
            }
            Ok((change, sources)) => {
                let preferences = self.basis.preferences.read().await.map_err(|error| {
                    if change.changed {
                        Error::OutcomeUnknown(error)
                    } else {
                        Error::Source(error)
                    }
                })?;
                let revision = if change.changed {
                    catalog::revision(&input.context, &workspace, &sources, Some(&preferences))
                        .map_err(|error| Error::OutcomeUnknown(error.to_string()))?
                } else {
                    current_revision
                };
                let entry = catalog::governance::items(&sources, Some(&preferences))
                    .into_iter()
                    .find_map(|item| match item {
                        CatalogItem::Skill(item)
                            if Some(&item.reference) == change.reference.as_ref() =>
                        {
                            Some(MutationEntry::Skill(item))
                        }
                        _ => None,
                    });
                if change.changed {
                    MutationOutcome::Committed { revision, entry }
                } else {
                    MutationOutcome::Unchanged { revision, entry }
                }
            }
        };
        Ok(MutationResult {
            outcome,
            resolved_workspace: workspace,
        })
    }
}
