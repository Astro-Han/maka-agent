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
use std::path::PathBuf;

/// Selected once, so the advertised dialect and executed command cannot drift.
#[derive(Clone)]
pub(crate) enum ShellPlan {
    #[cfg(unix)]
    Posix,
    #[cfg(windows)]
    Pwsh(PathBuf),
    #[cfg(windows)]
    WindowsPowerShell(PathBuf),
    #[cfg(windows)]
    Cmd(PathBuf),
}

impl ShellPlan {
    pub(crate) fn pty_command(
        &self,
        cwd: &std::path::Path,
        source: &str,
    ) -> crate::pty::PtyCommand {
        use crate::pty::PtyCommand;
        match self {
            #[cfg(unix)]
            Self::Posix => {
                let mut command = PtyCommand::new("/bin/sh", cwd);
                command.args(["-c", source]);
                command
            }
            #[cfg(windows)]
            Self::Pwsh(path) | Self::WindowsPowerShell(path) => {
                let mut plan = PtyCommand::new(path, cwd);
                plan.args(["-NoLogo", "-NoProfile", "-Command", command::WRAPPER])
                    .env(command::SLOT, format!("{source}{}", command::EXIT));
                plan
            }
            #[cfg(windows)]
            Self::Cmd(path) => {
                let mut command = PtyCommand::new(path, cwd);
                command.raw_arg(format!("/d /s /c \"{source}\""));
                command
            }
        }
    }

    pub(crate) fn interactive(&self, cwd: &std::path::Path) -> (String, crate::pty::PtyCommand) {
        use crate::pty::PtyCommand;
        let (source, mut command) = match self {
            #[cfg(unix)]
            Self::Posix => {
                let shell = std::env::var_os("SHELL")
                    .filter(|v| !v.is_empty())
                    .or_else(account_shell)
                    .unwrap_or_else(|| {
                        if cfg!(target_os = "macos") {
                            "/bin/zsh"
                        } else {
                            "/bin/sh"
                        }
                        .into()
                    });
                let source = "exec \"$SHELL\" -l";
                let mut command = PtyCommand::new("/bin/sh", cwd);
                command.args(["-c", source]).env("SHELL", shell);
                (source.into(), command)
            }
            #[cfg(windows)]
            Self::Pwsh(path) | Self::WindowsPowerShell(path) => {
                let source = format!("& '{}' -NoLogo", path.to_string_lossy().replace('\'', "''"));
                let mut command = PtyCommand::new(path, cwd);
                command.arg("-NoLogo");
                (source, command)
            }
            #[cfg(windows)]
            Self::Cmd(path) => {
                let mut command = PtyCommand::new(path, cwd);
                command.args(["/d", "/q"]);
                ("%ComSpec% /d /q".into(), command)
            }
        };
        command
            .env("DISABLE_AUTO_UPDATE", "true")
            .env("DISABLE_UPDATE_PROMPT", "true");
        (source, command)
    }

    pub(crate) fn detect() -> Self {
        #[cfg(unix)]
        return Self::Posix;
        #[cfg(windows)]
        {
            fn find(name: &str, fallback: Option<PathBuf>) -> Option<PathBuf> {
                std::env::var_os("PATH")
                    .into_iter()
                    .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                    .map(|directory| directory.join(name))
                    .chain(fallback)
                    .find_map(|path| {
                        path.is_file()
                            .then(|| dunce::canonicalize(path).ok())
                            .flatten()
                    })
            }
            if let Some(path) = find(
                "pwsh.exe",
                std::env::var_os("ProgramFiles")
                    .map(|root| PathBuf::from(root).join(r"PowerShell\7\pwsh.exe")),
            ) {
                return Self::Pwsh(path);
            }
            let system = PathBuf::from(
                std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()),
            );
            if let Some(path) = find(
                "powershell.exe",
                Some(system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")),
            ) {
                return Self::WindowsPowerShell(path);
            }
            Self::Cmd(system.join(r"System32\cmd.exe"))
        }
    }

    pub(crate) fn guidance(&self) -> &'static str {
        match self {
            #[cfg(unix)]
            Self::Posix => "Commands use /bin/sh -c; write POSIX shell syntax.",
            #[cfg(windows)]
            Self::Pwsh(_) => {
                "Commands use PowerShell 7 (pwsh); write PowerShell syntax, not cmd or bash syntax."
            }
            #[cfg(windows)]
            Self::WindowsPowerShell(_) => {
                "Commands use Windows PowerShell 5.1; write PowerShell 5.1-compatible syntax, not cmd or bash syntax."
            }
            #[cfg(windows)]
            Self::Cmd(_) => "Commands use cmd.exe; write cmd syntax, not bash syntax.",
        }
    }
}

#[cfg(unix)]
fn account_shell() -> Option<std::ffi::OsString> {
    use std::{ffi::CStr, os::unix::ffi::OsStrExt};
    let mut buffer = vec![0u8; 65_536];
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: output pointers and scratch storage remain live through copying
    // pw_shell; no borrowed libc account memory escapes this function.
    unsafe {
        if libc::getpwuid_r(
            libc::geteuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
        {
            return None;
        }
        let entry = entry.assume_init();
        if entry.pw_shell.is_null() {
            return None;
        }
        let bytes = CStr::from_ptr(entry.pw_shell).to_bytes();
        (!bytes.is_empty()).then(|| std::ffi::OsStr::from_bytes(bytes).to_owned())
    }
}

#[cfg(windows)]
pub(crate) mod command {
    use super::ShellPlan;
    use std::{collections::BTreeMap, ffi::OsString, os::windows::ffi::OsStrExt, path::Path};

    pub(super) const SLOT: &str = "__MAKA_RUNTIME_POWERSHELL_COMMAND";
    pub(super) const WRAPPER: &str = "$__makaUtf8 = [System.Text.UTF8Encoding]::new($false)\n[Console]::InputEncoding = $__makaUtf8\n[Console]::OutputEncoding = $__makaUtf8\n$OutputEncoding = $__makaUtf8\n$__makaCommandText = [Environment]::GetEnvironmentVariable('__MAKA_RUNTIME_POWERSHELL_COMMAND')\n[Environment]::SetEnvironmentVariable('__MAKA_RUNTIME_POWERSHELL_COMMAND', $null)\n$__makaCommand = [ScriptBlock]::Create($__makaCommandText)\n. $__makaCommand";
    pub(super) const EXIT: &str = "\n$__makaOk = $?\nif (-not $__makaOk) { if ($LASTEXITCODE -is [int] -and $LASTEXITCODE -ne 0) { exit $LASTEXITCODE } else { exit 1 } }";

    pub(crate) struct Command {
        pub executable: Vec<u16>,
        pub line: Vec<u16>,
        pub environment: Vec<u16>,
    }

    pub(crate) fn prepare(shell: &ShellPlan, source: &str) -> std::io::Result<Command> {
        let (path, line, script) = match shell {
            ShellPlan::Pwsh(path) | ShellPlan::WindowsPowerShell(path) => {
                let line = [
                    path.to_str()
                        .ok_or_else(|| std::io::Error::other("shell path must be UTF-8"))?,
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    WRAPPER,
                ]
                .map(quote)
                .join(" ");
                (path, line, Some(format!("{source}{EXIT}")))
            }
            ShellPlan::Cmd(path) => (
                path,
                format!("{} /d /s /c \"{source}\"", quote(&path.to_string_lossy())),
                None,
            ),
        };
        let mut environment = BTreeMap::<Vec<u16>, (OsString, OsString)>::new();
        for (key, value) in std::env::vars_os() {
            let folded = key
                .to_string_lossy()
                .to_uppercase()
                .encode_utf16()
                .collect();
            environment.insert(folded, (key, value));
        }
        if let Some(script) = script {
            // Windows environment keys are case-insensitive.
            environment.insert(SLOT.encode_utf16().collect(), (SLOT.into(), script.into()));
        }
        let mut block = Vec::new();
        for (key, value) in environment.into_values() {
            block.extend(key.encode_wide());
            block.push('=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        Ok(Command {
            executable: wide(path)?,
            line: wide(Path::new(&line))?,
            environment: block,
        })
    }

    pub(crate) fn wide(path: &Path) -> std::io::Result<Vec<u16>> {
        let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(std::io::Error::other("NUL in process input"));
        }
        value.push(0);
        Ok(value)
    }

    pub(crate) fn quote(value: &str) -> String {
        let mut quoted = String::from("\"");
        let mut slashes = 0;
        for ch in value.chars() {
            if ch == '\\' {
                slashes += 1;
                continue;
            }
            quoted.extend(std::iter::repeat_n(
                '\\',
                if ch == '"' { slashes * 2 + 1 } else { slashes },
            ));
            quoted.push(ch);
            slashes = 0;
        }
        quoted.extend(std::iter::repeat_n('\\', slashes * 2));
        quoted.push('"');
        quoted
    }
}
