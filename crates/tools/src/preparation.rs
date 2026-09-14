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

use maka_runtime::{
    event::Invocation,
    tool_call::{ToolRejection, tool_use_id},
    tool_output::ToolSuccess,
    tools::{ToolExecutor, ToolFuture},
};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type PreparedEffect = Box<dyn FnOnce(CancellationToken) -> ToolFuture<ToolSuccess> + Send>;
pub type PreparationFuture =
    Pin<Box<dyn Future<Output = Result<PreparedEffect, ToolRejection>> + Send>>;

/// Host-owned identity; neither model arguments nor Code Mode may choose it.
#[derive(Clone)]
pub struct ToolCallContext {
    pub invocation: Invocation,
    pub operation_id: String,
}

impl ToolCallContext {
    pub fn tool_use_id(&self) -> String {
        tool_use_id(&self.invocation.invocation_id, &self.operation_id)
    }
}

/// Preparation may parse a remote call and obtain policy approval, but must not
/// admit its effect. A returned one-shot effect is invoked only after durable T1.
pub trait ToolPreparer: Send + Sync + 'static {
    fn names(&self) -> Vec<String>;
    fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        cancellation: CancellationToken,
    ) -> PreparationFuture;
}

#[derive(Clone)]
pub enum ToolHandler {
    Immediate(Arc<dyn ToolExecutor>),
    Prepared(Arc<dyn ToolPreparer>),
}
impl ToolHandler {
    pub(crate) fn names(&self) -> Vec<String> {
        match self {
            Self::Immediate(executor) => executor.names(),
            Self::Prepared(preparer) => preparer.names(),
        }
    }
    /// Prepare arguments already checked against the owning catalog's schema.
    pub fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        cancellation: CancellationToken,
    ) -> PreparationFuture {
        match self {
            Self::Immediate(executor) => {
                let executor = executor.clone();
                Box::pin(async move {
                    let effect: PreparedEffect = Box::new(move |cancellation| {
                        Box::pin(async move {
                            executor
                                .invoke(name, input, cancellation)
                                .await
                                .map(ToolSuccess::from)
                        })
                    });
                    Ok(effect)
                })
            }
            Self::Prepared(preparer) => preparer.prepare(name, input, context, cancellation),
        }
    }
}
