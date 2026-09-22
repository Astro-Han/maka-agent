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

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act, createElement, useLayoutEffect } from 'react';
import type { ComposerPublication, ComposerSuggestion } from '@maka-agent/plugin-sdk/client';
import { ComposerMentionsProvider, useComposerMentionsContext, type ComposerMentions } from '../../renderer/composer-mentions.js';
import { usePublishComposerSuggestions } from '../../renderer/features/client-plugins/testing.js';
import { cleanupFakeDom, installReactRenderer } from './fake-dom.js';

test('composer publications retire by owner and cannot cross a target switch or replace the draft', async (t) => {
  const { root } = installReactRenderer();
  t.after(() => cleanupFakeDom());
  let publish: ((items: readonly ComposerSuggestion[]) => ComposerPublication) | undefined;
  let observed: ComposerMentions | undefined;
  const observations: string[][] = [];
  let draft: HTMLInputElement | undefined;
  function View() {
    publish = usePublishComposerSuggestions();
    const value = useComposerMentionsContext();
    useLayoutEffect(() => {
      observed = value;
      observations.push(value?.suggestions.map((item) => item.insertText) ?? []);
    });
    return createElement('input', {defaultValue:'unfinished draft', ref: (node: HTMLInputElement | null) => { if (node) draft = node; }});
  }
  const render = (scope: string) => act(() => root.render(createElement(ComposerMentionsProvider, {
    scope, sessionId:scope, children:createElement(View),
  })));
  await render('a');
  assert.ok(draft);
  const originalDraft = draft;
  let first!: ComposerPublication;
  let second!: ComposerPublication;
  const originalPublisher = publish!;
  const item = (text: string): ComposerSuggestion => ({id:'same-id',name:text,insertText:text});
  await act(() => {
    first = originalPublisher([item('/first ')]);
    second = originalPublisher([item('/second ')]);
  });
  assert.deepEqual(observed!.suggestions.map((item) => item.insertText), ['/first ', '/second ']);
  assert.notEqual(observed!.suggestions[0].id, observed!.suggestions[1].id);
  const snapshot = observed!.suggestions;
  await act(() => first.update([item('/first ')]));
  assert.equal(observed!.suggestions, snapshot, 'same-content refresh keeps the open menu stable');
  await act(() => first.update([{...item('/first '), tokenLabel:'First chip'}]));
  assert.equal(observed!.suggestions[0].tokenLabel, 'First chip');
  assert.equal(observed!.suggestions[0].id, snapshot[0].id);
  assert.equal(observed!.suggestions[0].insertText, snapshot[0].insertText);
  await act(() => first.update([item('/revised ')]));
  assert.equal(observed!.suggestions[0].id, snapshot[0].id);
  assert.equal(observed!.suggestions[0].insertText, '/revised ');
  await act(() => first.dispose());
  await act(() => first.update([item('/retired ')]));
  assert.deepEqual(observed!.suggestions.map((item) => item.insertText), ['/second ']);
  const before = observations.length;
  await render('b');
  assert.ok(observations.slice(before).every((items) => items.length === 0));
  assert.equal(draft, originalDraft);
  assert.equal(draft.value, 'unfinished draft');
  let current!: ComposerPublication;
  await act(() => {
    current = publish!([item('/current ')]);
    second.dispose();
    const late = originalPublisher([item('/late ')]);
    late.dispose();
  });
  assert.deepEqual(observed!.suggestions.map((item) => item.insertText), ['/current ']);
  await act(() => current.dispose());
  assert.deepEqual(observed!.suggestions, []);
});
