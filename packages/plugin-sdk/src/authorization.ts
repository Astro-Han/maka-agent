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

export type AuthorizationCapability =
  | 'read_files'
  | 'write_files'
  | 'network'
  | 'models'
  | 'processes'
  | 'client_capabilities'
  | 'executions'
  | 'notifications';
export type AuthorizationScope = 'profile' | `session:${string}`;
export interface AuthorizationRequest {
  /** Retain across lost replies; changing the proposal requires a new UUID. */
  readonly operationId: string;
  readonly title: string;
  readonly capabilities: readonly AuthorizationCapability[];
  readonly target:
    | { readonly kind: 'profile' }
    | { readonly kind: 'session'; readonly sessionId: string }
    | {
        readonly kind: 'workspace';
        readonly workspace:
          | { readonly kind: 'project'; readonly projectId: string }
          | { readonly kind: 'host_path'; readonly path: string };
        readonly permissionMode: 'explore' | 'ask' | 'bypass';
      };
}
/** A durable reference, never a bearer permission. */
export interface AuthorizationGrant {
  readonly id: string;
  readonly request: AuthorizationRequest;
  readonly revoked: boolean;
}
export interface ClientAuthorization {
  /** The application presents consent outside plugin UI. Cancellation returns null. */
  approve(
    scope: AuthorizationScope,
    request: AuthorizationRequest,
  ): Promise<AuthorizationGrant | null>;
  query(scope: AuthorizationScope, id: string): Promise<AuthorizationGrant | null>;
  revoke(scope: AuthorizationScope, id: string): Promise<void>;
}
