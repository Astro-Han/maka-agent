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

import type { PipeShellOutput } from '@maka/core/shell-run';

import { ShellTailBuffer } from './shell-tail-buffer.js';
import { SHELL_MAX_RETAINED_CHARS } from './shell-exec.js';

export class PipeTailCollector {
  private readonly stdout: ShellTailBuffer;
  private readonly stderr: ShellTailBuffer;
  private stdoutChars = 0;
  private stderrChars = 0;
  private generation = 0;
  private latestStream: 'stdout' | 'stderr' | undefined;

  constructor(maxRetainedChars = SHELL_MAX_RETAINED_CHARS) {
    this.stdout = new ShellTailBuffer(maxRetainedChars);
    this.stderr = new ShellTailBuffer(maxRetainedChars);
  }

  accept(stream: 'stdout' | 'stderr', chunk: string): number {
    if (!chunk) return this.generation;
    if (chunk.trim()) this.latestStream = stream;
    if (stream === 'stdout') {
      this.stdout.push(chunk);
      this.stdoutChars += chunk.length;
    } else {
      this.stderr.push(chunk);
      this.stderrChars += chunk.length;
    }
    this.generation += 1;
    return this.generation;
  }

  snapshot(): PipeShellOutput {
    const stdout = this.stdout.value();
    const stderr = this.stderr.value();
    return {
      mode: 'pipes',
      stdout,
      stderr,
      ...(this.latestStream ? { latestStream: this.latestStream } : {}),
      stdoutTruncated: this.stdoutChars > stdout.length,
      stderrTruncated: this.stderrChars > stderr.length,
      redacted: false,
    };
  }

  currentGeneration(): number {
    return this.generation;
  }
}
