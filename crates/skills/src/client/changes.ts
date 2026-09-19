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

import { useEffect, useState } from 'react';
import type { ClientContext } from '@maka-agent/plugin-sdk/client';

export function useChanges(context: ClientContext) {
  const [revision, setRevision] = useState(0);
  const [failure, setFailure] = useState('');
  useEffect(() => {
    const lifetime = new AbortController();
    void (async () => {
      const changes = context.remote.stream<null, null>('changes');
      for await (const _ of changes(null, lifetime.signal)) {
        if (lifetime.signal.aborted) break;
        setRevision((value) => value + 1);
      }
    })().catch((error) => {
      if (!lifetime.signal.aborted && !context.signal.aborted) setFailure(String(error));
    });
    return () => lifetime.abort();
  }, [context]);
  return { revision, failure };
}
