// The header of a settings sub-level: back affordance, title, and one quiet
// line of context.
//
// A settings page that owns more than one level (a list and the detail behind
// a row) needs exactly one way back. This was private to `ProvidersPanel`,
// where the four-level provider route lives; the subagent page needs the same
// two-level shape, so the header moved here rather than being written twice.
//
// Deliberately a `Toolbar` with everything in `startContent`: the back button,
// the optional logo, and the title block read as one left-aligned cluster, so
// the title sits where the list's rows started and the eye does not travel.
// Modelled on the settings-sidebar template's detail view, which puts this
// same Toolbar inside the content area rather than reaching for a second page
// shell.
import type { ReactNode } from 'react';
import { Heading, HStack, IconButton, Text, Toolbar, VStack } from '@astryxdesign/core';
import { ArrowLeft } from '@maka/ui/icons';

export function SettingsRouteHeader(props: {
  onBack(): void;
  backLabel: string;
  /** `data-maka-contract` on the back button, so an e2e can aim at this level. */
  contract: string;
  logo?: ReactNode;
  title: string;
  badge?: ReactNode;
  subtitle?: string;
}) {
  return (
    <Toolbar
      label={props.title}
      gap={2}
      startContent={(
        <>
          <IconButton
            variant="ghost"
            label={props.backLabel}
            tooltip={props.backLabel}
            icon={<ArrowLeft size={16} aria-hidden="true" />}
            onClick={props.onBack}
            data-maka-contract={props.contract}
          />
          {props.logo}
          <VStack gap={0}>
            <HStack gap={2} vAlign="center">
              <Heading level={3}>{props.title}</Heading>
              {props.badge}
            </HStack>
            {props.subtitle && (
              <Text type="supporting" color="secondary">{props.subtitle}</Text>
            )}
          </VStack>
        </>
      )}
    />
  );
}
