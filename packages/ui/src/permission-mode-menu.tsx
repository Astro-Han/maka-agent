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

import type { ChatDefaultSandboxMode } from '@maka/core/settings';
import type { SandboxMode } from '@maka/core/permission';
import type { ApprovalPolicy } from '@maka/core/execution-permissions';
import { CHAT_DEFAULT_SANDBOX_MODES } from '@maka/core/settings';
import type { UiLocale } from '@maka/core/ui-locale';
import {
  Selector,
  SelectorOption,
  type SelectorOptionData,
} from '@astryxdesign/core/Selector';
import {
  DropdownMenu,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuCheckboxItem,
  DropdownMenuDivider,
  DropdownMenuItem,
} from '@astryxdesign/core/DropdownMenu';
import { ICON_SIZE, Eye, ShieldAlert, ShieldCheck } from './icons.js';
import { useUiLocale } from './locale-context.js';
import { getConversationCopy } from './conversation-copy.js';
import { cn } from './utils.js';

type SandboxModeAppearance = 'field' | 'icon';

function sandboxModeIcon(mode: SandboxMode) {
  if (mode === 'danger-full-access') return <ShieldAlert size={ICON_SIZE.control} aria-hidden="true" />;
  if (mode === 'read-only') return <Eye size={ICON_SIZE.control} aria-hidden="true" />;
  return <ShieldCheck size={ICON_SIZE.control} aria-hidden="true" />;
}

export interface SandboxModeMeta {
  label: string;
  hint: string;
}

/** Shared labels and hints for the composer and settings. */
export function getSandboxModeMeta(locale: UiLocale): Record<SandboxMode, SandboxModeMeta> {
  return getConversationCopy(locale).permissions.mode;
}

/** User-selectable modes in canonical display order. */
export const SANDBOX_MODE_ORDER: readonly ChatDefaultSandboxMode[] = CHAT_DEFAULT_SANDBOX_MODES;

/** Field in settings; compact icon menu in the composer. */
export function SandboxModeSelect(props: {
  activeMode: SandboxMode;
  onSelect(mode: ChatDefaultSandboxMode): void | Promise<void>;
  align?: 'start' | 'end';
  disabled?: boolean;
  disabledReason?: string;
  ariaLabel?: string;
  className?: string;
  appearance?: SandboxModeAppearance;
  approval?: {
    policy: ApprovalPolicy;
    onChange(policy: ApprovalPolicy): void | Promise<void>;
    onDisableProtections?(): void | Promise<void>;
  };
}) {
  const locale = useUiLocale();
  const permissionCopy = getConversationCopy(locale).permissions;
  const modeMeta = getSandboxModeMeta(locale);
  const displayMode: SandboxMode = props.activeMode;
  const meta = modeMeta[displayMode];
  const selectedValue = displayMode;
  const fullyUnrestricted = displayMode === 'danger-full-access' && props.approval?.policy.kind === 'never';
  const options: SelectorOptionData[] = SANDBOX_MODE_ORDER.map((mode) => ({
    value: mode,
    label: modeMeta[mode].label,
  }));
  const modeLabel = fullyUnrestricted ? permissionCopy.approval.unrestricted : meta.label;
  const ariaLabel = props.ariaLabel ?? permissionCopy.modeAriaLabel(modeLabel);

  // Composer footer: match the ＋ ghost icon button. Astryx puts DropdownMenu
  // className on the panel, so product anchors wrap the whole control.
  if (props.appearance === 'icon') {
    return (
      <span className={cn('sandboxModeIcon', props.className)}>
        <DropdownMenu
          placement="above"
          hasChevron={false}
          className="maka-composer-quiet-menu"
          button={{
            label: ariaLabel,
            icon: sandboxModeIcon(displayMode),
            isIconOnly: true,
            variant: 'ghost',
            size: 'sm',
            isDisabled: props.disabled,
            tooltip: props.disabledReason ?? `${modeLabel} — ${meta.hint}`,
            'aria-description': meta.hint,
          }}
        >
          <DropdownMenuRadioGroup
            value={selectedValue}
            label={ariaLabel}
            onChange={(value) => {
              void props.onSelect(value as ChatDefaultSandboxMode);
            }}
          >
            {SANDBOX_MODE_ORDER.map((mode) => (
              <DropdownMenuRadioItem
                key={mode}
                value={mode}
                label={modeMeta[mode].label}
                icon={sandboxModeIcon(mode)}
                isDisabled={props.disabled}
              />
            ))}
          </DropdownMenuRadioGroup>
          {props.approval ? (
            <>
              <DropdownMenuDivider />
              <DropdownMenuRadioGroup
                value={props.approval.policy.kind}
                label={permissionCopy.approval.label}
                onChange={(value) => {
                  if (value === props.approval?.policy.kind) return;
                  const policy: ApprovalPolicy = value === 'granular'
                    ? { kind: 'granular', sandbox: false, rules: false, permissions: false, client: false }
                    : { kind: value === 'on-request' ? 'on-request' : 'never' };
                  void props.approval?.onChange(policy);
                }}
              >
                {(['on-request', 'never', 'granular'] as const).map((kind) => (
                  <DropdownMenuRadioItem
                    key={kind}
                    value={kind}
                    label={permissionCopy.approval.modes[kind]}
                    isDisabled={props.disabled}
                    aria-description={permissionCopy.approval.hint}
                  />
                ))}
              </DropdownMenuRadioGroup>
              {props.approval.policy.kind === 'granular'
                ? (['sandbox', 'rules', 'permissions', 'client'] as const).map((category) => (
                    <DropdownMenuCheckboxItem
                      key={category}
                      label={permissionCopy.approval.categories[category]}
                      value={props.approval?.policy.kind === 'granular' && props.approval.policy[category]}
                      isDisabled={props.disabled}
                      onChange={(enabled) => {
                        if (props.approval?.policy.kind === 'granular') {
                          void props.approval.onChange({ ...props.approval.policy, [category]: enabled });
                        }
                      }}
                    />
                  ))
                : null}
              {props.approval.onDisableProtections ? (
                <>
                  <DropdownMenuDivider />
                  <DropdownMenuItem
                    label={permissionCopy.approval.unrestricted}
                    icon={<ShieldAlert size={ICON_SIZE.control} aria-hidden="true" />}
                    isDisabled={props.disabled || fullyUnrestricted}
                    description={permissionCopy.approval.unrestrictedHint}
                    onClick={() => { void props.approval?.onDisableProtections?.(); }}
                  />
                </>
              ) : null}
            </>
          ) : null}
        </DropdownMenu>
      </span>
    );
  }

  return (
    <Selector
      label={ariaLabel}
      isLabelHidden
      value={selectedValue}
      placeholder={meta.label}
      options={options}
      onChange={(value) =>
        void props.onSelect(value as ChatDefaultSandboxMode)
      }
      isDisabled={props.disabled}
      disabledMessage={props.disabledReason}
      aria-description={meta.hint}
      placement="below"
      className={cn('sandboxModeSelector', props.className)}
      renderOption={(option) => (
        <SelectorOption
          icon={sandboxModeIcon(option.value as SandboxMode)}
          label={option.label ?? option.value}
          description={
            modeMeta[option.value as ChatDefaultSandboxMode].hint
          }
        />
      )}
    />
  );
}
