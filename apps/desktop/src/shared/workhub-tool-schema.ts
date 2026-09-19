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

import { z } from "zod";
import { SETTINGS_SECTIONS, UI_LOCALE_PREFERENCES } from "@maka/core/settings";

/** The validated tool contract is also the model's capability map. */
export const workHubActionSchema = z.union([
  z
    .object({ kind: z.literal("navigate"), section: z.enum(SETTINGS_SECTIONS) })
    .strict()
    .describe(
      "Open a Settings section directly. To delete a task, first archive it through its sidebar hover menu, then open archived-tasks and use its delete action.",
    ),
  z
    .object({
      kind: z.literal("set"),
      target: z.literal("language"),
      value: z.enum(UI_LOCALE_PREFERENCES),
    })
    .strict()
    .describe(
      "Change interface language (界面语言 / 顯示語言). Stored on this Desktop. The executor navigates, clicks, verifies the saved value, and supports guarded undo.",
    ),
  z
    .object({
      kind: z.literal("set"),
      target: z.literal("theme"),
      value: z.enum(["auto", "light", "dark"]),
    })
    .strict()
    .describe(
      "Change appearance theme (主题 / 外觀). Stored on this Desktop. The executor navigates, clicks, verifies the saved value, and supports guarded undo.",
    ),
  z
    .object({
      kind: z.literal("set"),
      target: z.literal("displayName"),
      value: z
        .string()
        .trim()
        .max(60)
        .refine((value) => !/[\u0000-\u001f\u007f]/.test(value)),
    })
    .strict()
    .describe(
      "Change the name Maka uses for you (称呼 / 稱呼) on the current Host. Empty string restores the default form of address. The executor opens the editor, types, saves, verifies, and supports guarded undo.",
    ),
  z
    .object({
      kind: z.literal("open"),
      area: z.enum(["newTask", "extensions", "automations", "app"]),
    })
    .strict()
    .describe(
      "Open a new task, Skills/MCP extensions, scheduled tasks, or return from Settings to the app. To start in an existing project, open newTask and choose its workspace in the composer before sending. Find existing tasks with sidebar search or project groups; task actions (rename, pin, archive) are in each row’s hover menu. WorkHub, when enabled, and task workbar controls are available through the current observation.",
    ),
  z
    .object({ kind: z.enum(["click", "hover"]), ref: z.string().max(100) })
    .strict()
    .describe(
      "Click or reveal a visible control. Use controls[].ref from the current observation; numeric references in accessibility text are informational only.",
    ),
  z
    .object({
      kind: z.literal("type"),
      ref: z.string().max(100),
      text: z.string().max(8000),
    })
    .strict()
    .describe(
      "Replace an observed editor’s contents through native input. Use this for task messages, search, and ordinary fields; use set for known preferences.",
    ),
  z
    .object({
      kind: z.literal("key"),
      ref: z.string().max(100),
      key: z.enum([
        "Enter",
        "Space",
        "Escape",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
      ]),
    })
    .strict()
    .describe("Send a key to an observed editor or selection control."),
  z
    .object({
      kind: z.literal("scroll"),
      ref: z.string().max(100),
      deltaY: z.number().int().min(-900).max(900),
    })
    .strict()
    .describe(
      "Scroll at an observed control with native wheel input, then observe the newly visible controls.",
    ),
]);
export type WorkHubAction = z.infer<typeof workHubActionSchema>;

export const workHubControlSchema = z.discriminatedUnion("operation", [
  z
    .object({
      operation: z.literal("observe"),
      waitMs: z.number().int().min(0).max(5000).optional(),
    })
    .strict()
    .describe(
      "Read the current interface without input. After sending a task, waitMs:2000 allows asynchronous creation and replies; observe for up to 30 seconds without resending, then report unresolved failure.",
    ),
  z
    .object({ operation: z.literal("visual") })
    .strict()
    .describe(
      "Read a cropped visible language/theme control when the selected model accepts images.",
    ),
  z
    .object({
      operation: z.literal("act"),
      actions: z.array(workHubActionSchema).min(1).max(8),
    })
    .strict()
    .describe(
      "Execute ordered native UI actions. Batch known preferences with set; batch generic actions only when every reference is already observed. Opening a page or menu may require a fresh observation before the next action.",
    ),
]);
