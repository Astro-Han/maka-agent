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

use super::{Endpoint, Runner};
use maka_sandbox::{Network, filesystem::Policy};
use std::{future::Future, io, path::PathBuf, pin::Pin};

pub type Preparation = Pin<Box<dyn Future<Output = io::Result<Launch>> + Send>>;

/// The process layer owns native I/O and process lifetime, while the executor
/// owns account provisioning and recoverable filesystem authority. Preparation
/// is invoked only after durable execution admission, never while capturing a
/// command or deciding whether an approval is needed.
pub trait Backend: Send + Sync {
    fn prepare(
        &self,
        filesystem: Policy,
        network: Network,
        route: maka_network::Policy,
        executable: PathBuf,
        cwd: PathBuf,
    ) -> Preparation;
}

/// One already prepared, uniquely owned command tree. There is no fallback to
/// an unisolated launch if its runner, transport or settlement fails.
pub struct Launch {
    pub runner: Runner,
    pub endpoint: Endpoint,
    pub capabilities: Vec<uuid::Uuid>,
    pub proxy_address: Option<std::net::SocketAddr>,
}
