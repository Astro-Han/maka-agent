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

#[cfg(any(target_os = "linux", windows))]
mod cleanup;
#[cfg(windows)]
mod windows;
#[cfg(any(target_os = "linux", windows))]
pub(crate) use cleanup::Cleanup;

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

/// Captured launch data shared by native pipe and PTY transports. It deliberately
/// does not try to reverse-engineer platform-specific std::Command internals.
pub struct Command {
    pub(crate) executable: PathBuf,
    pub(crate) args: Vec<Argument>,
    pub(crate) cwd: PathBuf,
    pub(crate) environment: BTreeMap<OsString, OsString>,
    sandbox: maka_sandbox::Sandbox,
    network_route: maka_network::Policy,
    #[cfg(unix)]
    proxy: Option<maka_network::proxy::Proxy>,
    #[cfg(target_os = "macos")]
    proxy_address: Option<std::net::SocketAddr>,
    #[cfg(target_os = "linux")]
    network_helper: Option<PathBuf>,
    #[cfg(windows)]
    backend: Option<std::sync::Arc<dyn crate::bootstrap::Backend>>,
    #[cfg(windows)]
    launch: Option<crate::bootstrap::Launch>,
    #[cfg(windows)]
    write_token: Option<std::sync::Arc<maka_sandbox::windows::WriteToken>>,
    #[cfg(target_os = "linux")]
    files: Vec<std::fs::File>,
    #[cfg(target_os = "linux")]
    pub(crate) mount_lease: Option<maka_sandbox::launch::MountLease>,
}

/// Only successful platform preparation can construct a native launch. Keeping
/// conversion off Command makes forgetting the sandbox a compile-time error.
pub(crate) struct Prepared(Command);

#[cfg_attr(windows, derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum Argument {
    Quoted(OsString),
    #[cfg(windows)]
    Verbatim(OsString),
}

impl AsRef<OsStr> for Argument {
    fn as_ref(&self) -> &OsStr {
        match self {
            Self::Quoted(value) => value,
            #[cfg(windows)]
            Self::Verbatim(value) => value,
        }
    }
}

#[cfg(windows)]
impl Argument {
    pub(crate) fn command_line(&self) -> std::io::Result<String> {
        let value = self
            .as_ref()
            .to_str()
            .ok_or_else(|| std::io::Error::other("PTY arguments must be UTF-8"))?;
        Ok(match self {
            Self::Quoted(_) => crate::shell::command::quote(value),
            Self::Verbatim(_) => value.into(),
        })
    }
}

impl Command {
    pub fn new(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            environment: crate::environment::inherit(std::env::vars_os()),
            sandbox: maka_sandbox::Sandbox::Disabled,
            network_route: maka_network::Policy::default(),
            #[cfg(unix)]
            proxy: None,
            #[cfg(target_os = "macos")]
            proxy_address: None,
            #[cfg(target_os = "linux")]
            network_helper: None,
            #[cfg(windows)]
            backend: None,
            #[cfg(windows)]
            launch: None,
            #[cfg(windows)]
            write_token: None,
            #[cfg(target_os = "linux")]
            files: Vec::new(),
            #[cfg(target_os = "linux")]
            mount_lease: None,
        }
    }

    pub fn arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(Argument::Quoted(value.as_ref().to_owned()));
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.environment.clear();
        self
    }

    /// Capture upstream routing without granting network access.
    pub fn network_route(mut self, route: maka_network::Policy) -> Self {
        self.network_route = route;
        self
    }

    #[cfg(target_os = "linux")]
    pub fn with_network_helper(mut self, helper: PathBuf) -> Self {
        self.network_helper = Some(helper);
        self
    }

    /// Explicit executor-owned installation; never inferred from environment or
    /// process-global state. Merely capturing it performs no native work.
    #[cfg(windows)]
    pub fn with_backend(mut self, backend: std::sync::Arc<dyn crate::bootstrap::Backend>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Apply the runner's write-restricted token at process creation. This is
    /// only the token layer: the caller must already own account isolation,
    /// network rules and ACL lifetime. It cannot replace a managed policy.
    #[cfg(windows)]
    pub fn with_write_token(
        mut self,
        token: std::sync::Arc<maka_sandbox::windows::WriteToken>,
    ) -> Self {
        self.write_token = Some(token);
        self
    }

    /// Capture an already authorized policy without creating native resources.
    /// Dropping an unstarted plan never changes the filesystem. Windows verbatim
    /// arguments retain their original representation.
    pub fn sandbox(mut self, policy: &maka_sandbox::Sandbox) -> Result<Self, maka_sandbox::Error> {
        if !self.executable.is_absolute() {
            return Err(maka_sandbox::Error::Invalid(
                "sandbox executable must be absolute".into(),
            ));
        }
        if let maka_sandbox::Sandbox::Managed { filesystem, .. } = policy {
            filesystem.compile()?;
        }
        self.sandbox = policy.clone();
        Ok(self)
    }

    /// Only the accepted process owner calls this, and must await startup rather
    /// than race it against cancellation. Filesystem preparation runs off the
    /// async scheduler; the owner's admission fence covers preparation and spawn.
    pub(crate) async fn prepare(self) -> std::io::Result<Prepared> {
        if matches!(self.sandbox, maka_sandbox::Sandbox::Disabled) {
            return Ok(Prepared(self));
        }
        #[cfg(target_os = "macos")]
        let mut command = self;
        #[cfg(target_os = "macos")]
        if let maka_sandbox::Sandbox::Managed {
            network: maka_sandbox::Network::Restricted { .. },
            ..
        } = &command.sandbox
        {
            let maka_sandbox::Sandbox::Managed { network, .. } = &command.sandbox else {
                unreachable!()
            };
            let (address, proxy) =
                maka_network::proxy::Proxy::start(network.clone(), command.network_route.clone())
                    .await?;
            let url = format!("http://{address}");
            for key in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ] {
                command.env(key, &url);
            }
            command.env("NO_PROXY", "").env("no_proxy", "");
            command.proxy = Some(proxy);
            command.proxy_address = Some(address);
        }
        #[cfg(target_os = "linux")]
        let mut command = self;
        #[cfg(target_os = "linux")]
        if let maka_sandbox::Sandbox::Managed {
            network: network @ maka_sandbox::Network::Restricted { .. },
            ..
        } = &command.sandbox
        {
            use std::os::fd::AsRawFd;
            let helper = command
                .network_helper
                .take()
                .filter(|path| path.is_absolute())
                .ok_or_else(|| {
                    std::io::Error::other(
                        "restricted network requires the trusted CLI namespace helper",
                    )
                })?;
            let (control, proxy) =
                crate::network_namespace::prepare(network.clone(), command.network_route.clone())?;
            let mut args = vec![
                Argument::Quoted(crate::network_namespace::HELPER.into()),
                Argument::Quoted(control.as_raw_fd().to_string().into()),
                Argument::Quoted(command.executable.into_os_string()),
            ];
            args.append(&mut command.args);
            command.args = args;
            command.executable = helper;
            command.files.push(control);
            command.proxy = Some(proxy);
        }
        #[cfg(windows)]
        let command = self;
        #[cfg(windows)]
        if let maka_sandbox::Sandbox::Managed {
            filesystem,
            network,
        } = &command.sandbox
        {
            let backend = command.backend.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Windows sandbox execution requires an explicit installation",
                )
            })?;
            let launch = backend
                .prepare(
                    filesystem.clone(),
                    network.clone(),
                    command.network_route.clone(),
                    command.executable.clone(),
                    command.cwd.clone(),
                )
                .await?;
            let mut command = command;
            if let Some(address) = launch.proxy_address {
                let url = format!("http://{address}");
                for key in [
                    "HTTP_PROXY",
                    "http_proxy",
                    "HTTPS_PROXY",
                    "https_proxy",
                    "ALL_PROXY",
                    "all_proxy",
                ] {
                    command.env(key, &url);
                }
                command.env("NO_PROXY", "").env("no_proxy", "");
            }
            command.launch = Some(launch);
            return Ok(Prepared(command));
        }
        tokio::task::spawn_blocking(move || command.prepare_native())
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)
    }

    fn prepare_native(mut self) -> Result<Prepared, maka_sandbox::Error> {
        if let Some(name) = self
            .environment
            .keys()
            .find(|name| crate::environment::controls_loader(name))
        {
            return Err(maka_sandbox::Error::Invalid(format!(
                "{} controls the sandbox launcher's loader; set it inside the sandboxed command instead (for example with env)",
                name.to_string_lossy()
            )));
        }
        #[cfg(target_os = "linux")]
        let launch = if self.proxy.is_some() {
            self.sandbox.prepare_with_private_network(&self.cwd)?
        } else {
            self.sandbox.prepare(&self.cwd)?
        };
        #[cfg(windows)]
        let launch = self.sandbox.prepare(&self.cwd)?;
        #[cfg(target_os = "macos")]
        let launch = match self.proxy_address {
            Some(address) => self.sandbox.prepare_with_proxy(&self.cwd, address)?,
            None => self.sandbox.prepare(&self.cwd)?,
        };
        match launch {
            maka_sandbox::launch::Launch::Direct => {}
            maka_sandbox::launch::Launch::Wrapped {
                program,
                args,
                #[cfg(target_os = "linux")]
                files,
                #[cfg(target_os = "linux")]
                lease,
            } => {
                let mut wrapped: Vec<_> = args.into_iter().map(Argument::Quoted).collect();
                wrapped.push(Argument::Quoted(self.executable.into_os_string()));
                wrapped.append(&mut self.args);
                self.executable = program;
                self.args = wrapped;
                #[cfg(target_os = "linux")]
                {
                    self.files.extend(files);
                    self.mount_lease = Some(lease);
                }
            }
        }
        Ok(Prepared(self))
    }

    pub fn args<I, S>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(
            values
                .into_iter()
                .map(|v| Argument::Quoted(v.as_ref().to_owned())),
        );
        self
    }

    /// Append literal Windows command-line syntax without CRT argument quoting.
    /// The caller must supply the target program's quoting (e.g. cmd.exe /c).
    #[cfg(windows)]
    pub fn raw_arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.args
            .push(Argument::Verbatim(value.as_ref().to_owned()));
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        let key = key.as_ref();
        #[cfg(windows)]
        self.environment.retain(|existing, _| {
            existing.to_string_lossy().to_uppercase() != key.to_string_lossy().to_uppercase()
        });
        self.environment
            .insert(key.to_owned(), value.as_ref().to_owned());
        self
    }
}

impl Prepared {
    #[cfg(unix)]
    pub(crate) fn take_proxy(&mut self) -> Option<maka_network::proxy::Proxy> {
        self.0.proxy.take()
    }
    #[cfg(windows)]
    pub(crate) fn take_launch(&mut self) -> Option<crate::bootstrap::Launch> {
        self.0.launch.take()
    }

    #[cfg(windows)]
    pub(crate) fn runner_plan(self) -> crate::bootstrap::protocol::Plan {
        crate::bootstrap::protocol::Plan {
            executable: self.0.executable,
            args: self.0.args,
            cwd: self.0.cwd,
            environment: self.0.environment.into_iter().collect(),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn take_mount_lease(&mut self) -> Option<maka_sandbox::launch::MountLease> {
        self.0.mount_lease.take()
    }

    #[cfg(unix)]
    pub(crate) fn unix(self) -> tokio::process::Command {
        let plan = self.0;
        let mut command = tokio::process::Command::new(plan.executable);
        command
            .args(plan.args)
            .current_dir(plan.cwd)
            .env_clear()
            .envs(plan.environment);
        #[cfg(target_os = "linux")]
        if !plan.files.is_empty() {
            use std::os::fd::AsRawFd;
            // Keep the pinned descriptors alive with Command. Clear CLOEXEC
            // only after fork, never in the multithreaded Host.
            unsafe {
                command.pre_exec(move || {
                    for file in &plan.files {
                        if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }
        command
    }
}
