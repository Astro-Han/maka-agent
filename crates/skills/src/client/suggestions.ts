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

import { useEffect, useRef, useState } from 'react';
import type { ComposerPublication, ComposerSuggestion } from '@maka-agent/plugin-sdk/client';
import type { Call } from './model.js';

/** Suggestions share the authoritative paged query with the visible picker. */
export function useSuggestions(
  call: Call,
  publish: ((items: readonly ComposerSuggestion[]) => ComposerPublication) | undefined,
  revision: string,
) {
  const [failure, setFailure] = useState('');
  const publication = useRef<ComposerPublication | undefined>(undefined);
  useEffect(() => {
    // A changed target retires immediately; same-target refreshes replace one
    // publication atomically, without tearing down an unchanged open menu.
    void call;
    const owner = publish?.([]);
    publication.current = owner;
    return () => {
      owner?.dispose();
      publication.current = undefined;
    };
  }, [call, publish]);
  useEffect(() => {
    const owner = publication.current;
    if (!owner) return;
    let cancelled = false;
    setFailure('');
    void (async () => {
      const items: ComposerSuggestion[] = [];
      let page: { revision: string; cursor: string } | null = null;
      do {
        const result = await call({ kind: 'invocable', page });
        if (cancelled) return;
        if (result.kind !== 'page' || 'view' in result)
          throw new Error('Skills changed during pagination');
        items.push(
          ...result.items.map((item) => ({
            id: item.ref,
            name: item.name,
            description: item.description,
            insertText: '/skill:' + item.id + ' ',
          })),
        );
        if (items.length > 4096) throw new Error('Skill suggestions exceed the catalog limit');
        page = result.nextCursor ? { revision: result.revision, cursor: result.nextCursor } : null;
      } while (page);
      if (!cancelled) owner.update(items);
    })().catch((error) => {
      if (!cancelled) {
        owner.update([]);
        setFailure(String(error));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [call, publish, revision]);
  return failure;
}
