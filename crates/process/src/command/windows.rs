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

use super::Command;
use crate::shell::command::{quote, wide};
use std::{collections::BTreeMap, io, os::windows::ffi::OsStrExt, path::Path};

pub(crate) struct Buffers {
    pub executable: Vec<u16>,
    pub cwd: Vec<u16>,
    pub line: Vec<u16>,
    pub environment: Vec<u16>,
}
impl Command {
    pub(crate) fn windows(&self) -> io::Result<Buffers> {
        if !self.executable.is_absolute() || !self.cwd.is_absolute() {
            return Err(io::Error::other(
                "process requires captured absolute executable and cwd",
            ));
        }
        let executable = wide(dunce::simplified(&self.executable))?;
        let cwd = wide(dunce::simplified(&self.cwd))?;
        let executable_arg = dunce::simplified(&self.executable)
            .to_str()
            .map(|path| quote(&path.replace('/', "\\")))
            .ok_or_else(|| io::Error::other("process executable must be UTF-8"))?;
        let arguments = std::iter::once(Ok(executable_arg))
            .chain(self.args.iter().map(super::Argument::command_line))
            .collect::<io::Result<Vec<_>>>()?
            .join(" ");
        let line = wide(Path::new(&arguments))?;
        let mut sorted = BTreeMap::new();
        for (key, value) in &self.environment {
            if key.is_empty()
                || key.encode_wide().any(|c| c == 0)
                || value.encode_wide().any(|c| c == 0)
            {
                return Err(io::Error::other("invalid process environment"));
            }
            sorted.insert(key.to_string_lossy().to_uppercase(), (key, value));
        }
        let mut environment = Vec::new();
        for (key, value) in sorted.into_values() {
            environment.extend(key.encode_wide());
            environment.push('=' as u16);
            environment.extend(value.encode_wide());
            environment.push(0);
        }
        if environment.is_empty() {
            environment.push(0);
        }
        environment.push(0);
        Ok(Buffers {
            executable,
            cwd,
            line,
            environment,
        })
    }
}
