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

use super::{Deployment, HostError, arguments, checked, command, label, publish, xml};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) struct Service {
    uid: String,
    label: String,
    target: String,
    path: PathBuf,
    definition: String,
}

struct Instance {
    pid: Option<libc::pid_t>,
}

impl Service {
    pub fn new(deployment: &Deployment) -> Result<Self, HostError> {
        let uid = unsafe { libc::geteuid() }.to_string();
        let label = label(deployment);
        let home = crate::serve::home_directory()?.ok_or("missing account home")?;
        let path = home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        let args = arguments(deployment)?
            .map(|arg| format!("<string>{}</string>", xml(arg)))
            .join("");
        let log = super::super::directory(&deployment.root_id)?.join("host.stderr.log");
        let log = xml(log.to_str().ok_or("service log path must be UTF-8")?);
        let definition = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{args}</array><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>2</integer><key>ExitTimeOut</key><integer>45</integer><key>Umask</key><integer>63</integer><key>StandardErrorPath</key><string>{log}</string></dict></plist>\n"
        );
        Ok(Self {
            target: format!("gui/{uid}/{label}"),
            uid,
            label,
            path,
            definition,
        })
    }

    fn in_user_domain(&self, args: &[&str]) -> Result<String, HostError> {
        let mut command_args = vec!["asuser", &self.uid, "/bin/launchctl"];
        command_args.extend_from_slice(args);
        checked(command("/bin/launchctl", &command_args)?)
    }

    pub fn prepare(&self) -> Result<(), HostError> {
        self.stop()?;
        publish(&self.path, &self.definition)?;
        checked(command("/bin/launchctl", &["enable", &self.target])?)?;
        checked(command(
            "/bin/launchctl",
            &[
                "bootstrap",
                &format!("gui/{}", self.uid),
                self.path.to_str().ok_or("service path must be UTF-8")?,
            ],
        )?)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), HostError> {
        if self.in_user_domain(&["managername"])?.trim() != "Aqua"
            || self.in_user_domain(&["manageruid"])?.trim() != self.uid
        {
            return Err("the account GUI launchd domain is unavailable".into());
        }
        // Unlike print's diagnostic format, list's three columns are documented.
        if let Some(instance) = self.instance()? {
            // Root is held, so even a racing automatic launch cannot execute work.
            checked(command("/bin/launchctl", &["bootout", &self.target])?)?;
            // bootout can return before launchd removes the old registration.
            // Do not race bootstrap against that asynchronous teardown.
            let deadline = Instant::now() + Duration::from_secs(45);
            while self.instance()?.is_some() || instance.pid.is_some_and(process_exists) {
                if Instant::now() >= deadline {
                    return Err("launchd service did not finish unloading".into());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        Ok(())
    }

    pub fn remove(&self) -> Result<(), HostError> {
        self.stop()?;
        match self.path.symlink_metadata() {
            Ok(metadata) if metadata.is_file() => {
                std::fs::remove_file(&self.path)?;
                std::fs::File::open(self.path.parent().ok_or("service path has no parent")?)?
                    .sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("service definition is not a regular file".into()),
        }
        Ok(())
    }

    fn instance(&self) -> Result<Option<Instance>, HostError> {
        for line in self.in_user_domain(&["list"])?.lines() {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() == 3 && columns[2] == self.label {
                let pid = match columns[0] {
                    "-" => None,
                    pid => Some(
                        pid.parse::<libc::pid_t>()
                            .ok()
                            .filter(|pid| *pid > 0)
                            .ok_or("invalid launchd service PID")?,
                    ),
                };
                return Ok(Some(Instance { pid }));
            }
        }
        Ok(None)
    }

    pub fn start(&self) -> Result<(), HostError> {
        // No -k: an implicitly launched instance must never be killed.
        checked(command("/bin/launchctl", &["kickstart", &self.target])?)?;
        Ok(())
    }
}

fn process_exists(pid: libc::pid_t) -> bool {
    // Signal zero only observes existence; it never signals a potentially reused PID.
    let exists = unsafe { libc::kill(pid, 0) == 0 };
    exists || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
