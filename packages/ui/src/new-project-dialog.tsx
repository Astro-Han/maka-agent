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

import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Button, HStack, TextInput } from '@astryxdesign/core';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { Layout, LayoutContent, LayoutFooter } from '@astryxdesign/core/Layout';
import { getConversationCopy } from './conversation-copy.js';
import { useUiLocale } from './locale-context.js';

/** Collect a name before the caller opens its directory picker. */
export function NewProjectDialog(props: {
  isDisabled?: boolean;
  onOpenChange(open: boolean): void;
  /** The typed name, trimmed and non-empty; the caller opens the folder picker. */
  onSubmit(name: string): void;
}) {
  const copy = getConversationCopy(useUiLocale()).workspace;
  const [name, setName] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    const focusAndSelect = () => {
      input.focus({ preventScroll: true });
      input.select();
    };
    focusAndSelect();
    const frame = window.requestAnimationFrame(() => {
      // Closing a menu and opening a native dialog both manage focus. If either
      // handoff wins after this effect, take ownership back once it has settled.
      if (document.activeElement !== input) focusAndSelect();
    });
    return () => window.cancelAnimationFrame(frame);
  }, []);

  const trimmed = name.trim();

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!trimmed || props.isDisabled) return;
    props.onOpenChange(false);
    props.onSubmit(trimmed);
  }

  return (
    <Dialog isOpen onOpenChange={props.onOpenChange} purpose="form" width={440}>
      <Layout
        header={
          <DialogHeader title={copy.newProjectTitle} onOpenChange={props.onOpenChange} />
        }
        footer={
          <LayoutFooter>
            <HStack gap={2} hAlign="end">
              <Button
                variant="primary"
                type="submit"
                form="maka-new-project-form"
                isDisabled={!trimmed || props.isDisabled}
                label={copy.newProjectSubmit}
              />
            </HStack>
          </LayoutFooter>
        }
        content={
          <LayoutContent>
            <form id="maka-new-project-form" onSubmit={submit}>
              <TextInput
                ref={inputRef}
                label={copy.newProjectNameLabel}
                description={props.isDisabled ? copy.newProjectUnavailable : copy.newProjectDescription}
                value={name}
                // A project name is a label, not a document; the same 80 the
                // titlebar's session field takes.
                onChange={(value) => setName(value.slice(0, 80))}
                hasAutoFocus
                width="100%"
              />
            </form>
          </LayoutContent>
        }
      />
    </Dialog>
  );
}
