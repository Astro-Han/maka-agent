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

#[cfg(windows)]
mod windows;

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
}

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
            environment: std::env::vars_os().collect(),
        }
    }

    pub fn arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(Argument::Quoted(value.as_ref().to_owned()));
        self
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
