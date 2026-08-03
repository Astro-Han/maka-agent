import type { ReactNode } from 'react';
import { SettingsRow } from './settings-section';

export function SettingRow(props: { title: string; detail: string; value: string; mono?: boolean; action?: ReactNode }) {
  // `mono` means the value is machine text — a path, an id, a key. That is a
  // markup fact, and since the role table composes the code family for the
  // code element group, saying it in the markup is also what makes it render
  // monospaced. The layout rule (break-all, start) selects the same element,
  // so there is no second name for it: `code.settingsReadOnlyValue`.
  const Value = props.mono ? 'code' : 'span';
  const value = <Value className="settingsReadOnlyValue">{props.value}</Value>;
  return (
    <SettingsRow
      label={props.title}
      description={props.detail || undefined}
      align="start"
      end={props.action ? <>{value}{props.action}</> : value}
    />
  );
}
