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

# 后台任务健康检查

[English](./README.md)

`maka.background-health` 提供 `BackgroundTaskHealth` 工具。通过公共 Files API 读取
Shell 返回的任务引用，可选探测 HTTP(S) 地址；不持有私有进程注册表，不按 PID 推测存活。

进程状态与端点健康是独立观察。使用 HEAD，仅在 405／501 时回退 GET；不跟随重定向，
不读取响应正文。2xx 为健康，3xx 为未知，4xx／5xx 为不健康；连接失败、超时或拒绝访问为未知。
端点健康不证明该进程拥有监听端口，也不证明浏览器应用已就绪。

网络授权、传输超时与资源结算由 Host 负责。取消会清理子作用域，清理无法确认时返回错误。
隐私模式允许本地任务观察，但不探测端点。日志仅在 `include_logs` 为真时返回，保留 Host 输出限制。

验证：`cargo nextest run -p maka-background-health`；Host 集成测试覆盖真实 Shell 引用和跨 Session 拒绝。
