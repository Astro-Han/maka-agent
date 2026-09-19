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

/*
 * The `#maka-preload` overlay in index.html covers the window until the app
 * reports a usable frame. It lives outside #root, so React never owns it —
 * removal happens here. A wedged snapshot or renderer read must not strand
 * the logo forever, so a failsafe drops it after a bounded wait and the
 * underlying fail-soft UI shows instead.
 */
const LAUNCH_SURFACE_ID = 'maka-preload';
const LAUNCH_SURFACE_EXIT_CLASS = 'maka-preload-exit';
const LAUNCH_SURFACE_FAILSAFE_MS = 8000;
const LAUNCH_SURFACE_EXIT_MS = 400;

export function dismissLaunchSurface(): void {
  const el = document.getElementById(LAUNCH_SURFACE_ID);
  if (!el || el.classList.contains(LAUNCH_SURFACE_EXIT_CLASS)) return;
  el.classList.add(LAUNCH_SURFACE_EXIT_CLASS);
  const remove = () => el.remove();
  el.addEventListener('transitionend', remove, { once: true });
  setTimeout(remove, LAUNCH_SURFACE_EXIT_MS);
}

export function armLaunchSurfaceFailsafe(): void {
  setTimeout(dismissLaunchSurface, LAUNCH_SURFACE_FAILSAFE_MS);
}
