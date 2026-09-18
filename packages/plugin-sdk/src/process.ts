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

export interface ProcessCommand {
  /** Absolute executable path. argv is never interpreted by a shell. */
  executable: string;
  args?: readonly string[];
  env?: Readonly<Record<string, string>>;
  /** Default: invocation. Instance processes survive calls, not plugin retirement. */
  lifetime?: 'invocation' | 'instance';
}
export interface ProcessExit {
  code: number | null;
  success: boolean;
  stopped: boolean;
  error: string | null;
}
export interface ProcessChunk {
  stream: 'stdout' | 'stderr';
  bytes: Uint8Array;
}
export interface Process {
  readonly id: string;
  /** Strings are UTF-8; bytes are unchanged. Resolves after bounded queue admission. */
  write(data: string | Uint8Array | readonly number[]): Promise<void>;
  endInput(): Promise<void>;
  /** One pending read. null means end of output, not successful process exit. */
  next(): Promise<ProcessChunk | null>;
  /** Also waits for owned descendants and output cleanup. */
  wait(): Promise<ProcessExit>;
  close(): Promise<void>;
}
export interface Processes {
  /** Requires this call's current Bypass permission and uses its frozen workspace. */
  spawn(command: ProcessCommand): Promise<Process>;
  /** Rebinds an instance process to this invocation; old call handles stay revoked. */
  open(id: string): Process;
}
