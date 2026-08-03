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
import { useEffect, useMemo, useRef, useState } from 'react';
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
import { SettingRow } from './settings-rows.js';
import {
  nextSubagentDraftForName,
  subagentPresetAvailability,
  suggestSubagentPresetId,
} from './subagent-preset-presentation.js';
import { statusBadgeVariant } from './settings-status-badge.js';

/**
 * Where the page is. `create` and `edit` are the same form; they are separate
 * cases because the id, the delete section, and the enabled control differ, and
 * because an `edit` route can become unsatisfiable while `create` cannot.
 */
type PageRoute =
  | { kind: 'list' }
  | { kind: 'create' }
  | { kind: 'edit'; presetId: string };

export type SubagentEditorDraft = {
  id: string;
  name: string;
  description: string;
  profile: SubagentProfile;
  connectionSlug: string;
  model: string;
  thinkingLevel: ThinkingLevel | '';
  enabled: boolean;
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
  const [route, setRoute] = useState<PageRoute>({ kind: 'list' });
  const [saving, setSaving] = useState(false);
  const presets = props.settings.subagents.presets;
  const editorPreset = route.kind === 'edit'
    ? presets.find((preset) => preset.id === route.presetId) ?? null
    : null;
  // An edit route whose preset vanished (deleted in another window, or removed
  // by an external settings write) is an unsatisfiable route, not a state to
  // correct: the list is what it renders as. Deriving that beats scheduling a
  // setState from inside render — and beats the alternative this page shipped
  // with, where a missing preset silently became a blank create form that
  // appended a second preset on save.
  const level: PageRoute['kind'] = route.kind === 'edit' && !editorPreset ? 'list' : route.kind;
  const atLimit = presets.length >= MAX_SUBAGENT_PRESETS;
  const addButtonRef = useRef<HTMLButtonElement>(null);
  // Which row the user left the list from, so returning puts focus back where
  // they were rather than at the top of the page.
  const listReturnFocusRef = useRef<string | null>(null);

  // Focus follows the level. Without this a level change leaves the ring on
  // `document.body` — the chevron that had focus just unmounted — and a
  // keyboard user restarts from the top of the document on every move. The
  // Dialog this page replaced got the same behaviour for free; a route level
  // has to say it. Mirrors ProvidersPanel, which owns the same two levels.
  //
  // Navigating, not arriving: the page does not grab focus when the settings
  // surface first renders the list — the user is still in the settings nav
  // they clicked to get here, and taking the ring off it strands them.
  const hasNavigatedRef = useRef(false);
  useEffect(() => {
    if (!hasNavigatedRef.current) {
      hasNavigatedRef.current = true;
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      if (level !== 'list') {
        // The level, not its back button: an IconButton opens its tooltip on
        // focus, so focusing one on arrival would pop a tooltip at every mouse
        // user who merely clicked a row. `preventScroll` because this is a
        // landing — the level just rendered at the top of the content area.
        document
          .querySelector<HTMLElement>('[data-maka-contract="subagent-detail"]')
          ?.focus({ preventScroll: true });
        return;
      }
      // Consumed here and only here. The row the user came from may be gone —
      // that is exactly what deletion does — so the add button is the
      // fallback, not the default.
      const returnToId = listReturnFocusRef.current;
      listReturnFocusRef.current = null;
      const row = returnToId
        ? document.querySelector<HTMLElement>(`[data-subagent-preset="${CSS.escape(returnToId)}"]`)
        : null;
      (row ?? addButtonRef.current)?.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [level]);

  function openEditor(presetId: string): void {
    listReturnFocusRef.current = presetId;
    setRoute({ kind: 'edit', presetId });
  }

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
    if (removed) setRoute({ kind: 'list' });
  }

  if (level !== 'list') {
    return (
      // tabIndex -1 so the level itself can take the focus the effect above
      // hands it, instead of dropping a keyboard user on document.body. It
      // draws no ring.
      <VStack
        gap={5}
        tabIndex={-1}
        className="settingsRouteLevel"
        data-maka-contract="subagent-detail"
      >
        <SettingsRouteHeader
          onBack={() => setRoute({ kind: 'list' })}
          backLabel={copy.editor.backToList}
          contract="subagent-detail-back"
          title={editorPreset ? editorPreset.name : copy.editor.createTitle}
          subtitle={editorPreset ? copy.editor.editSubtitle : copy.editor.createSubtitle}
          // Enabled state is a fact about the preset the user just opened, and
          // the row that carried it is now off screen. The header slot already
          // exists for exactly this (providers marks its default connection
          // here), so the editor states it instead of re-asking for it.
          badge={editorPreset && !editorPreset.enabled
            ? <Badge variant="neutral" label={copy.status.disabled} />
            : null}
        />
        <SubagentPresetEditor
          key={editorPreset?.id ?? 'new'}
          preset={editorPreset}
          presets={presets}
          connections={props.connections}
          isSaving={saving}
          onCancel={() => setRoute({ kind: 'list' })}
          onDelete={editorPreset ? () => void removePreset(editorPreset) : undefined}
          onSave={async (next) => {
            const nextPresets = editorPreset
              ? presets.map((candidate) => candidate.id === editorPreset.id ? next : candidate)
              : [...presets, next];
            if (await persist(nextPresets)) setRoute({ kind: 'list' });
          }}
        />
      </VStack>
    );
  }

  return (
    <SettingsPage>
      <SettingsSection
        title={copy.section.title}
        description={atLimit ? copy.section.limitNote : copy.section.count(presets.length, MAX_SUBAGENT_PRESETS)}
        action={presets.length > 0 ? (
          <Button
            ref={addButtonRef}
            variant="primary"
            size="sm"
            label={copy.section.add}
            isDisabled={saving || atLimit}
            onClick={() => setRoute({ kind: 'create' })}
          />
        ) : undefined}
      >
        {presets.length === 0 ? (
          // The empty state owns the only call to action here; a section
          // action beside it would be the same button twice on one screen.
          <EmptyState
            title={copy.section.emptyTitle}
            description={copy.section.emptyDescription}
            actions={(
              <Button
                ref={addButtonRef}
                variant="primary"
                label={copy.section.add}
                isDisabled={saving}
                onClick={() => setRoute({ kind: 'create' })}
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
                    // The focus anchor for the way back. It rides the control
                    // the page already renders, so the settings kit does not
                    // need a row-level data escape hatch for one caller.
                    data-subagent-preset={preset.id}
                    onClick={() => openEditor(preset.id)}
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
  const [draft, setDraft] = useState<SubagentEditorDraft>(() => ({
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
    enabled: props.preset?.enabled ?? true,
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
    setDraft((current) => nextSubagentDraftForName(current, name, idWasEdited, existingIds));
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
      enabled: draft.enabled,
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
          // `SettingRow mono`, not a hand-rolled <code> in the end slot: the
          // kit renders machine text as its own full-width line under the
          // description, because `.settingsReadOnlyValue` without
          // `data-mono="true"` is body type in a 320px right-anchored box —
          // and a subagent_id runs to 128 characters.
          <SettingRow
            title={copy.editor.id}
            detail={copy.editor.idDescription}
            value={props.preset.id}
            mono
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
        {/* Only on create. An existing preset is switched from its list row —
            one back-press away, and the header states the current value — but
            a preset that does not exist yet has no row, so without this the
            user cannot land one in a disabled state and has to reach for the
            switch in the window where the main agent can already select it. */}
        {props.preset ? null : (
          <SettingsRow
            label={copy.editor.enabled}
            description={copy.editor.enabledDescription}
            align="start"
            end={(
              <Switch
                label={copy.editor.enabled}
                isLabelHidden
                value={draft.enabled}
                isDisabled={props.isSaving}
                onChange={(enabled) => setDraft((current) => ({ ...current, enabled }))}
              />
            )}
          />
        )}
      </SettingsSection>

      {/* Save commits the whole preset, not the route group it would sit
          inside, so it stands on its own between the groups and the delete
          section. A bare `HStack` under the page stack, not a title-less
          `SettingsSection`: with no header that section renders no header and
          no divider, so it was two wrappers around the same 32px rhythm the
          page stack already gives any direct child. */}
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

      {/* Deletion is last and stands alone, so a mis-aimed cursor has nothing
          quiet to hit beside it — the providers detail answers it the same way. */}
      {props.onDelete ? (
        <SettingsSection title={copy.editor.dangerZone} description={copy.editor.dangerZoneHelp}>
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
