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

use super::store;
use maka_event_log::root::{
    FileLease,
    windows::{account_home, create_private_file, open_nofollow, publish_file, validate_private},
};
use maka_sandbox::{
    filesystem::{Access, Compiled, Scope},
    windows::{
        acl::{Removal, Target},
        ensure_drained,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    source: PathBuf,
    destination: PathBuf,
    directory: bool,
}

/// Shared across installations belonging to this OS account. Each consumer
/// records its actual native Job before launch: a crashed Host's closed handles
/// alone do not prove that all of its descendants have stopped.
pub(super) struct Guards {
    root: PathBuf,
    id: Uuid,
    lease: FileLease,
    pins: Vec<File>,
}

pub(super) fn recover() -> io::Result<()> {
    let root = account_home()?.join(".maka-sandbox-guards");
    if !root.try_exists()? {
        return Ok(());
    }
    let _gate = store::admission(&root)?;
    collect(&root)
}
impl Guards {
    pub fn prepare(policy: &Compiled) -> io::Result<Self> {
        if policy.policy().default != Access::Read
            || !policy.policy().deny_globs.is_empty()
            || policy
                .policy()
                .rules
                .iter()
                .any(|rule| rule.path.components().count() > 256)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows sandbox requires a bounded read-default path policy",
            ));
        }
        // Resolve through the account token, not USERPROFILE or a particular
        // State Root. Separate Hosts must coordinate on the same namespace.
        let root = account_home()?.join(".maka-sandbox-guards");
        maka_event_log::root::private_directory(&root)?;
        store::lease_file(&root.join("admission.lock"))?;
        let _gate = store::admission(&root)?;
        collect(&root)?;
        let id = Uuid::new_v4();
        let name = format!("lease-{id}");
        store::lease_file(&root.join(format!("{name}.lock")))?;
        let lease = FileLease::acquire(&root.join(format!("{name}.lock")))?;
        let mut pins = Vec::new();
        let mut targets = Vec::new();
        let result = (|| {
            for rule in policy
                .policy()
                .rules
                .iter()
                .filter(|rule| rule.access != Access::Write)
            {
                materialize(&root, &rule.path, rule.scope == Scope::Subtree)?;
                let (target, pin) = Target::pin(&rule.path)?;
                targets.push(target);
                pins.push(pin);
            }
            store::publish(&root, &format!("{name}.json"), &targets)
        })();
        if let Err(error) = result {
            drop(pins);
            drop(lease);
            return match collect(&root) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(io::Error::other(format!(
                    "{error}; protection cleanup pending: {cleanup}"
                ))),
            };
        }
        Ok(Self {
            root,
            id,
            lease,
            pins,
        })
    }

    /// Persist before the first native process is created.
    pub fn bind(&self, execution: Uuid) -> io::Result<()> {
        let _gate = store::admission(&self.root)?;
        store::publish(
            &self.root,
            &format!("lease-{}.job.json", self.id),
            &execution,
        )
    }

    pub fn finish(self) -> io::Result<()> {
        let Self {
            root, lease, pins, ..
        } = self;
        drop(pins);
        drop(lease);
        let _gate = store::admission(&root)?;
        collect(&root)
    }
}

fn materialize(root: &Path, path: &Path, directory: bool) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("guard has no parent"))?;
    materialize(root, parent, true)?;
    let (_identity, _parent) = Target::pin(parent)?;
    let id = Uuid::new_v4();
    let name = format!("guard-{id}");
    let pending = Pending {
        source: parent.join(format!(".maka-guard-{id}")),
        destination: path.to_owned(),
        directory,
    };
    // The staging name is never reused. Before publication it is private to
    // this OS user; after publication recovery uses only its persistent file ID.
    store::publish(root, &format!("{name}.pending.json"), &pending)?;
    if directory {
        maka_event_log::root::private_directory(&pending.source)?;
    } else {
        create_private_file(&pending.source)?.sync_all()?;
    }
    let (target, file) = Target::capture(&pending.source)?;
    store::publish(root, &format!("{name}.json"), &target)?;
    drop(file);
    publish_file(&pending.source, path)
}

fn active_targets(root: &Path) -> io::Result<Vec<Target>> {
    let mut active = Vec::new();
    for entry in fs::read_dir(root)? {
        let filename = entry?.file_name();
        let Some(name) = filename
            .to_str()
            .and_then(|name| name.strip_suffix(".lock"))
            .filter(|name| name.starts_with("lease-"))
        else {
            continue;
        };
        let lock = root.join(format!("{name}.lock"));
        let lease = match FileLease::acquire(&lock) {
            Ok(lease) => Some(lease),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
            Err(error) => return Err(error),
        };
        let job: Option<Uuid> = store::read(&root.join(format!("{name}.job.json")))?;
        let mut running = lease.is_none();
        if let Some(job) = job {
            match ensure_drained(job) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => running = true,
                Err(error) => return Err(error),
            }
        }
        if running {
            active.extend(store::required::<Vec<Target>>(
                &root.join(format!("{name}.json")),
            )?);
        } else {
            for suffix in ["job.json", "json"] {
                store::remove(&root.join(format!("{name}.{suffix}")))?;
            }
            drop(lease);
            // Unique execution identities are never reused; unlinking this
            // inactive lease cannot split future lifecycle ownership.
            store::remove(&lock)?;
        }
    }
    Ok(active)
}

fn collect(root: &Path) -> io::Result<()> {
    let active = active_targets(root)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let filename = entry?.file_name();
        let Some(name) = filename
            .to_str()
            .and_then(|name| name.strip_suffix(".pending.json"))
            .filter(|name| name.starts_with("guard-"))
        else {
            continue;
        };
        let pending: Pending = store::required(&root.join(format!("{name}.pending.json")))?;
        let target = store::read::<Target>(&root.join(format!("{name}.json")))?;
        entries.push((name.to_owned(), pending, target));
    }
    let active_paths: Vec<_> = entries
        .iter()
        .filter(|(_, _, target)| {
            target
                .as_ref()
                .is_some_and(|target| active.contains(target))
        })
        .map(|(_, pending, _)| pending.destination.clone())
        .collect();
    // Children precede synthetic parents. Parents of live guards must remain
    // recorded, rather than being mistaken for user-populated directories.
    entries
        .sort_by_key(|(_, pending, _)| std::cmp::Reverse(pending.destination.components().count()));
    for (name, pending, target) in entries {
        if active_paths
            .iter()
            .any(|path| path.starts_with(&pending.destination))
        {
            continue;
        }
        if let Some(target) = target {
            match target.remove_empty()? {
                Removal::InUse => continue,
                Removal::Removed | Removal::Preserved => remove_record(root, &name)?,
            }
        } else {
            // Publication cannot precede the durable identity record. The only
            // possible orphan is the private staging name from this intent.
            match open_nofollow(&pending.source, false) {
                Ok(file) => {
                    validate_private(&file)?;
                    drop(file);
                    let (target, file) = Target::capture(&pending.source)?;
                    drop(file);
                    match target.remove_empty()? {
                        Removal::InUse => continue,
                        Removal::Removed | Removal::Preserved => {}
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            remove_record(root, &name)?;
        }
    }
    Ok(())
}

fn remove_record(root: &Path, name: &str) -> io::Result<()> {
    // The object was removed or preserved as user data before either record
    // disappears. An interrupted removal of the second record is harmless.
    store::remove(&root.join(format!("{name}.json")))?;
    store::remove(&root.join(format!("{name}.pending.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_sandbox::{
        filesystem::{Policy, Rule},
        windows::ExecutionJob,
    };
    use std::os::windows::io::AsHandle;

    #[test]
    fn protection_outlives_peer_settlement_and_a_lost_host_until_its_native_tree_exits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let boundary = root.join("missing/protected");
        let policy = Policy {
            default: Access::Read,
            rules: vec![
                Rule::subtree(root, Access::Write),
                Rule::subtree(&boundary, Access::Read),
            ],
            deny_globs: vec![],
        }
        .compile()
        .unwrap();
        let first = Guards::prepare(&policy).unwrap();
        let survivor = Guards::prepare(&policy).unwrap();
        first.finish().unwrap();
        assert!(
            boundary.is_dir(),
            "one consumer must not revoke its peer's protection"
        );

        let id = Uuid::new_v4();
        let job = ExecutionJob::create(id).unwrap();
        let executable = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let mut child = std::process::Command::new(executable)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        job.assign(child.as_handle()).unwrap();
        survivor.bind(id).unwrap();
        drop(survivor); // Lost Host handles do not prove the Job has drained.
        let result = recover();
        let retained = boundary.is_dir();
        drop(job);
        child.wait().unwrap();
        result.unwrap();
        assert!(
            retained,
            "recovery must observe the native execution, not only file leases"
        );
        recover().unwrap();
        assert!(
            !root.join("missing").exists(),
            "last settlement must collect empty synthetic parents"
        );
    }
}
