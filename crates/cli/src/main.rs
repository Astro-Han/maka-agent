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

use clap::Parser;
mod args;
mod candidate;
mod code;
mod deployment;
mod endpoint;
mod host_client;
mod serve;
mod signals;
mod stdio;
#[cfg(windows)]
mod windows;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = args::Cli::parse();
    let error_exit = cli.error_exit_code();
    let result = cli.run().await;
    if let Err(error) = result {
        eprintln!("{error}");
        // Let Tokio wait for accepted blocking work to finish before the OS
        // releases its leases. A reported timeout does not cancel that work.
        std::process::ExitCode::from(error_exit)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
