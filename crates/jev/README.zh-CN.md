<!--
  Licensed to the Apache Software Foundation (ASF) under one
  or more contributor license agreements.  See the NOTICE file
  distributed with this work for additional information
  regarding copyright ownership.  The ASF licenses this file
  to you under the Apache License, Version 2.0 (the
  "License"); you may not use this file except in compliance
  with the License.  You may obtain a copy of the License at

      http://www.apache.org/licenses/LICENSE-2.0

  Unless required by applicable law or agreed to in writing,
  software distributed under the License is distributed on an
  "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
  KIND, either express or implied.  See the License for the
  specific language governing permissions and limitations
  under the License.
-->

# Jev

[English](./README.md)

`maka.jev` 通过公共 Service `maka.jev.evaluate` 提供结构化评估。
Rust 使用类型化调用，JS 使用相同契约的 JSON 绑定；调用必须携带已准入作用域。
结果不是权限决定，也不绕过 Host 授权。

插件负责 System One 请求、问题与概率结果校验；Host 负责 HTTP、凭据隔离及资源结算。
支持 Noul、Choice、Score，保留不确定性和供应商返回的用量元数据；这些元数据尚不属于 Host 模型用量账本。

配置包含启用状态、完整 URL、模型和超时。API key 与自定义请求头按精确端点保存在插件凭据空间，
更改 URL 不转发原端点凭据。拒绝冲突标头，不跟随重定向，不自动重试可能已处理的 POST。

隐私模式不调用网络。取消、超时和无法确认的清理保留结果未知语义。
管理界面支持英文、简体中文和繁体中文。

验证：`cargo nextest run -p maka-jev`；Host 集成测试覆盖公共 Service、Remote、凭据及生命周期。
