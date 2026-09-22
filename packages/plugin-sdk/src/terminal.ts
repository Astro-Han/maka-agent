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

import type { ProcessCommand } from './process.js';

export interface TerminalSize {
  cols: number;
  rows: number;
}
export type TerminalOutput =
  | { kind: 'data'; sequence: number; text: string }
  /** Initial/recovery cut of the bounded raw output tail; replace prior output. */
  | { kind: 'reset'; sequence: number; text: string; size: TerminalSize }
  | { kind: 'closed' };
export type TerminalOutcome =
  | { kind: 'completed' }
  | { kind: 'exited'; code: number; message: string | null }
  | { kind: 'failed' | 'orphaned'; message: string }
  | { kind: 'timed_out' | 'cancelled'; message: string | null };
export interface TerminalWrite {
  /** Transport-accepted UTF-8 prefix, not application consumption. */
  acceptedBytes: number;
  resized: boolean;
}
export interface Terminal {
  readonly id: string;
  /** Raw UTF-8/ESC/C0 keystrokes (Enter is CR). Resize and input are serialized together. */
  write(text: string, size?: TerminalSize): Promise<TerminalWrite>;
  resize(size: TerminalSize): Promise<TerminalWrite>;
  /** One pending read; slow consumers receive an explicit reset, never silent truncation. */
  next(): Promise<TerminalOutput>;
  /** Wait for durable exit AND native resource cleanup, not merely output EOF. */
  wait(): Promise<TerminalOutcome>;
  /** Signal independently of input/output backpressure and confirm cleanup. */
  close(): Promise<void>;
}
export interface Terminals {
  /** Uses the same authorized OS sandbox as processes, plus Host PTY limits. */
  spawn(command: ProcessCommand, size?: TerminalSize): Promise<Terminal>;
  /** Rebind an instance terminal to a later invocation of the same Session. */
  open(id: string): Terminal;
}
