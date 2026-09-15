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

//! Ephemeral client capability ownership; durable execution remains in the log.
pub mod broker;
mod endpoint;
mod managed;
mod registry;
pub use managed::ManagedAdmissionError;
mod tool_name;
pub use tool_name::proxy_tool_name;

pub use endpoint::Endpoint;
pub use registry::{
    BindingError, BindingMode, PreparedBindings, Registration, Registry, RestoredBindings,
    Snapshot, SnapshotOffer,
};

pub use maka_runtime::capability::{ContractId, Identity, PrincipalKind};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("Client Capability registry is draining")]
    Draining,
    #[error("Client Capability reverse channel is unavailable")]
    Unavailable,
    #[error("Invalid Client Capability registration: {0}")]
    Invalid(&'static str),
}
