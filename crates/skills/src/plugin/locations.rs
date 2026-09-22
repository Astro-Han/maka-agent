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

use super::{Error, Skills, remote::failure};
use crate::{Source, api::*};
use maka_plugins::{
    authorization::{Capability, Request, Target},
    call::Owned,
    filesystem::{ListInput, ReadDirectory, ReadError, Symlinks, entries},
    remote::{Caller, Error as RemoteError, WorkspaceViewInput},
};
use maka_runtime::{
    execution::{CollaborationMode, SandboxMode, WorkspaceTarget},
    tools::ToolError,
};
use std::io;

impl Skills {
    pub(super) async fn locations(
        &self,
        action: LocationAction,
        workspace: Option<(ReadDirectory, WorkspaceTarget)>,
        caller: &Caller,
    ) -> Result<LocationResult, RemoteError> {
        let _call = self.basis.owner.admit().map_err(|_| RemoteError::Retired)?;
        let private = self.data.read_only().await.map_err(provider)?;
        let home = self.inputs.open("user-skills").map_err(provider)?;
        let sources = Source::standard(
            workspace.as_ref().map(|(files, _)| files),
            &private,
            home.as_ref(),
        );
        match action {
            LocationAction::List => {
                let _read = self.mutations.read().await;
                let locations =
                    inspect_locations(&sources, &self.basis.owner, &caller.cancellation)
                        .await
                        .map_err(failure)?;
                Ok(LocationResult::Locations { locations })
            }
            LocationAction::Open {
                id,
                expected_path,
                create_if_missing,
            } => {
                let Some(source) = sources
                    .iter()
                    .find(|source| source.reference_prefix == id.reference())
                else {
                    return Ok(rejected(LocationRejection::Unavailable));
                };
                if display_path(source).as_deref() != Some(expected_path.as_str()) {
                    return Ok(rejected(LocationRejection::Changed));
                }
                let status = directory_status(&source.access, id.directory())
                    .await
                    .map_err(failure)?;
                match status {
                    LocationStatus::Missing if create_if_missing => {
                        self.create_location(
                            id,
                            source,
                            workspace.as_ref().map(|(_, target)| target),
                            caller,
                        )
                        .await
                    }
                    LocationStatus::Available => Ok(LocationResult::Resolved {
                        path: expected_path,
                    }),
                    status => Ok(rejected(rejection(status))),
                }
            }
        }
    }

    async fn create_location(
        &self,
        id: LocationId,
        source: &Source,
        workspace: Option<&WorkspaceTarget>,
        caller: &Caller,
    ) -> Result<LocationResult, RemoteError> {
        let project = matches!(id.scope(), crate::SkillScope::Project);
        let authority = if id == LocationId::Workspace {
            None
        } else {
            // UI file management can edit instruction directories protected from
            // Agent workspace-write. This Remote grant permits files only.
            let target = if project {
                Target::Workspace {
                    workspace: workspace.expect("project source").clone(),
                    sandbox_mode: SandboxMode::DangerFullAccess,
                }
            } else {
                Target::Directory {
                    path: source
                        .root
                        .to_str()
                        .ok_or_else(|| RemoteError::Invalid("Skill root is not UTF-8".into()))?
                        .into(),
                }
            };
            Some(
                caller
                    .views
                    .authorize(Request {
                        operation_id: uuid::Uuid::new_v4(),
                        title: "Create Skill discovery directory / 创建技能发现目录".into(),
                        target,
                        capabilities: [Capability::ReadFiles, Capability::WriteFiles].into(),
                    })
                    .await?,
            )
        };
        let result = async {
            if project {
                let refreshed = caller
                    .views
                    .workspace(WorkspaceViewInput {
                        workspace: workspace.expect("project source").clone(),
                        sandbox_mode: SandboxMode::DangerFullAccess,
                        collaboration_mode: CollaborationMode::Agent,
                    })
                    .await?;
                if refreshed.files.location() != source.root {
                    return Ok(rejected(LocationRejection::Changed));
                }
            }
            let _write = self.mutations.write().await;
            let _invalidation = self.input_revision.invalidate().await;
            let _notice = self.notify_on_exit();
            let mut relative = String::new();
            for component in id.directory().split('/') {
                if !relative.is_empty() {
                    relative.push('/');
                }
                relative.push_str(component);
                match directory_status(&source.access, &relative)
                    .await
                    .map_err(failure)?
                {
                    LocationStatus::Available => continue,
                    LocationStatus::Missing => {}
                    status => return Ok(rejected(rejection(status))),
                }
                self.create_directory(relative.clone(), authority.as_ref())
                    .await?;
                let status = directory_status(&source.access, &relative)
                    .await
                    .map_err(failure)?;
                if status != LocationStatus::Available {
                    return Ok(rejected(rejection(status)));
                }
            }
            Ok(LocationResult::Resolved {
                path: display_path(source).expect("validated location"),
            })
        }
        .await;
        if let Some(authority) = authority {
            authority
                .finish()
                .await
                .map_err(|_| RemoteError::CleanupUnconfirmed)?;
        }
        result
    }

    async fn create_directory(
        &self,
        path: String,
        authority: Option<&Owned>,
    ) -> Result<(), RemoteError> {
        if let Some(authority) = authority {
            match self
                .user
                .files
                .invoke(
                    authority.scope(),
                    maka_plugins::filesystem::Operation::Entries(
                        entries::Operation::CreateDirectory { path },
                    ),
                )
                .await
            {
                Ok(maka_plugins::filesystem::Output::Entries(entries::Output::Done))
                | Err(ToolError::Io {
                    kind: io::ErrorKind::AlreadyExists,
                    ..
                }) => Ok(()),
                Err(ToolError::OutcomeUnknown(message)) => {
                    Err(RemoteError::OutcomeUnknown(message))
                }
                Err(error) => Err(provider(error)),
                Ok(_) => Err(provider("Invalid directory result")),
            }
        } else {
            match self.data.create_directory(path).await {
                Ok(()) | Err(entries::Error::AlreadyExists) => Ok(()),
                Err(entries::Error::OutcomeUnknown(message)) => {
                    Err(RemoteError::OutcomeUnknown(message))
                }
                Err(error) => Err(provider(error)),
            }
        }
    }
}
fn provider(error: impl ToString) -> RemoteError {
    RemoteError::Provider(error.to_string())
}
fn display_path(source: &Source) -> Option<String> {
    source
        .root
        .join(&source.directory)
        .to_str()
        .filter(|path| path.len() <= 4096)
        .map(str::to_owned)
}
async fn inspect_locations(
    sources: &[Source],
    owner: &maka_plugins::fiber::Context,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Vec<Location>, Error> {
    let mut locations = Vec::new();
    for id in LocationId::ALL {
        if cancellation.is_cancelled() || !owner.is_effective() {
            return Err(Error::Retired);
        }
        let source = sources
            .iter()
            .find(|source| source.reference_prefix == id.reference());
        let result = inspect(id, source, cancellation).await;
        if cancellation.is_cancelled() || !owner.is_effective() {
            return Err(Error::Retired);
        }
        locations.push(match result {
            // A captured workspace can expire independently of the plugin and
            // caller. Never follow its replacement or hide unrelated roots.
            Err(Error::Retired) => unavailable(id),
            result => result?,
        });
    }
    Ok(locations)
}
fn unavailable(id: LocationId) -> Location {
    Location {
        id,
        path: None,
        status: LocationStatus::Unavailable,
        valid_count: 0,
        invalid_count: 0,
    }
}
async fn inspect(
    id: LocationId,
    source: Option<&Source>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Location, Error> {
    let mut location = unavailable(id);
    let Some(source) = source else {
        return Ok(location);
    };
    location.path = display_path(source);
    if location.path.is_none() {
        location.status = LocationStatus::BlockedPath;
        return Ok(location);
    }
    location.status = directory_status(&source.access, id.directory()).await?;
    if location.status == LocationStatus::Available {
        let scan = match crate::scan(std::slice::from_ref(source), cancellation).await {
            Ok(scan) => scan,
            Err(crate::ScanError::Cancelled) => return Err(Error::Retired),
            Err(_) => {
                location.status = LocationStatus::ReadFailed;
                return Ok(location);
            }
        };
        location.valid_count = scan.inventory.len();
        location.invalid_count = scan.rejected.len();
        if !scan.diagnostics.is_empty() {
            location.status = LocationStatus::ReadFailed;
        }
    }
    Ok(location)
}
async fn directory_status(files: &ReadDirectory, path: &str) -> Result<LocationStatus, Error> {
    let result = files
        .list(ListInput {
            files: entries::ListFiles {
                path: path.into(),
                after: None,
                limit: 1,
            },
            symlinks: Symlinks::Reject,
        })
        .await;
    Ok(match result {
        Ok(_) => LocationStatus::Available,
        Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            LocationStatus::Missing
        }
        Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::NotADirectory => {
            LocationStatus::BlockedPath
        }
        Err(ReadError::Invalid(_)) => LocationStatus::BlockedPath,
        Err(ReadError::Retired) => return Err(Error::Retired),
        Err(ReadError::Io(_)) => LocationStatus::ReadFailed,
    })
}
fn rejected(reason: LocationRejection) -> LocationResult {
    LocationResult::Rejected { reason }
}
fn rejection(status: LocationStatus) -> LocationRejection {
    match status {
        LocationStatus::Missing => LocationRejection::Missing,
        LocationStatus::BlockedPath => LocationRejection::BlockedPath,
        LocationStatus::ReadFailed => LocationRejection::ReadFailed,
        LocationStatus::Unavailable => LocationRejection::Unavailable,
        LocationStatus::Available => unreachable!("available location"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_plugins::{
        composition::Scope,
        fiber::Fiber,
        filesystem::{ReadAuthorization, ReadRoot},
    };
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct ExpiredWorkspace;
    impl ReadAuthorization for ExpiredWorkspace {
        fn check(
            &self,
        ) -> futures_util::future::BoxFuture<'_, Result<maka_plugins::call::Ticket, ReadError>>
        {
            Box::pin(async { Err(ReadError::Retired) })
        }
    }

    #[tokio::test]
    async fn expired_workspace_preserves_independent_locations_but_not_cancelled_calls() {
        let directory = tempfile::tempdir().unwrap();
        for path in ["skills", ".maka/skills", ".agents/skills"] {
            std::fs::create_dir_all(directory.path().join(path)).unwrap();
        }
        let owner = Fiber::new("example.locations", "reader", Scope::Profile).unwrap();
        owner.begin_loading().unwrap();
        owner.ready().unwrap();
        owner.publish().unwrap();
        let cancellation = CancellationToken::new();
        let root = ReadRoot::open(directory.path()).await.unwrap();
        let project = root.bind_authorized(
            owner.context(),
            cancellation.clone(),
            Arc::new(ExpiredWorkspace),
        );
        let independent = root.bind(owner.context(), cancellation.clone());
        let sources = Source::standard(Some(&project), &independent, Some(&independent));
        let locations = inspect_locations(&sources, &owner.context(), &cancellation)
            .await
            .unwrap();
        assert_eq!(locations.len(), 5);
        for location in &locations[..2] {
            assert_eq!(location.status, LocationStatus::Unavailable);
            assert!(location.path.is_none());
        }
        for location in &locations[2..] {
            assert_eq!(location.status, LocationStatus::Available);
            assert!(location.path.is_some());
        }
        cancellation.cancel();
        assert!(matches!(
            inspect_locations(&sources, &owner.context(), &cancellation).await,
            Err(Error::Retired)
        ));
    }
}
