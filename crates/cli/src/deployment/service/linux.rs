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

use super::{Deployment, HostError, arguments, checked, command, label, publish};
use std::path::PathBuf;

pub(super) struct Service {
    unit: String,
    path: PathBuf,
    definition: String,
}

impl Service {
    pub fn new(deployment: &Deployment) -> Result<Self, HostError> {
        let unit = format!("{}.service", label(deployment));
        let home = crate::serve::home_directory()?.ok_or("missing account home")?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        if !config.is_absolute() {
            return Err("XDG_CONFIG_HOME must be absolute".into());
        }
        let executable = arguments(deployment)?.map(quote).join(" ");
        let definition = format!(
            "[Unit]\nDescription=Maka Host\nStartLimitIntervalSec=60\nStartLimitBurst=5\n\n[Service]\nType=simple\nExecStart=:{executable}\nRestart=on-failure\nRestartSec=2\nKillMode=mixed\nTimeoutStopSec=45\nUMask=0077\n\n[Install]\nWantedBy=default.target\n"
        );
        Ok(Self {
            path: config.join("systemd/user").join(&unit),
            unit,
            definition,
        })
    }

    pub fn prepare(&self) -> Result<(), HostError> {
        // An SSH logout must not destroy a persistent user service.
        let uid = unsafe { libc::geteuid() }.to_string();
        if checked(command(
            "loginctl",
            &["show-user", &uid, "--property=Linger", "--value"],
        )?)?
        .trim()
            != "yes"
        {
            return Err(
                format!("persistent user services require lingering for user {uid}").into(),
            );
        }
        self.stop()?;
        publish(&self.path, &self.definition)?;
        checked(command("systemctl", &["--user", "daemon-reload"])?)?;
        checked(command("systemctl", &["--user", "enable", &self.unit])?)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), HostError> {
        let loaded = checked(command(
            "systemctl",
            &[
                "--user",
                "show",
                "--property=LoadState",
                "--value",
                &self.unit,
            ],
        )?)?;
        match loaded.trim() {
            "not-found" => {}
            "loaded" | "error" | "bad-setting" => {
                // The caller holds Root: no service instance can own user work.
                checked(command("systemctl", &["--user", "stop", &self.unit])?)?;
            }
            _ => return Err("unrecognized systemd service load state".into()),
        }
        Ok(())
    }

    pub fn remove(&self) -> Result<(), HostError> {
        let fragment = checked(command(
            "systemctl",
            &[
                "--user",
                "show",
                "--property=FragmentPath",
                "--value",
                &self.unit,
            ],
        )?)?;
        if !fragment.trim().is_empty() && std::path::Path::new(fragment.trim()) != self.path {
            return Err("service definition location differs; restore its original XDG_CONFIG_HOME before cleanup".into());
        }
        self.stop()?;
        match self.path.symlink_metadata() {
            Ok(metadata) if metadata.is_file() => {
                checked(command("systemctl", &["--user", "disable", &self.unit])?)?;
                std::fs::remove_file(&self.path)?;
                std::fs::File::open(self.path.parent().ok_or("service path has no parent")?)?
                    .sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("service definition is not a regular file".into()),
        }
        checked(command("systemctl", &["--user", "daemon-reload"])?)?;
        Ok(())
    }

    pub fn start(&self) -> Result<(), HostError> {
        checked(command(
            "systemctl",
            &["--user", "reset-failed", &self.unit],
        )?)?;
        checked(command("systemctl", &["--user", "start", &self.unit])?)?;
        Ok(())
    }
}

fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}
