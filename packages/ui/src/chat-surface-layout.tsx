import type { ComponentProps } from 'react';
import { ChatLayout } from '@astryxdesign/core/Chat';
import { cn } from './utils.js';

export type ChatSurfaceLayoutProps = ComponentProps<typeof ChatLayout>;

/**
 * Maka's product seam for the Astryx chat page shell.
 *
 * Astryx owns scrolling, new-message following, the bottom dock, and the
 * scroll-to-bottom affordance. Maka supplies only transcript and composer
 * content through the published ChatLayout slots.
 *
 * `balanced`, not `compact`: density here sets the dock's own padding, and at
 * compact that is spacing-2 — 8px between the composer card's rounded bottom
 * edge and the window edge, at every window height. The card read as pushed
 * against the frame rather than resting above it. Balanced spends spacing-3 on
 * the same gutters and lengthens the fade over the transcript to match (blur
 * layer 80px → 100px, mask ramp 24px → 36px); the message area is byte-identical
 * between the two tiers, so this moves the dock and nothing else. Astryx's own
 * ai-chat template runs spacious, one step further still.
 */
export function ChatSurfaceLayout({ className, density = 'balanced', ...props }: ChatSurfaceLayoutProps) {
  return (
    <ChatLayout
      {...props}
      density={density}
      className={cn('maka-chat-layout', className)}
      data-chat-scroll-container="true"
    />
  );
}
