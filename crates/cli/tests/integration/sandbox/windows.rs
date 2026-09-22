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

use maka_process::{Command, bootstrap};
mod network;
mod unelevated;
use maka_runtime::terminal::TerminalSize;
use maka_runtime_host::sandbox::windows::{Installation, WriteAccess, WriteRule};
use maka_sandbox::{
    Network,
    filesystem::{Access, Rule, Scope},
    windows::{
        WriteCapability,
        acl::{self, Permission},
        ensure_drained,
    },
};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
};

#[tokio::test]
#[ignore = "requires administrator; creates and removes sandbox accounts, ACLs and WFP rules"]
async fn account_runner_preserves_pipe_and_console_ownership_under_isolation() {
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_owned();
    let state = root.join("state");
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let mut fixture = Fixture {
        installation: Installation::new(&state),
        grants: Vec::new(),
        jobs: Vec::new(),
        directory: Some(directory),
    };
    let mut host = super::super::candidate::CandidateFixture::new(state.clone());
    provision(&state, "setup").await;
    // Recovery must not strand Desktop behind a manual cleanup instruction.
    // Leave the durable removal intent without running its privileged phase.
    drop(Installation::new(&state).begin_removal().unwrap());
    assert_eq!(
        Installation::new(&state).status().unwrap(),
        maka_runtime_host::sandbox::windows::Status::Removing
    );
    provision(&state, "setup").await;
    let configured = std::fs::read(state.join("windows-sandbox/ready.json")).unwrap();
    provision(&state, "setup").await;
    assert_eq!(
        std::fs::read(state.join("windows-sandbox/ready.json")).unwrap(),
        configured
    );
    // Exercise the last offline account, not only the first ACE in the shared
    // WFP descriptor. Distinct read surfaces must not overwrite live peers.
    let mut occupied = Vec::new();
    for index in 0..7 {
        let path = root.join(format!("reserved-{index}"));
        std::fs::create_dir(&path).unwrap();
        let execution = fixture
            .installation
            .execution(Network::Denied, &[Rule::exact(path, Access::Read)], &[])
            .unwrap();
        fixture.jobs.push(execution.id);
        occupied.push(execution);
    }
    let mut executions = Vec::new();
    for _ in 0..3 {
        let execution = fixture
            .installation
            .execution(
                Network::Denied,
                &[],
                &[WriteRule {
                    path: work.clone(),
                    scope: Scope::Subtree,
                    access: WriteAccess::Allowed,
                }],
            )
            .unwrap();
        fixture.jobs.push(execution.id);
        executions.push(execution);
    }
    let mut participants: Vec<_> = executions
        .iter()
        .map(|execution| WriteCapability::new(execution.capability).sid().to_owned())
        .collect();
    let mut executions = executions.into_iter();
    let execution = executions.next().unwrap();
    let cap_id = execution.capability;
    let account = execution.account;
    participants.push(account.sid().to_owned());
    eprintln!(
        "runner acceptance account={}, executions={:?}",
        account.name(),
        fixture.jobs
    );
    std::fs::write(root.join("outside"), "untouched").unwrap();
    let executable = Path::new(env!("CARGO_BIN_EXE_maka"));
    fixture
        .grant(executable, account.sid(), Scope::Exact, Permission::Read)
        .unwrap();
    fixture
        .grant(&root, account.sid(), Scope::Subtree, Permission::Read)
        .unwrap();
    // PowerShell resolves provider locations through ancestor metadata. These
    // non-inheriting entries do not grant access to sibling file contents.
    for ancestor in root.ancestors().skip(1) {
        fixture
            .grant(ancestor, account.sid(), Scope::Exact, Permission::Read)
            .unwrap();
    }
    fixture
        .grant(&work, account.sid(), Scope::Subtree, Permission::Write)
        .unwrap();
    let desktop = Arc::new(
        bootstrap::desktop(executable, &account, &execution.password, &participants)
            .await
            .unwrap(),
    );
    let endpoint = bootstrap::Endpoint::new(&account).unwrap();
    let identity = bootstrap::Identity {
        account,
        password: execution.password,
        desktop: desktop.clone(),
        job: execution.job,
        leases: execution.leases,
    };
    let runner = identity
        .start(executable.to_owned(), endpoint.id())
        .await
        .unwrap();
    let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut command = Command::new(&powershell, &work);
    command.env("TEMP", &work).env("TMP", &work);
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
    command.arg(format!(r#"
        $ErrorActionPreference='Stop'
        [IO.File]::WriteAllText((Join-Path $pwd 'allowed'), 'written')
        try {{ [IO.File]::WriteAllText((Join-Path $pwd '..\outside'), 'escaped'); exit 72 }}
        catch {{ if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) {{ exit 73 }} }}
        try {{ $c=[Net.Sockets.TcpClient]::new(); $c.Connect('127.0.0.1',{port}); exit 74 }}
        catch {{ if ($_.Exception.GetBaseException().SocketErrorCode -ne [Net.Sockets.SocketError]::AccessDenied) {{ exit 75 }} }}
        [Console]::Write('runner-io'); exit 17
    "#));
    let mut spawned = runner
        .spawn_pipes(endpoint, command, vec![cap_id])
        .await
        .unwrap();
    drop(spawned.stdin);
    let mut output = String::new();
    let mut errors = Vec::new();
    let (status, read, error_read) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(
            spawned.child.wait(),
            spawned.stdout.read_to_string(&mut output),
            spawned.stderr.read_to_end(&mut errors)
        )
    })
    .await
    .expect("runner pipe ownership and tree settlement must be bounded");
    read.unwrap();
    error_read.unwrap();
    assert_eq!(status.unwrap().code(), Some(17), "{output}\n{errors:?}");
    assert_eq!(output, "runner-io");
    assert_eq!(
        std::fs::read_to_string(root.join("outside")).unwrap(),
        "untouched"
    );
    assert_eq!(
        std::fs::read_to_string(work.join("allowed")).unwrap(),
        "written"
    );
    drop(spawned.child);
    drop((spawned.stdout, spawned.stderr));
    let mut terminals = Vec::new();
    for execution in executions {
        let cap_id = execution.capability;
        let account = execution.account;
        let endpoint = bootstrap::Endpoint::new(&account).unwrap();
        let runner = bootstrap::Identity {
            account,
            password: execution.password,
            desktop: desktop.clone(),
            job: execution.job,
            leases: execution.leases,
        }
        .start(executable.to_owned(), endpoint.id())
        .await
        .unwrap();
        let mut command = Command::new(&powershell, &work);
        command.env("TEMP", &work).env("TMP", &work);
        command.args(["-NoLogo", "-NoProfile", "-Command"]);
        command.arg(
            r#"
            $ErrorActionPreference='Stop'
            [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
            [Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
            if ([Console]::IsInputRedirected -or [Console]::IsOutputRedirected) { exit 71 }
            try { [IO.File]::WriteAllText((Join-Path $pwd '..\outside'), 'escaped'); exit 72 }
            catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 73 } }
            $start = [Diagnostics.ProcessStartInfo]::new()
            $start.FileName = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
            $start.Arguments = '-NoLogo -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 30"'
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $background = [Diagnostics.Process]::Start($start)
            if ($background.HasExited) { exit 76 }
            [Console]::WriteLine('ready')
            $line = [Console]::ReadLine()
            [Console]::WriteLine('received:' + $line)
            [Console]::WriteLine('size:' + [Console]::WindowWidth + 'x' + [Console]::WindowHeight)
            [Console]::WriteLine('final-frame')
            exit 259
        "#,
        );
        let terminal = runner
            .spawn_pty(
                endpoint,
                command,
                vec![cap_id],
                TerminalSize::new(80, 24).unwrap(),
            )
            .await
            .unwrap();
        terminals.push(terminal);
    }
    // Terminating one account runner must not kill its live peer or poison
    // the next admission under the same private desktop and account.
    for (index, (mut child, io)) in terminals.into_iter().enumerate() {
        let terminate = index == 0;
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut output = Vec::new();
            let mut buffer = [0; 4096];
            while !String::from_utf8_lossy(&output).contains("ready") {
                let count = io.read(&mut buffer).await.unwrap();
                assert!(count > 0, "early EOF: {}", String::from_utf8_lossy(&output));
                output.extend_from_slice(&buffer[..count]);
                assert!(output.len() < 65536);
            }
            if terminate {
                assert!(child.terminate().unwrap());
            } else {
                child
                    .resize(TerminalSize::new(101, 37).unwrap())
                    .await
                    .unwrap();
                let mut input = "中文😀\r".as_bytes();
                while !input.is_empty() {
                    let count = io.write(input).await.unwrap();
                    assert!(count > 0);
                    input = &input[count..];
                }
            }
            let (status, ()) = tokio::join!(
                async {
                    let status = child.wait().await.unwrap();
                    child.close().await.unwrap();
                    status
                },
                async {
                    loop {
                        let count = io.read(&mut buffer).await.unwrap();
                        if count == 0 {
                            break;
                        }
                        output.extend_from_slice(&buffer[..count]);
                        assert!(output.len() < 65536);
                    }
                }
            );
            if !terminate {
                assert_eq!(status.code(), Some(259));
                let output = String::from_utf8(output).unwrap();
                for expected in ["received:中文😀", "size:101x37", "final-frame"] {
                    assert!(output.contains(expected), "{output:?}");
                }
            }
        })
        .await
        .expect("account console I/O, resize and settlement must be bounded");
        drop((child, io));
    }
    drop(desktop);
    drop(occupied);
    managed_cli(&state, &root, &work).await;
    network::verify(&state, &work).await;
    unelevated::verify(&root, &state, &work).unwrap();
    managed_host(&mut host, &root, &work).await;
    provision(&state, "remove").await;
    assert!(!state.join("windows-sandbox/installation.json").exists());
    drop(host);
    fixture.cleanup().unwrap();
}

async fn managed_host(
    host: &mut super::super::candidate::CandidateFixture,
    root: &Path,
    work: &Path,
) {
    use std::process::{Command, Stdio};
    let temporary = root.join("temporary");
    std::fs::create_dir(&temporary).unwrap();
    for reopened in [false, true] {
        host.child = Some(
            Command::new(env!("CARGO_BIN_EXE_maka"))
                .args(["host", "candidate", "--root"])
                .arg(&host.root)
                .args([
                    "--expected-root-id",
                    &host.root_id,
                    "--startup-attempt-id",
                    &uuid::Uuid::new_v4().to_string(),
                    "--owner-stdin",
                ])
                // Default policy must authorize a distinct temporary root.
                .env("TEMP", &temporary)
                .env("TMP", &temporary)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let registration = host.wait_for_registration();
        let mut client = tokio::process::Command::new("node");
        client
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
            .args([
                "--socket",
                registration["endpoint"].as_str().unwrap(),
                "--root-id",
                &host.root_id,
                "--sandbox-workspace",
            ])
            .arg(work)
            .env("MAKA_TEST_STATE_ROOT", &host.root)
            .kill_on_drop(true);
        if reopened {
            client.arg("--reopened");
        }
        let result = tokio::time::timeout(Duration::from_secs(60), client.output()).await;
        drop(host.child.as_mut().unwrap().stdin.take());
        let status = host.wait_for_exit();
        assert!(status.success(), "Host must drain: {status}");
        let output = result.expect("sandbox client did not finish").unwrap();
        assert!(output.status.success(), "sandbox client: {output:?}");
    }
}

async fn managed_cli(state: &Path, root: &Path, work: &Path) {
    let started = std::time::Instant::now();
    let pending = || {
        std::fs::read_dir(state.join("windows-sandbox"))
            .unwrap()
            .map(Result::unwrap)
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with("execution-"))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let previous = pending();
    let protected = work.join("nested/protected");
    std::fs::create_dir_all(&protected).unwrap();
    std::fs::write(protected.join("original"), "unchanged").unwrap();
    let secret = work.join("private");
    std::fs::create_dir(&secret).unwrap();
    let secret_file = secret.join("secret");
    std::fs::write(&secret_file, "must remain private").unwrap();
    let acl = std::process::Command::new("icacls.exe")
        .arg(&secret_file)
        .args(["/inheritance:d", "/grant", "*S-1-1-0:(R)"])
        .output()
        .unwrap();
    assert!(acl.status.success(), "{acl:?}");
    let policy = maka_sandbox::Sandbox::Managed {
        filesystem: maka_sandbox::filesystem::Policy {
            default: Access::Read,
            rules: vec![
                Rule::subtree(work, Access::Write),
                Rule::subtree(&protected, Access::Read),
                // A more specific allow must not reopen a glob-denied tree.
                Rule::exact(&secret_file, Access::Read),
                Rule::subtree(work.join("nested/missing"), Access::Read),
                Rule::subtree(work.join("created/deep/readonly"), Access::Read),
                Rule::exact(work.join(".marker"), Access::Read),
                Rule::subtree(state, Access::Deny),
            ],
            deny_globs: vec![format!("{}/*ivate", work.display())],
        },
        network: Network::Denied,
    };
    let path = root.join("policy.json");
    std::fs::write(&path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let output = tokio::time::timeout(Duration::from_secs(30),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["sandbox", "run", "--root"]).arg(state)
            .arg("--policy").arg(path).arg("--cwd").arg(work)
            .env("TEMP", work).env("TMP", work)
            .arg("--command").arg(r#"
                $ErrorActionPreference='Stop'
                try { [IO.File]::ReadAllText((Join-Path $pwd 'private/secret')); exit 89 }
                catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 90 } }
                [IO.File]::WriteAllText((Join-Path $pwd 'managed'), 'accepted')
                [IO.File]::WriteAllText((Join-Path $pwd 'created/user-content'), 'keep')
                try { [IO.File]::WriteAllText((Join-Path $pwd '.marker'), 'escaped'); exit 85 }
                catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 86 } }
                try { [IO.File]::WriteAllText((Join-Path $pwd 'nested/missing/new'), 'escaped'); exit 87 }
                catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 88 } }
                try { [IO.File]::WriteAllText((Join-Path $pwd 'nested/protected/original'), 'escaped'); exit 81 }
                catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 82 } }
                try { [IO.Directory]::Move((Join-Path $pwd 'nested'), (Join-Path $pwd 'moved')); exit 83 }
                catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 84 } }
                [Console]::Write('managed-settled')
            "#).kill_on_drop(true).output()
    ).await.expect("managed CLI must settle").unwrap();
    assert!(output.status.success(), "managed CLI: {output:?}");
    eprintln!(
        "managed CLI with real account profile settled in {:?}",
        started.elapsed()
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("managed-settled"),
        "{output:?}"
    );
    assert_eq!(
        std::fs::read_to_string(protected.join("original")).unwrap(),
        "unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(work.join("managed")).unwrap(),
        "accepted"
    );
    for path in ["nested/missing", "created/deep", ".marker"] {
        assert!(
            !work.join(path).exists(),
            "synthetic boundary was not collected: {path}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(work.join("created/user-content")).unwrap(),
        "keep"
    );
    // Previous raw runner fixtures intentionally leave their recovery intents.
    // The managed command owns and removes only its own accepted execution.
    assert!(
        pending().is_subset(&previous),
        "managed settlement left a new ACL intent"
    );
}

async fn provision(root: &Path, operation: &str) {
    let output = tokio::time::timeout(
        Duration::from_secs(90),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["sandbox", operation, "--root"])
            .arg(root)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("administrative helper did not settle")
    .unwrap();
    assert!(output.status.success(), "{operation}: {output:?}");
}

struct Fixture {
    installation: Installation,
    grants: Vec<(File, String, Scope)>,
    jobs: Vec<uuid::Uuid>,
    directory: Option<tempfile::TempDir>,
}
impl Fixture {
    fn grant(
        &mut self,
        path: &Path,
        sid: &str,
        scope: Scope,
        permission: Permission,
    ) -> io::Result<()> {
        let file = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        self.grants.push((file, sid.into(), scope));
        let (file, _, _) = self.grants.last().unwrap();
        acl::set(file, sid, scope, Some(permission))
    }
    fn cleanup(&mut self) -> io::Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        for id in &self.jobs {
            loop {
                match ensure_drained(*id) {
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    result => {
                        result?;
                        break;
                    }
                }
            }
        }
        for (file, sid, scope) in self.grants.iter().rev() {
            acl::set(file, sid, *scope, None)?;
        }
        self.grants.clear();
        let removal = self.installation.begin_removal()?;
        let receipt = removal
            .request()
            .map(|request| request.apply())
            .transpose()?;
        removal.finish(receipt)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            let root = self.directory.take().unwrap().keep();
            eprintln!(
                "runner cleanup failed: {error}; fixture retained at {}",
                root.display()
            );
        }
    }
}
