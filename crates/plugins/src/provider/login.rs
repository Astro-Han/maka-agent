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

use super::{
    Context, Error, Provider,
    authentication::{Authenticate, Credential},
};
use crate::fiber::CallGuard;
use std::sync::Arc;

/// A single admitted login, independent of the originating client's lifetime.
pub struct AuthenticationCall {
    pub(super) call: CallGuard,
    pub(super) implementation: Arc<dyn Provider>,
    pub(super) request: Authenticate,
    pub(super) context: Context,
}

impl AuthenticationCall {
    pub async fn run(self) -> Result<Credential, Error> {
        let Self {
            call,
            implementation,
            request,
            context,
        } = self;
        let _call = call;
        let credential = implementation.authenticate(request, context).await?;
        credential.validate().map_err(Error::Invalid)?;
        Ok(credential)
    }
}
