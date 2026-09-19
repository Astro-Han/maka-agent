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
export const styles = `
[data-maka-skills-plugin] { color:var(--foreground,inherit); font:inherit; font-size:13px; line-height:1.5; min-width:0; }
[data-maka-skills-plugin][data-mode="composer"] { margin:4px 0; max-height:45vh; overflow:auto; }
[data-maka-skills-plugin] header, [data-maka-skills-plugin] nav, [data-maka-skills-plugin] .skills-actions { display:flex; align-items:center; gap:6px; flex-wrap:wrap; }
[data-maka-skills-plugin] header { padding:4px 0; }
[data-maka-skills-plugin] header strong { margin-right:auto; color:var(--muted-foreground,inherit); font-size:12px; }
[data-maka-skills-plugin] section { margin-top:10px; }
[data-maka-skills-plugin] button { padding:5px 10px; border:1px solid var(--border,#8884); border-radius:8px; background:transparent; color:inherit; font:inherit; font-size:12px; line-height:1.5; cursor:pointer; }
[data-maka-skills-plugin] button:hover:not(:disabled), [data-maka-skills-plugin] button[aria-pressed="true"], [data-maka-skills-plugin] button[aria-expanded="true"] { background:color-mix(in srgb,var(--accent,#6aa5ff) 12%,transparent); border-color:color-mix(in srgb,var(--accent,#6aa5ff) 40%,transparent); }
[data-maka-skills-plugin] button:disabled { opacity:.45; cursor:default; }
[data-maka-skills-plugin] :is(button,input):focus-visible { outline:2px solid var(--accent,#6aa5ff); outline-offset:2px; }
[data-maka-skills-plugin] ul { display:grid; gap:8px; padding:0; margin:12px 0; list-style:none; }
[data-maka-skills-plugin] li { min-width:0; padding:12px; border:1px solid var(--border,#8884); border-radius:12px; background:var(--background-elevated,transparent); }
[data-maka-skills-plugin] p { margin:4px 0 8px; color:var(--muted-foreground,inherit); white-space:pre-wrap; overflow-wrap:anywhere; }
[data-maka-skills-plugin] small { display:block; margin:6px 0; color:var(--muted-foreground,inherit); font-size:11px; overflow-wrap:anywhere; }
[data-maka-skills-plugin] .skills-actions { margin-top:10px; }
[data-maka-skills-plugin] .skills-filter { display:flex; gap:8px; margin:12px 0 8px; }
[data-maka-skills-plugin] input { flex:1; min-width:0; padding:7px 10px; border:1px solid var(--border,#8884); border-radius:8px; background:var(--background-elevated,transparent); color:inherit; font:inherit; }
[data-maka-skills-plugin] .skills-picker li > button { width:100%; text-align:left; font-weight:600; border:0; padding:0; font-size:13px; }
[data-maka-skills-plugin] .skills-picker li > p { margin-bottom:0; }
[data-maka-skills-plugin] pre { max-height:240px; overflow:auto; padding:10px; border:1px solid var(--border,#8884); border-radius:8px; white-space:pre-wrap; overflow-wrap:anywhere; font-size:12px; }
[data-maka-skills-plugin] [role="alert"] { padding:10px; border:1px solid var(--border,#8884); border-radius:8px; }
`;
