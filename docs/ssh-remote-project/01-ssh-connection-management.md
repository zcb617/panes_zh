# 第一阶段：SSH 连接管理设计

## 一、阶段目标

在全局设置中增加“连接”一级菜单，让用户能够查看和管理 Panes 使用的远端电脑。本阶段交付完整的 SSH 连接业务：扫描导入、手动添加、连接检测、编辑、启用、禁用、软删除和恢复。

本阶段结束后，Panes 已能安全识别和连接一台远端 Linux 电脑，但还不创建远端项目；远端项目在第二阶段实现。

## 二、业务范围

本阶段包含：

- 在设置左侧导航增加“连接”。
- 展示正常连接及其启用状态、检测状态和最近错误。
- 扫描本机 `~/.ssh/config` 并批量导入明确配置的主机。
- 手动填写并保存 SSH 连接。
- 检测 SSH、远端系统和当前 Panes 支持的 CLI/智能体。
- 编辑连接的显示名称和连接参数。
- 启用、禁用、删除及恢复连接。
- 为后续阶段提供统一的 SSH 命令、数据传输、PTY 和隧道网关。

本阶段不包含：

- 创建远端项目。
- 浏览项目文件、执行 Git、打开远端终端。
- 发送远端对话或运行远端定时任务。
- 永久删除连接及关联数据。
- 密码登录或键盘交互式登录。

## 三、概要设计

### 3.1 页面位置

在现有全局设置页的左侧导航中增加“连接”一级菜单。它与现有“远程访问”是两个独立业务：

- “连接”管理 Panes 主动连接的 SSH 远端电脑。
- “远程访问”继续管理其他设备访问当前 Panes 的能力，名称和行为不变。

计划改造入口：

- `src/stores/uiStore.ts`：增加 `connections` 设置分区。
- `src/components/settings/SettingsPage.tsx`：增加导航项和内容挂载点。
- 新增连接页、添加弹窗和连接编辑弹窗，组件的 DOM、样式和行为在各自组件内维护。
- `src/i18n/resources/*/app.json`：增加对应文案。

### 3.2 页面结构

“连接”页从上到下分为：

1. 页面标题“SSH 连接”和说明文字。
2. “来自此电脑的 SSH 连接”区域，右上角是“添加”按钮。
3. 正常连接列表。
4. “已删除连接”折叠区域，仅在存在软删除记录时展示。

每条正常连接显示：

- 启用开关。
- 连接图标和显示名称。
- SSH config 别名或 `user@host:port`。
- 状态：未检测、检测中、可用、不可达、已禁用。
- 最近一次成功时间或最近错误摘要。
- “检测”“编辑”“删除”操作。

“已连接”只表示最近一次检测成功，不表示 Panes 永久维持一条 SSH 会话。

### 3.3 总体结构

后端新增统一 SSH 网关，所有后续远端功能必须复用它：

```text
设置界面
   │
   ▼
SSH 连接命令
   │
   ├── 连接数据仓库
   ├── SSH config 解析器
   └── SshRuntimeGateway
          ├── exec
          ├── upload / download
          ├── pty
          └── tunnel
```

网关只负责 SSH 通道、认证、进程监管、错误归类和清理，不包含文件、Git、对话等业务判断。

## 四、详细交互设计

### 4.1 首次打开连接页

1. 前端查询未删除连接。
2. 列表立即展示数据库中的上次状态，不因逐台检测阻塞页面。
3. 对启用连接触发受控的后台检测；同一连接同一时刻只能存在一个检测任务。
4. 检测结果逐条更新，不影响其他连接操作。
5. 禁用连接不自动检测，用户可以在操作菜单中手动检测参数，但不能创建业务运行时。

### 4.2 扫描并导入 SSH config

用户点击页面右上角“添加”，打开“添加 SSH 连接”弹窗。弹窗默认显示扫描结果：

1. 后端从当前用户的 `~/.ssh/config` 开始读取，并处理 `Include`。
2. 只展示明确的 `Host` 别名。
3. 排除 `Host *`、含通配符的 Host、否定规则和无法解析的条目。
4. 对每个别名调用 `ssh -G <alias>`，读取 OpenSSH 最终解析后的 `HostName`、`User`、`Port` 和 `IdentityFile`。
5. 列表展示别名、最终主机、用户名和端口，支持多选。
6. 已经导入且未删除的连接标记“已添加”，不能重复勾选。
7. 已软删除的同别名连接标记“可恢复”，用户选择后进入恢复流程，不创建新 ID。
8. 点击“添加”后批量保存；单条失败不回滚已经成功的其他条目，弹窗逐条显示成功或失败结果。

弹窗底部提供“重新扫描”和“手动添加”。扫描失败时保留弹窗，并明确显示：SSH config 不存在、Include 文件不可读、OpenSSH 不存在或具体条目解析失败。

导入连接的实际执行入口始终是 config alias。`ProxyJump`、代理、IdentityFile、ssh-agent 和系统 Host Key 行为继续交给 OpenSSH 处理，Panes 不把复杂 config 展开成另一套连接协议。

### 4.3 手动添加

点击“手动添加”切换为表单，字段如下：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| 显示名称 | 是 | 在 Panes 中展示，可后续修改 |
| Host/IP | 是 | 远端主机名或 IP |
| 用户名 | 是 | SSH 登录用户 |
| 端口 | 是 | 默认 22，范围 1～65535 |
| IdentityFile | 是 | 本机私钥文件路径 |
| Host Key | 是 | 完整 OpenSSH 公钥行，不接受只有指纹的输入 |

Host Key 允许以下两种输入，保存时统一提取密钥类型和 Base64 公钥内容：

```text
ssh-ed25519 AAAAC3...
server.example.com ssh-ed25519 AAAAC3...
```

只有 `SHA256:...` 指纹不足以生成严格校验记录，表单直接提示用户填写完整公钥。

点击“保存并检测”时依次执行：

1. 前端完成必填项和端口格式校验。
2. 后端验证 IdentityFile 存在且是普通文件。
3. 后端验证 Host Key 类型与 Base64 内容。
4. 生成不可变连接 UUID。
5. 在应用数据目录写入该连接独立的 `known_hosts` 文件。
6. 保存连接记录。
7. 立即执行非交互检测。
8. 检测成功后关闭弹窗；检测失败仍保存记录并留在编辑页展示错误，用户可以修改后重试。

### 4.4 连接检测

检测分两层：

第一层确认 SSH 通道：

- 本机存在系统 `ssh`。
- DNS、端口和超时正常。
- 公钥认证成功，整个过程不得等待密码输入。
- Host Key 校验成功。

第二层确认远端能力：

- 操作系统为 Linux。
- 获取远端 `HOME`、默认 shell。
- 检测 Git。
- 检测当前 Panes 支持的全部 CLI/智能体路径和版本。

检测结果写入连接记录的最近成功时间和最近错误；详细 CLI 能力缓存按连接 ID 保存，供后续项目和智能体阶段使用。

### 4.5 启用与禁用

关闭启用开关时：

1. 弹出确认提示，说明该连接的远端操作将暂时不可用，数据不会删除。
2. 保存 `enabled = false`。
3. 关闭该连接当前存在的命令、PTY、隧道和 CLI 运行时。
4. 后续请求统一返回“SSH 连接已禁用”。

重新启用时保存 `enabled = true`，不复用禁用前的旧进程；首次业务操作按需建立新连接，页面同时触发一次检测。

### 4.6 删除与恢复

删除确认框必须明确说明：

- 连接及其关联项目、对话、消息保留；定时任务相关联动当前已暂缓，不作为当前执行项。
- 关联项目会从项目列表隐藏。
- 定时任务暂停与恢复属于总计划第六阶段内容，该阶段当前已暂缓。
- 不删除远端电脑上的任何文件。
- 可以从“已删除连接”恢复。

确认删除后只写入 `deleted_at`，不删除数据库记录，不生成新 ID，并立即关闭该连接的全部运行时资源。

“已删除连接”区域显示原名称、主机、删除时间和“恢复”按钮。恢复行为：

1. 使用原 `ssh_connections.id`。
2. 清空 `deleted_at`。
3. 保留删除前的 `enabled` 状态；原来禁用的连接恢复后仍禁用。
4. config 导入连接使用当前 `ssh -G` 结果刷新连接字段；有差异时先展示差异，再由用户确认恢复。
5. 手动连接继续使用原参数，用户可在恢复后编辑。
6. 触发一次连接检测，但检测失败不撤销恢复。

首版不提供“永久删除”。手动重新填写相同 Host、端口和用户名不能被猜测为原连接；只有从已删除记录执行恢复，才能保证原 ID 不变。

## 五、数据设计

新增 `ssh_connections` 表：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | TEXT | 不可变 UUID，主键 |
| `display_name` | TEXT | 显示名称 |
| `source_kind` | TEXT | `ssh_config` 或 `manual` |
| `config_alias` | TEXT | config 导入时的 Host 别名 |
| `host_name` | TEXT | 最终主机名或 IP |
| `user` | TEXT | 登录用户 |
| `port` | INTEGER | SSH 端口 |
| `identity_file` | TEXT | 手动连接必填，导入连接保存解析结果用于展示 |
| `host_key_type` | TEXT | 手动连接的 Host Key 类型 |
| `host_key_base64` | TEXT | 手动连接的 Host Key 内容 |
| `enabled` | INTEGER | 用户启用状态 |
| `last_connected_at` | TEXT | 最近检测成功时间 |
| `last_error` | TEXT | 最近检测错误摘要 |
| `deleted_at` | TEXT | 软删除时间 |
| `created_at` | TEXT | 创建时间 |
| `updated_at` | TEXT | 更新时间 |

约束：

- 未删除的 config 导入记录按 `config_alias` 唯一。
- 连接 ID 永不因编辑、禁用、删除、恢复或 config 参数变化而改变。
- 手动连接的独立 `known_hosts` 文件路径使用连接 ID，不使用显示名称。
- 数据库不保存私钥内容、密码、一次性运行时密钥或完整敏感命令行。

## 六、后端接口设计

计划增加以下命令，命名可按现有 Tauri 命令规范调整，但职责不得混合：

| 命令 | 用途 |
| --- | --- |
| `list_ssh_connections` | 查询未删除连接 |
| `list_deleted_ssh_connections` | 查询已删除连接 |
| `scan_ssh_config_hosts` | 扫描并解析 SSH config |
| `import_ssh_config_hosts` | 批量导入选中别名 |
| `create_manual_ssh_connection` | 创建手动连接 |
| `update_ssh_connection` | 编辑连接参数或显示名称 |
| `test_ssh_connection` | 检测 SSH 和远端能力 |
| `set_ssh_connection_enabled` | 启用或禁用 |
| `delete_ssh_connection` | 软删除 |
| `restore_ssh_connection` | 按原 ID 恢复 |

后端模块计划：

- `src-tauri/src/db/ssh_connections.rs`：连接持久化。
- `src-tauri/src/commands/ssh_connections.rs`：界面命令入口。
- `src-tauri/src/ssh/config.rs`：config 扫描与 `ssh -G` 解析。
- `src-tauri/src/ssh/gateway.rs`：统一 SSH 进程、数据、PTY 和隧道能力。
- `src-tauri/src/ssh/known_hosts.rs`：手动连接 Host Key 文件管理。
- `src-tauri/src/ssh/errors.rs`：稳定错误码与用户可读信息。

所有连接命令都只接收连接 ID，再从数据库加载实际参数；业务调用不能信任前端传来的 Host 或 IdentityFile 覆盖数据库值。

## 七、SSH 安全与生命周期

手动连接固定使用非交互、严格 Host Key 校验和指定身份文件。config 导入连接沿用 OpenSSH 配置，同时强制非交互，任何密码或 Host Key 确认提示都视为失败。

需要区分的错误至少包括：

- 本机未安装 OpenSSH。
- config 或 Include 文件不可读。
- IdentityFile 不存在或无权限。
- 公钥认证失败。
- 远端要求密码或键盘交互。
- Host Key 缺失、变化或不匹配。
- DNS、拒绝连接和超时。
- 远端不是 Linux。
- 远端命令执行失败。

所有 SSH 子进程都必须由网关登记并清理。连接禁用、删除、应用退出或隧道异常时，关闭对应进程、端口转发、事件订阅和待处理请求，不能保留不可追踪的后台进程。

## 八、前端状态设计

新增连接状态仓库，状态按连接 ID 管理：

```text
connections
deletedConnections
scanResults
testStateByConnectionId
saveState
errorByConnectionId
```

扫描、检测和保存使用独立加载状态，避免一个连接检测时锁住整页。异步结果返回时必须校验连接 ID 和请求序号，防止旧检测覆盖新编辑后的状态。

## 九、实施改造清单

前端主要涉及：

- `src/stores/uiStore.ts`
- `src/components/settings/SettingsPage.tsx`
- 新增 SSH 连接页面和弹窗组件
- 新增 SSH 连接状态仓库
- `src/lib/ipc.ts`
- `src/types.ts`
- `src/i18n/resources/*/app.json`
- 对应组件内样式

后端主要涉及：

- `src-tauri/src/db/mod.rs`
- 新增数据库迁移及连接仓库
- 新增 SSH 连接命令模块
- 新增统一 SSH 网关模块
- `src-tauri/src/lib.rs` 中的命令注册和应用退出清理

## 十、测试设计

### 10.1 单元测试

- config 普通 Host、Include、通配符、否定规则和缺失文件解析。
- `ssh -G` 输出解析。
- Host Key 两种输入格式、非法 Base64 和仅指纹输入。
- 连接状态迁移：启用、禁用、删除、恢复。
- 恢复后 ID 不变。
- 错误输出到稳定错误类型的映射。

### 10.2 集成测试

- 使用 config alias 成功连接。
- 使用手动 IdentityFile 和 Host Key 成功连接。
- 错误私钥、错误 Host Key、要求密码、超时均能快速失败。
- 删除时关闭现有 SSH 资源，恢复后重新创建。
- 两台远端电脑的状态和错误互不覆盖。

### 10.3 界面测试

- 设置导航能进入“连接”。
- 添加弹窗可扫描、多选、重新扫描和手动添加。
- 已添加和可恢复条目不可被误创建为新连接。
- 列表开关、检测、编辑、删除和恢复反馈正确。
- 现有“远程访问”页面没有回归。

## 十一、阶段验收标准

1. 设置页出现独立“连接”一级菜单和“添加”按钮。
2. 能正确扫描 `~/.ssh/config`，批量导入可用主机。
3. 能通过手动 IdentityFile 和完整 Host Key 添加连接。
4. 能检测 SSH、Linux、HOME、shell、Git 和全部当前支持的 CLI/智能体。
5. 能启用、禁用、编辑和软删除连接。
6. 删除后可从已删除列表恢复，恢复前后连接 ID 完全相同。
7. Host Key、认证、网络等错误均有明确提示，不能弹出交互式密码窗口。
8. 禁用或删除连接会关闭其全部运行时资源。
9. 未修改现有本地项目及“远程访问”业务行为。

## 十二、阶段输出与下一阶段入口

本阶段输出稳定的 SSH 连接 ID、连接管理界面、远端能力检测结果和统一 SSH 网关。第二阶段只通过连接 ID 选择远端电脑和查询目录，不重新解析或复制 SSH 参数。
