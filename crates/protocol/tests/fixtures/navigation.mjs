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

import { readFileSync } from 'node:fs';
import { HOST_OPERATION_SPECS } from '../../../../packages/runtime-host/src/protocol/operations.ts';

const cases = JSON.parse(readFileSync(0, 'utf8'));
const results = cases.map(({ operation, direction, value }) => {
  try {
    const spec = HOST_OPERATION_SPECS[operation];
    return {
      ok: true,
      value: direction === 'input' ? spec.decodeInput(value) : spec.decodeOutput(value),
    };
  } catch {
    return { ok: false };
  }
});
console.log(JSON.stringify(results));
