import { Button } from '@maka/ui';
import { SettingsField, SettingsRow, SettingsSection } from './settings-section';
import { workspaceInstructionStatusLabel } from './memory-settings-labels';
import type { MemorySettingsCopy } from '../locales/settings-memory-copy';

type WorkspaceInstructionState = Awaited<ReturnType<typeof window.maka.workspaceInstructions.getState>>;

export function WorkspaceInstructionsSection(props: {
  state: WorkspaceInstructionState | null;
  copy: MemorySettingsCopy;
  disabled: boolean;
  isActionPending(key: string): boolean;
  onOpen(file: string): void | Promise<void>;
  onCreate(file: string): void | Promise<void>;
}) {
  if (!props.state) return null;
  /* Was a nested gray box inside the section ("no Cards in Cards") with the
     detected-count heading, the char-limit note, and per-file chip buttons all
     crammed on one line. Now: one summary row plus one row per instruction
     file, sharing the group's divider rhythm; the open/create affordance is a
     quiet ghost action on the row it belongs to. */
  return (
    <>
      <SettingsRow
        label={props.copy.detectedInstructions(props.state.detectedCount)}
        description={props.copy.instructionLimit(props.state.fileCharLimit)}
      />
      {props.state.files.map((file) => (
        <SettingsRow
          key={file.file}
          label={<code>{file.file}</code>}
          description={workspaceInstructionStatusLabel(file.status, file.chars, file.truncated, props.copy)}
          end={(file.status === 'available' || file.status === 'empty') ? (
            <Button
              variant="ghost"
              size="sm"
              aria-label={props.copy.openInstructionAria(file.file)}
              isDisabled={props.disabled || props.isActionPending(`instruction:${file.file}:open`)}
              onClick={() => void props.onOpen(file.file)}
              label={props.isActionPending(`instruction:${file.file}:open`) ? props.copy.text.opening : props.copy.text.instructionOpen}
            />
          ) : file.status === 'missing' ? (
            <Button
              variant="ghost"
              size="sm"
              aria-label={props.copy.createInstructionAria(file.file)}
              isDisabled={props.disabled || props.isActionPending(`instruction:${file.file}:create`)}
              onClick={() => void props.onCreate(file.file)}
              label={props.isActionPending(`instruction:${file.file}:create`) ? props.copy.text.creating : props.copy.text.instructionCreate}
            />
          ) : undefined}
        />
      ))}
    </>
  );
}

export function MemoryPromptPreviewSection(props: {
  copy: MemorySettingsCopy;
  active: boolean;
  preview: string;
  budgetLabel: string;
  blockedReason: string;
  safeMode: boolean;
  copyPending: boolean;
  onCopy(): void | Promise<void>;
}) {
  /* Was a muted card holding a bordered <pre> — a container in a container in
     the page ("no Cards in Cards"). Now a first-class section: the will/won't
     inject state reads as quiet header-adjacent text, the copy affordance is
     the section's one action, and the <pre> is the single owned surface. */
  return (
    <SettingsSection
      title={props.copy.text.promptPreview}
      description={props.copy.text.promptPreviewHelp}
      action={(
        <div className="settingsFormRowControlCluster">
          <span className="settingsMemoryInjectState" data-active={props.active ? 'true' : 'false'}>
            {props.active ? props.copy.text.willInject : props.copy.text.willNotInject}
          </span>
          <Button
            variant="ghost"
            size="sm"
            isDisabled={!props.preview || props.copyPending}
            onClick={() => void props.onCopy()}
            label={props.copyPending ? props.copy.text.copying : props.copy.text.copyContext}
          />
        </div>
      )}
    >
      <SettingsField className="settingsMemoryPromptPreview">
        <small className="settingsMemoryPromptPreviewBudget">{props.budgetLabel}</small>
        {props.preview ? (
          <pre>{props.preview}</pre>
        ) : (
          <p>{props.safeMode ? props.copy.text.safeModePreview : props.copy.text.emptyPromptPreview}</p>
        )}
        {props.blockedReason && props.preview && <small>{props.blockedReason}</small>}
      </SettingsField>
    </SettingsSection>
  );
}
