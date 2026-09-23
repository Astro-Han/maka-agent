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

export const style = `
[data-maka-insights] { padding:24px; max-width:1200px; color:var(--foreground,inherit); font:inherit; }
[data-maka-insights] header, [data-maka-insights] .insights-actions, [data-maka-insights] nav { display:flex; align-items:center; gap:10px; flex-wrap:wrap; margin:16px 0; }
[data-maka-insights] h2 { margin:0; flex:1; font-size:22px; font-weight:600; }
[data-maka-insights] button, [data-maka-insights] input, [data-maka-insights] select { font:inherit; color:inherit; background:var(--background-elevated,transparent); border:1px solid var(--border,#8884); border-radius:8px; padding:7px 10px; }
[data-maka-insights] button { cursor:pointer; }
[data-maka-insights] button[aria-current=page] { background:var(--accent,#8882); font-weight:600; }
[data-maka-insights] :disabled { opacity:.5; cursor:default; }
[data-maka-insights] :focus-visible { outline:2px solid var(--ring,#6aa5ff); outline-offset:2px; }
[data-maka-insights] .insights-cards { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:12px; margin:20px 0; }
[data-maka-insights] .insights-cards>div { padding:16px; border:1px solid var(--border,#8884); border-radius:12px; }
[data-maka-insights] small { display:block; opacity:.7; }
[data-maka-insights] .insights-cards strong { display:block; font-size:24px; margin-top:8px; overflow-wrap:anywhere; }
[data-maka-insights] .insights-facts { display:grid; grid-template-columns:auto 1fr; gap:12px 24px; }
[data-maka-insights] dd { margin:0; }
[data-maka-insights] .insights-note { opacity:.75; line-height:1.6; }
[data-maka-insights] .insights-table { overflow:auto; margin-top:16px; }
[data-maka-insights] table { width:100%; border-collapse:collapse; font-size:13px; }
[data-maka-insights] th, [data-maka-insights] td { padding:12px 8px; text-align:left; border-bottom:1px solid var(--border,#8883); overflow-wrap:anywhere; max-width:320px; }
[data-maka-insights] th { opacity:.7; white-space:nowrap; font-weight:500; }
[data-maka-insights] td button { margin:2px; }
[data-maka-insights] label { display:flex; align-items:center; gap:8px; }
[data-maka-insights] .insights-price-form { display:flex; flex-wrap:wrap; gap:12px; border:1px solid var(--border,#8884); border-radius:10px; margin:20px 0; padding:16px; }
[data-maka-insights] .insights-price-form label { flex-direction:column; align-items:stretch; }
[data-maka-insights] input { min-width:0; }
[data-maka-insights] [role=alert] { color:var(--destructive,#c44); overflow-wrap:anywhere; }
`;
