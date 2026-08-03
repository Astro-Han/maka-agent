// 设置 · 子 Agent — the approved model routes the main agent may delegate to.
//
// Two levels, one container, one back affordance, and nothing modal:
//
//   list ── editor(new | existing)
//
// The editor was a 560px Dialog holding eight fields behind an inner
// scrollbar. A modal exists to interrupt the current task for something short
// and immediately decidable while preserving the context behind it; naming a
// capability, writing the guidance the main agent selects on, and picking a
// connection/model/thinking route is none of those. Astryx says the same —
// "if the content grows beyond what fits, consider a full page instead" — and
// the providers panel next door already answers this exact shape with a route
// level, so this page follows it rather than inventing a second answer.
//
// Every element here is a settings-kit part (`SettingsSection`, `SettingsRow`,
// `SettingsField`, `SettingsActions`) or an Astryx primitive. The page owns no
// CSS: the three hand-written `.subagentPreset*` rules — a flex-wrap action
// cluster, an oklch-tinted warning callout, and a flex-end button row — were
// each a restatement of something the kit or Astryx already draws (the row end
// slot, `Banner status="warning"`, `SettingsActions`).
import { useMemo, useState } from 'react';
import { Badge, Banner, HStack, VStack } from '@astryxdesign/core';
import {
  connectionEnabledModelIds,
  isSafeSubagentPresetId,
  MAX_SUBAGENT_PRESETS,
  thinkingVariantsForModel,
  type AppSettings,
  type LlmConnection,
  type SubagentPreset,
  type SubagentProfile,
  type ThinkingLevel,
  type UpdateAppSettingsResult,
} from '@maka/core';
import {
  Button,
  EmptyState,
  IconButton,
  Selector,
  Switch,
  TextArea,
  TextInput,
  type SelectorOptionData,
  useToast,
  useUiLocale,
} from '@maka/ui';
import { ChevronRight } from '@maka/ui/icons';
import { getSubagentSettingsCopy } from '../locales/settings-subagents-copy.js';
import { settingsActionErrorMessage } from './settings-error-copy.js';
import { SettingsRouteHeader } from './settings-route-header.js';
import {
  SettingsActions,
  SettingsField,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from './settings-section.js';
import {
  subagentPresetAvailability,
  suggestSubagentPresetId,
} from './subagent-preset-presentation.js';
import { statusBadgeVariant } from './settings-status-badge.js';

/** `null` is the unsaved new preset; a string is an existing preset's id. */
type EditorRoute = { presetId: string | null };

type EditorDraft = {
  id: string;
  name: string;
  description: string;
  profile: SubagentProfile;
  connectionSlug: string;
  model: string;
  thinkingLevel: ThinkingLevel | '';
};

export function SubagentSettingsPage(props: {
  settings: AppSettings;
  connections: readonly LlmConnection[];
  onUpdate(
    patch: Parameters<typeof window.maka.settings.update>[0],
  ): Promise<UpdateAppSettingsResult>;
}) {
  const locale = useUiLocale();
  const copy = getSubagentSettingsCopy(locale);
  const toast = useToast();
  const [route, setRoute] = useState<EditorRoute | null>(null);
  const [saving, setSaving] = useState(false);
  const presets = props.settings.subagents.presets;
  const editorPreset = route?.presetId == null
    ? null
    : presets.find((preset) => preset.id === route.presetId) ?? null;
  const atLimit = presets.length >= MAX_SUBAGENT_PRESETS;

  async function persist(nextPresets: SubagentPreset[]): Promise<boolean> {
    setSaving(true);
    try {
      await props.onUpdate({ subagents: { presets: nextPresets } });
      return true;
    } catch (error) {
      toast.error(copy.toast.saveFailed, settingsActionErrorMessage(error, locale));
      return false;
    } finally {
      setSaving(false);
    }
  }

  async function removePreset(preset: SubagentPreset): Promise<void> {
    const confirmed = await toast.confirm({
      title: copy.remove.title(preset.name),
      description: copy.remove.description,
      confirmLabel: copy.remove.confirm,
      cancelLabel: copy.remove.cancel,
      destructive: true,
    });
    if (!confirmed) return;
    const removed = await persist(presets.filter((candidate) => candidate.id !== preset.id));
    if (removed) setRoute(null);
  }

  if (route) {
    return (
      // tabIndex -1 so the level itself can take focus when it lands, instead
      // of dropping a keyboard user on document.body. It draws no ring.
      <VStack
        gap={5}
        tabIndex={-1}
        className="settingsRouteLevel"
        data-maka-contract="subagent-detail"
      >
        <SettingsRouteHeader
          onBack={() => setRoute(null)}
          backLabel={copy.editor.backToList}
          contract="subagent-detail-back"
          title={editorPreset ? editorPreset.name : copy.editor.createTitle}
          subtitle={editorPreset ? copy.editor.editSubtitle : copy.editor.createSubtitle}
        />
        <SubagentPresetEditor
          key={editorPreset?.id ?? 'new'}
          preset={editorPreset}
          presets={presets}
          connections={props.connections}
          isSaving={saving}
          onCancel={() => setRoute(null)}
          onDelete={editorPreset ? () => void removePreset(editorPreset) : undefined}
          onSave={async (next) => {
            const nextPresets = editorPreset
              ? presets.map((candidate) => candidate.id === editorPreset.id ? next : candidate)
              : [...presets, next];
            if (await persist(nextPresets)) setRoute(null);
          }}
        />
      </VStack>
    );
  }

  return (
    <SettingsPage className="settingsSubagentsPage">
      {/* The page's single lead group, so it carries no title of its own: the
          settings surface already heads it 子 Agent over the same sentence,
          and a section title here was that heading a second time. Only the
          preset ceiling has anything left to say, and only once it is hit. */}
      <SettingsSection
        description={atLimit ? copy.section.limitNote : undefined}
        action={(
          <Button
            variant="primary"
            size="sm"
            label={copy.section.add}
            isDisabled={saving || atLimit}
            onClick={() => setRoute({ presetId: null })}
          />
        )}
      >
        {presets.length === 0 ? (
          <EmptyState
            title={copy.section.emptyTitle}
            description={copy.section.emptyDescription}
            actions={(
              <Button
                variant="primary"
                label={copy.section.add}
                isDisabled={saving}
                onClick={() => setRoute({ presetId: null })}
              />
            )}
          />
        ) : presets.map((preset) => {
          const availability = subagentPresetAvailability(preset, props.connections);
          // A row states what the preset is for; its route, its id, and its
          // capability boundary are the editor's answer, one level in. Only a
          // preset that cannot currently be selected carries a badge — a list
          // where every row says 可用 says nothing.
          const problem = availability.kind === 'available' ? null : {
            disabled: copy.status.disabled,
            missing_connection: copy.status.missingConnection,
            connection_disabled: copy.status.connectionDisabled,
            model_disabled: copy.status.modelDisabled,
          }[availability.kind];
          return (
            <SettingsRow
              key={preset.id}
              align="start"
              label={(
                <HStack gap={2} vAlign="center" wrap="wrap">
                  <span>{preset.name}</span>
                  {problem ? (
                    <Badge variant={statusBadgeVariant(availability.tone)} label={problem} />
                  ) : null}
                </HStack>
              )}
              description={preset.description || copy.row.fallbackDescription}
              end={(
                <>
                  <Switch
                    label={`${copy.row.enabled}: ${preset.name}`}
                    isLabelHidden
                    value={preset.enabled}
                    isDisabled={saving}
                    onChange={(enabled) => {
                      void persist(
                        presets.map((candidate) =>
                          candidate.id === preset.id ? { ...candidate, enabled } : candidate,
                        ),
                      );
                    }}
                  />
                  <IconButton
                    variant="ghost"
                    size="sm"
                    label={copy.row.configure(preset.name)}
                    tooltip={copy.row.configure(preset.name)}
                    icon={<ChevronRight size={16} aria-hidden="true" />}
                    isDisabled={saving}
                    onClick={() => setRoute({ presetId: preset.id })}
                  />
                </>
              )}
            />
          );
        })}
      </SettingsSection>
    </SettingsPage>
  );
}

function SubagentPresetEditor(props: {
  preset: SubagentPreset | null;
  presets: readonly SubagentPreset[];
  connections: readonly LlmConnection[];
  isSaving: boolean;
  onCancel(): void;
  onDelete?(): void;
  onSave(preset: SubagentPreset): Promise<void>;
}) {
  const locale = useUiLocale();
  const copy = getSubagentSettingsCopy(locale);
  const usableConnections = useMemo(
    () => props.connections.filter((connection) => connection.enabled),
    [props.connections],
  );
  const existingIds = useMemo(
    () => new Set(props.presets.filter((preset) => preset.id !== props.preset?.id).map((preset) => preset.id)),
    [props.preset?.id, props.presets],
  );
  const initialConnection = props.preset
    ? props.connections.find((connection) => connection.slug === props.preset?.connectionSlug)
    : usableConnections[0];
  const initialModels = initialConnection && initialConnection.enabled
    ? connectionEnabledModelIds(initialConnection)
    : [];
  const [draft, setDraft] = useState<EditorDraft>(() => ({
    // Empty, not a pre-derived `subagent`: an id the user has not been asked
    // for yet reads as a value the page already decided. Typing the name fills
    // it (until the user takes it over), and the placeholder says what it is.
    id: props.preset?.id ?? '',
    name: props.preset?.name ?? '',
    description: props.preset?.description ?? '',
    profile: props.preset?.profile ?? 'local_read',
    connectionSlug: props.preset?.connectionSlug ?? usableConnections[0]?.slug ?? '',
    model: props.preset?.model ?? initialModels[0] ?? '',
    thinkingLevel: props.preset?.thinkingLevel ?? '',
  }));
  const [idWasEdited, setIdWasEdited] = useState(props.preset !== null);
  const [submitted, setSubmitted] = useState(false);
  const selectedConnection = props.connections.find(
    (connection) => connection.slug === draft.connectionSlug,
  );
  const enabledModels = selectedConnection?.enabled
    ? connectionEnabledModelIds(selectedConnection)
    : [];
  const thinkingLevels = selectedConnection
    ? thinkingVariantsForModel(selectedConnection.providerType, draft.model)
    : [];
  const profileCopy = copy.profiles[draft.profile];
  const validId = isSafeSubagentPresetId(draft.id.trim());
  const duplicateId = existingIds.has(draft.id.trim());
  const validRoute = Boolean(
    selectedConnection?.enabled && enabledModels.includes(draft.model),
  );
  const canSave = Boolean(
    draft.name.trim() &&
    draft.description.trim() &&
    validId &&
    !duplicateId &&
    validRoute,
  );
  const connectionOptions = props.connections.map((connection) => ({
    value: connection.slug,
    label: connection.name,
    disabled: !connection.enabled,
  }));
  if (
    draft.connectionSlug &&
    !props.connections.some((connection) => connection.slug === draft.connectionSlug)
  ) {
    connectionOptions.unshift({
      value: draft.connectionSlug,
      label: `${draft.connectionSlug} · ${copy.status.missingConnection}`,
      disabled: true,
    });
  }
  const modelOptions: SelectorOptionData[] = enabledModels.map((model) => ({
    value: model,
    label: model,
  }));
  if (draft.model && !enabledModels.includes(draft.model)) {
    modelOptions.unshift({
      value: draft.model,
      label: `${draft.model} · ${copy.status.modelDisabled}`,
      disabled: true,
    });
  }

  function updateName(name: string): void {
    setDraft((current) => ({
      ...current,
      name,
      ...(!idWasEdited ? { id: suggestSubagentPresetId(name, existingIds) } : {}),
    }));
  }

  function selectConnection(connectionSlug: string): void {
    const connection = usableConnections.find((candidate) => candidate.slug === connectionSlug);
    const models = connection ? connectionEnabledModelIds(connection) : [];
    setDraft((current) => ({
      ...current,
      connectionSlug,
      model: models[0] ?? '',
      thinkingLevel: '',
    }));
  }

  async function submit(): Promise<void> {
    setSubmitted(true);
    if (!canSave) return;
    await props.onSave({
      id: draft.id.trim(),
      name: draft.name.trim(),
      description: draft.description.trim(),
      profile: draft.profile,
      connectionSlug: draft.connectionSlug,
      model: draft.model,
      ...(draft.thinkingLevel ? { thinkingLevel: draft.thinkingLevel } : {}),
      // A preset is created ready to use, and whether the main agent may still
      // select it is the list row's switch — the one place that answers it. A
      // second "enable now" control in here answered the same question twice.
      enabled: props.preset?.enabled ?? true,
    });
  }

  const idStatus = submitted && !validId
    ? { type: 'error' as const, message: copy.editor.invalidId }
    : submitted && duplicateId
      ? { type: 'error' as const, message: copy.editor.duplicateId }
      : undefined;

  return (
    <SettingsPage>
      {/* Name and guidance are prose the user writes, so they are full-width
          fields, not values crammed into a row's end slot. */}
      <SettingsSection title={copy.editor.groupPurpose} description={copy.editor.groupPurposeHelp}>
        <SettingsField>
          <TextInput
            label={copy.editor.name}
            value={draft.name}
            placeholder={copy.editor.namePlaceholder}
            isDisabled={props.isSaving}
            status={submitted && !draft.name.trim()
              ? { type: 'error', message: copy.editor.requiredName }
              : undefined}
            onChange={updateName}
          />
        </SettingsField>
        <SettingsField>
          <TextArea
            label={copy.editor.description}
            value={draft.description}
            placeholder={copy.editor.descriptionPlaceholder}
            rows={3}
            isDisabled={props.isSaving}
            status={submitted && !draft.description.trim()
              ? { type: 'error', message: copy.editor.requiredDescription }
              : undefined}
            onChange={(description) => setDraft((current) => ({ ...current, description }))}
          />
        </SettingsField>
        {/* An existing id is a settled fact — session history and the main
            agent's routing both reference it — so it reads as a row's value.
            Only a new preset's id is still the user's to type. */}
        {props.preset ? (
          <SettingsRow
            label={copy.editor.id}
            description={copy.editor.idDescription}
            align="start"
            end={<code className="settingsReadOnlyValue">{props.preset.id}</code>}
          />
        ) : (
          <SettingsField>
            <TextInput
              label={copy.editor.id}
              description={copy.editor.idDescription}
              value={draft.id}
              placeholder={copy.editor.idPlaceholder}
              isDisabled={props.isSaving}
              status={idStatus}
              onChange={(id) => {
                setIdWasEdited(true);
                setDraft((current) => ({ ...current, id }));
              }}
            />
          </SettingsField>
        )}
      </SettingsSection>

      <SettingsSection title={copy.editor.groupRoute} description={copy.editor.groupRouteHelp}>
        <SettingsRow
          label={copy.editor.profile}
          description={profileCopy.description}
          align="start"
          end={(
            <Selector
              label={copy.editor.profile}
              isLabelHidden
              value={draft.profile}
              options={(Object.keys(copy.profiles) as SubagentProfile[]).map((profile) => ({
                value: profile,
                label: copy.profiles[profile].label,
              }))}
              width="100%"
              isDisabled={props.isSaving}
              onChange={(profile) => setDraft((current) => ({
                ...current,
                profile: profile as SubagentProfile,
              }))}
            />
          )}
        />
        {draft.profile === 'implementation' ? (
          <SettingsField>
            <Banner status="warning" title={copy.editor.implementationWarning} />
          </SettingsField>
        ) : null}
        <SettingsRow
          label={copy.editor.connection}
          end={(
            <Selector
              label={copy.editor.connection}
              isLabelHidden
              value={draft.connectionSlug}
              options={connectionOptions}
              width="100%"
              isDisabled={props.isSaving || usableConnections.length === 0}
              disabledMessage={usableConnections.length === 0 ? copy.editor.noConnection : undefined}
              status={submitted && !validRoute
                ? { type: 'error', message: copy.editor.invalidRoute }
                : usableConnections.length === 0
                  ? { type: 'warning', message: copy.editor.noConnection }
                  : undefined}
              onChange={selectConnection}
            />
          )}
        />
        <SettingsRow
          label={copy.editor.model}
          end={(
            <Selector
              label={copy.editor.model}
              isLabelHidden
              value={draft.model}
              options={modelOptions}
              width="100%"
              isDisabled={props.isSaving || enabledModels.length === 0}
              disabledMessage={enabledModels.length === 0 ? copy.editor.noModel : undefined}
              onChange={(model) => setDraft((current) => ({ ...current, model, thinkingLevel: '' }))}
            />
          )}
        />
        {thinkingLevels.length > 0 ? (
          <SettingsRow
            label={copy.editor.thinking}
            end={(
              <Selector
                label={copy.editor.thinking}
                isLabelHidden
                value={thinkingLevels.includes(draft.thinkingLevel as ThinkingLevel)
                  ? draft.thinkingLevel
                  : ''}
                options={[
                  { value: '', label: copy.editor.defaultThinking },
                  ...thinkingLevels.map((level) => ({ value: level, label: copy.thinking[level] })),
                ]}
                width="100%"
                isDisabled={props.isSaving}
                onChange={(thinkingLevel) => setDraft((current) => ({
                  ...current,
                  thinkingLevel: thinkingLevel as ThinkingLevel | '',
                }))}
              />
            )}
          />
        ) : null}
      </SettingsSection>

      {/* Save commits the whole preset, not the route group it would sit
          inside, so it stands on its own between the groups and the delete
          section. */}
      <SettingsSection variant="bare">
        <HStack gap={2} wrap="wrap">
          <Button
            variant="primary"
            label={props.isSaving
              ? copy.editor.saving
              : props.preset
                ? copy.editor.save
                : copy.editor.create}
            isDisabled={props.isSaving}
            onClick={() => void submit()}
          />
          <Button
            variant="ghost"
            label={copy.editor.cancel}
            isDisabled={props.isSaving}
            onClick={props.onCancel}
          />
        </HStack>
      </SettingsSection>

      {/* Deletion is last and stands alone, so a mis-aimed cursor has nothing
          quiet to hit beside it — the providers detail answers it the same way. */}
      {props.onDelete ? (
        <SettingsSection title={copy.editor.dangerZone} description={copy.remove.description}>
          <SettingsActions>
            <Button
              variant="destructive"
              label={copy.editor.delete}
              isDisabled={props.isSaving}
              onClick={props.onDelete}
            />
          </SettingsActions>
        </SettingsSection>
      ) : null}
    </SettingsPage>
  );
}
