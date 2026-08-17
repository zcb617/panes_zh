# SSH 远端项目技术方案 v2（修订版，待审核）

核心原则：不另造远端业务协议，不部署常驻 Panes 服务端。所有远端能力统一经过系统 `ssh` 提供的安全通道；需要监听端口的 CLI 服务只绑定远端 `127.0.0.1`，再通过 SSH 本地端口转发供 Panes 现有客户端连接。文件、Git、附件和终端分别使用同一 SSH 网关提供的命令、数据和 PTY 通道。任何远端错误都不能降级到本机执行。

## 一、功能边界

首版支持：

- 远端系统：Linux。
- 从 `~/.ssh/config` 扫描并导入 SSH 主机。
- 手动添加 SSH 主机，配置只保存到 Panes，不修改 SSH config。
- SSH config 连接依赖已有的 `IdentityFile` 或 ssh-agent。
- 手动连接认证只支持 `IdentityFile`，Host Key 独立维护。
- 禁止密码输入和密码保存；远端要求密码时直接提示错误。
- 远端项目完整支持当前 Panes 已支持的全部 CLI/智能体及其关联能力，包括文件、Git、工作树、终端、CLI 对话、模型、思考强度、权限、启动终端、定时任务和附件；首版不按 Codex、Claude、OpenCode 等名称缩减范围。
- 远端项目发生错误时，绝不降级成本地执行。

现有“手机远程访问”功能保持不变，与 SSH 连接是两个独立模块。

## 二、总体架构

后端增加统一执行目标：

```text
ExecutionTarget
├── Local
└── Ssh(connection_id)
```

每个项目绑定一个执行目标：

```text
本地项目：root_path = 本地绝对路径
远端项目：ssh_connection_id + root_path = Linux 绝对路径
```

所有操作都先通过 `workspace_id` 查询项目目标，再决定本地执行还是 SSH 执行。前端不能为聊天、文件或 Git 请求临时指定连接，防止把远端项目误发到本机。

后端增加统一目标解析器和 SSH 网关：

```text
workspace_id / thread_id
        │
        ▼
ExecutionTargetResolver
        ├── Local
        └── Ssh(connection_id)
                 │
                 ▼
          SshRuntimeGateway
          ├── exec：文件、Git、检测、目录浏览
          ├── upload/download：附件和必要的运行时文件
          ├── pty：终端和终端型智能体
          └── tunnel：远端回环端口与本地端口转发
```

不得按 Codex、Claude、OpenCode 等名称分别实现 SSH 连接器。CLI 适配层只负责原有业务协议和启动描述，SSH 认证、进程监管、端口分配、隧道存活检测、错误归类和清理全部由 `SshRuntimeGateway` 负责。

后端命令的执行目标必须从持久化关系中解析：

- 文件、Git、终端和工作树通过 `workspace_id` 解析。
- 聊天、模型和权限通过 `thread_id -> workspace_id` 解析。
- 定时任务通过 `scheduled_task.workspace_id` 解析。
- 手机远程访问触发的消息继续进入同一消息发送入口，并由线程所属项目解析目标。

前端提交的路径、模型或连接 ID 不能覆盖后端解析出的目标。目标是已删除、已禁用或不可达的 SSH 连接时，操作必须失败并返回明确错误。

## 三、数据结构

新增 `ssh_connections` 表，保存：

- `id`：不可变 UUID，是连接及其全部关联数据的稳定身份。
- `display_name`。
- `source_kind`：`ssh_config` 或 `manual`。
- `config_alias`、`host_name`、`user`、`port`、`identity_file`。
- `host_key_type`、`host_key_base64`：手动连接独立维护的远端 Host Key。
- `enabled`、`last_connected_at`、`last_error`。
- `deleted_at`：连接软删除时间；为空表示正常连接。
- `created_at`、`updated_at`。

`workspaces` 增加：

- `location_kind`：`local` 或 `ssh`。
- `ssh_connection_id`：本地项目为空，远端项目必填。

现有项目全部迁移成 `local`。原来的 `root_path UNIQUE` 调整为按项目位置分别建立唯一性约束：

```text
本地项目：root_path UNIQUE
远端项目：ssh_connection_id + root_path UNIQUE
```

其中远端 `root_path` 是远端 Linux 的实际绝对目录路径，不是项目显示名称。这样不同远端机器可以同时添加相同路径，同一远端也不会重复添加同一个实际目录。SQLite 迁移必须移除原有 `root_path UNIQUE` 的单列约束，并通过本地、远端两个部分唯一索引或等价的表结构完成约束，不能只增加字段。

建议使用部分唯一索引表达约束：

```sql
CREATE UNIQUE INDEX idx_workspaces_local_root
ON workspaces(root_path)
WHERE location_kind = 'local';

CREATE UNIQUE INDEX idx_workspaces_ssh_root
ON workspaces(ssh_connection_id, root_path)
WHERE location_kind = 'ssh';
```

远端 Linux 路径按大小写敏感规则比较，不能使用 Windows 本地路径的小写归一化逻辑。远端路径在创建项目时通过远端 `realpath` 解析为实际绝对路径，再用于保存和唯一性判断。

现有 `workspaces.root_path` 的单列唯一约束定义在表结构中，不能通过删除索引解除。迁移必须在启动阶段重建 `workspaces` 表并恢复索引、外键和现有数据；迁移完成后执行 `PRAGMA foreign_key_check`。任何一步失败都必须回滚，不能留下部分迁移后的数据库。

项目的 `name` 是显示 label：首次创建时由实际目录名自动生成，用户可以修改；修改只更新 label 映射，不修改远端实际目录。

`ssh_connection_id` 使用外键关联 `ssh_connections.id`，物理外键删除策略使用 `RESTRICT`。首版不物理删除 SSH 连接，也不级联删除项目数据。

`scheduled_tasks` 增加：

- `suspended_at`：因连接删除或禁用而暂停的时间。
- `suspension_reason`：例如 `ssh_connection_deleted`、`ssh_connection_disabled`。
- `suspended_by_connection_id`：触发暂停的 SSH 连接 ID。

`enabled` 保留用户原来的启用状态，连接删除或禁用不能覆盖这个状态。调度器只选择 `enabled = 1 AND suspended_at IS NULL` 的任务。

### 连接删除与恢复

删除 SSH 连接采用软删除，事务内执行：

1. 在 `ssh_connections.deleted_at` 写入删除时间。
2. 给该连接关联项目的定时任务写入暂停原因，但不改变 `enabled`。
3. 提交事务后，关闭该连接的 CLI 运行时、隧道和终端会话。
4. 项目列表查询排除连接已删除的远端项目；项目、仓库、对话、消息、操作记录、审批、定时任务和任务执行历史均保留。

连接删除不能复用项目自身的 `archived_at`。项目在删除连接前是否已归档，必须在恢复连接后保持原状。

恢复 SSH 连接采用原记录恢复，事务内执行：

1. 根据原 `ssh_connections.id` 找到软删除记录，禁止新建连接记录。
2. 清除 `deleted_at`；必要时更新扫描得到的非身份字段，但 `id` 不变。
3. 仅清除由该连接造成的任务暂停状态。
4. 已启用任务从恢复时间重新计算 `next_run_at`，不补跑连接删除期间错过的任务。

如果连接在删除前已经处于禁用状态，恢复后仍保持禁用；对应定时任务继续以 `ssh_connection_disabled` 原因暂停。只有恢复后的连接处于启用状态，才能清除连接级暂停。

恢复前后，下列 ID 必须完全一致：

```text
ssh_connection_id
workspace_id
repo_id
thread_id
message_id
scheduled_task_id
engine_thread_id
```

SSH config 扫描以原 `config_alias` 识别已删除记录，并显示“恢复”而不是“添加”。手动连接从“已删除连接”列表恢复；不能根据用户重新填写的 Host、端口和用户名猜测为同一连接。首版不提供“永久删除关联数据”。

## 四、SSH 连接管理

### 扫描 SSH config

点击“添加”后：

1. 读取 `~/.ssh/config` 以及其中的 `Include`。
2. 只展示明确的 `Host` 别名。
3. 排除 `Host *`、通配符和否定规则。
4. 使用 `ssh -G <alias>` 获取最终的 HostName、User、Port、IdentityFile。
5. 已经导入的连接标记为“已添加”。
6. 已软删除的同 alias 连接标记为“可恢复”。
7. 支持一次勾选多个连接。

导入后的连接以 SSH alias 为执行入口，`ProxyJump`、代理、端口、IdentityFile 等配置仍由 OpenSSH 处理。

已删除 alias 再次出现时，界面展示当前 `ssh -G` 解析结果与删除前保存信息的差异。用户执行恢复后仍使用原连接 ID，并用当前 SSH config 刷新解析字段；不能在扫描阶段静默恢复。

### 手动添加

字段包括显示名称、Host/IP、用户名、端口、IdentityFile 和 Host Key。端口默认 22，IdentityFile 必填。Host Key 属于手动连接的独立管理内容，配置保存到 Panes 数据库，不写入 `~/.ssh/config` 或系统 `known_hosts`。

Host Key 输入支持以下两种 OpenSSH 公钥文本：

```text
ssh-ed25519 AAAAC3...
主机名 ssh-ed25519 AAAAC3...
```

保存前统一提取并校验密钥类型和 Base64 公钥内容。只有 `SHA256:...` 指纹不能直接构造严格校验所需的 `known_hosts` 记录，因此不作为有效输入；界面需要明确提示填写完整公钥。

Panes 按连接 ID 在应用数据目录生成独立的 `known_hosts` 文件：

```text
<panes-app-data>/ssh/known-hosts/<connection_id>
```

标准端口使用 `host key-type base64`，非 22 端口使用 `[host]:port key-type base64`。该文件由 Panes 管理，不修改系统 `known_hosts`。Host Key 不匹配、缺失或校验失败时直接报错，不自动获取、不自动接受，也不覆盖原 Host Key。

手动连接固定使用以下非交互和身份校验参数：

```text
ssh -o BatchMode=yes
    -o NumberOfPasswordPrompts=0
    -o PasswordAuthentication=no
    -o KbdInteractiveAuthentication=no
    -o StrictHostKeyChecking=yes
    -o IdentitiesOnly=yes
    -o UserKnownHostsFile=<panes-connection-known-hosts>
    -i <identity-file>
    -p <port>
    <user>@<host>
```

通过 `~/.ssh/config` 导入的连接继续以 alias 为入口，并由 OpenSSH 处理 ProxyJump、IdentityFile、ssh-agent 和系统 Host Key 配置；Panes 仍追加非交互参数，禁止弹出密码或 Host Key 确认提示。

### 连接检测

后台统一使用非交互方式：

```text
ssh -o BatchMode=yes
    -o NumberOfPasswordPrompts=0
    -o PasswordAuthentication=no
    -o KbdInteractiveAuthentication=no
    -o StrictHostKeyChecking=yes
    -o ConnectTimeout=...
```

检测 SSH 是否可达、远端是否为 Linux、HOME 目录、shell、Git，以及当前 Panes 已支持的全部 CLI/智能体的路径和版本。

界面中的“已连接”表示最近一次检测成功，不代表维持永久 SSH 连接。

Host Key 不是要删除的功能：手动添加连接时必须保留独立 Host Key 管理；Panes 使用手动配置进行远端身份校验，不自动替换或覆盖。SSH config 导入连接则沿用系统 OpenSSH 的 Host Key 行为。Host Key、认证或网络错误均直接展示。

### 删除、恢复与禁用界面

- 删除连接前明确提示：连接、项目、对话、消息和定时任务会保留；项目暂时隐藏；可以从“已删除连接”恢复；不会删除远端文件。
- “已删除连接”列表提供恢复操作，恢复必须复用原连接 ID。
- 禁用连接不隐藏项目，项目仍显示，但所有执行入口返回“SSH 连接已禁用”。
- 删除与禁用都会停止该连接的运行时和隧道；恢复或启用后按需重新创建，不复用旧进程。

## 五、项目创建界面

点击“添加项目”后先选择本地或远端。

选择本地后继续使用现有本地目录选择流程。选择远端后显示：

- 远端主机下拉框。
- 默认进入远端 HOME。
- 来自当前远端机器的目录列表。
- 面包屑、返回上级和手动输入绝对路径。
- 项目名称，默认取目录名，允许修改。

创建后不能直接切换项目所属主机；需要更换主机时重新添加项目，避免路径和会话归属混乱。

## 六、远端功能执行方式

### 文件

通过 SSH 执行 Linux 文件命令，不增加 SFTP 库或远端守护程序。支持目录和仓库扫描、文件读取和写入、创建、重命名、删除、文本搜索、状态和权限检查。

现有只接收裸 `repo_path` 或裸文件路径的后端接口，必须增加 `workspace_id`，并先解析执行目标。远端路径不能在本机执行 `canonicalize`，也不能使用 Windows 盘符、分隔符或大小写规则处理。

所有命令必须经过：

- POSIX 参数转义。
- 项目根目录范围检查。
- 远端 `realpath` 校验。
- 二进制文件和大小限制检查。
- 同目录临时文件写入后原子替换，避免断线破坏目标文件。

远端项目的“在资源管理器中显示”“用本地默认应用打开”等功能首版禁用；Panes 内部编辑器正常支持。

### Git

本地项目继续使用现有 `git2` 和 Git CLI。远端项目统一执行：

```text
ssh ... git -C <remote-repo-path> ...
```

覆盖现有 Git 功能，包括仓库发现、状态、diff、日志、分支、提交、拉取、推送和工作树。

Git、仓库扫描和工作树接口必须以 `workspace_id` 作为目标依据。前端传入的 `repo_path` 只能作为该项目目标内部的路径参数，不能决定命令在本机还是远端执行。

本地文件监听器不能监听远端目录。远端 Git 状态在 Git 操作完成后立即刷新；Git 面板可见时短周期轮询，页面不可见时停止轮询。

### 终端

远端项目终端继续使用现有本地 PTY，PTY 内运行：

```text
ssh -tt <host>
```

连接后进入项目目录。输入、输出、尺寸调整和中断逻辑继续使用当前协议。

### 附件

远端项目中的本地附件不能直接把本地路径交给远端 CLI。发送前通过 SSH 标准输入上传到远端 Panes 缓存目录，再把远端路径交给 CLI；会话结束或缓存过期后清理。

远端缓存目录按连接和会话隔离，例如：

```text
~/.cache/panes/attachments/<workspace_id>/<thread_id>/
```

上传成功后才能启动消息请求。上传失败时整次发送失败，不允许把本地附件路径继续交给远端 CLI，也不允许改用本机 CLI 发送。

### 定时任务

现有 `execution_device_id` 表示负责触发任务的 Panes 设备，不用于表示 SSH 主机，也不能保存 `ssh_connection_id`。SSH 执行目标始终由 `scheduled_task.workspace_id` 对应的项目解析。

- 本地 Panes 调度器继续负责到点触发。
- 任务进入现有统一消息发送入口后，根据线程或项目解析 `ExecutionTarget`。
- 远端项目的新线程任务和已有线程任务都通过对应远端 CLI 运行时执行。
- 连接临时断开时，本次任务记录明确错误，不降级到本机，也不自动禁用任务。
- 连接删除或禁用时，任务进入可恢复暂停状态，不参与到期查询。
- 连接恢复或启用时，从当前时间重新计算下一次执行时间，不补跑暂停期间的历史触发点。

## 七、CLI 启动方式

首版范围不是固定的三个 CLI，而是当前 Panes 已支持的全部 CLI/智能体。它们都遵循同一条原则：由本地 Panes 通过 SSH 在远端启动对应的 CLI 服务端或 CLI 进程，再由现有客户端协议连接；消息、文件上下文和执行均发生在远端项目环境。

所有需要客户端持续连接的 CLI 服务统一使用以下流程：

1. `SshRuntimeGateway` 在本机分配空闲回环端口。
2. 在远端分配空闲回环端口。
3. 通过 SSH 在远端项目目录启动 CLI 服务或 Panes 协议适配器，监听 `127.0.0.1:<remote_port>`。
4. 建立 `127.0.0.1:<local_port> -> SSH -> 127.0.0.1:<remote_port>` 本地端口转发。
5. Panes 现有客户端使用本机回环地址连接，业务协议保持不变。
6. 隧道进程、远端 CLI 或适配器任意一方退出时，将该运行时标记为断开并清理另一方。

禁止让远端服务监听 `0.0.0.0` 或公网网卡。任何 CLI 的认证信息、会话内容和内部接口都不能绕过 SSH 隧道暴露到网络。

### Codex

远端启动：

```text
codex app-server --listen ws://127.0.0.1:<remote_port>
```

当前 Codex `app-server` 已支持 WebSocket 监听。Panes 的 Codex 客户端继续使用原 app-server 协议，只把传输端点切换为 SSH 转发后的本机 WebSocket 地址。模型、账号、用量、权限、会话恢复和消息事件均通过该连接获取。

远端 Codex 版本不支持 WebSocket 监听时，健康检查返回版本不兼容；首版不切换到另一套 SSH stdio 实现。

### OpenCode

远端启动：

```text
OPENCODE_SERVER_PASSWORD=<runtime-secret> \
opencode serve --hostname 127.0.0.1 --port <remote_port>
```

现有 HTTP/SSE 客户端继续连接 SSH 转发后的本机地址。运行时密码只存在于当前进程环境和本地运行时内存中，不能写入日志或数据库。

### Claude Code

Claude 对话继续使用 Panes 现有协议适配器，不要求用户理解或配置适配器内部实现。

实现方式：

1. Panes 按应用版本计算适配器和依赖包的内容版本。
2. 首次使用时，通过 SSH 上传到远端版本目录，例如 `~/.cache/panes/runtime/claude/<version>/`。
3. 校验远端文件版本和完整性；版本一致时复用，版本不一致时上传新目录，不覆盖正在运行的旧版本。
4. 在远端使用兼容的运行环境启动适配器；适配器在远端调用远端 Claude Code、读取远端账号配置，并只监听远端回环端口。
5. 本地 Claude 客户端通过 SSH 转发后的本机端口继续使用现有 JSON 行协议。
6. 远端缺少 Claude Code 或兼容运行环境时，健康状态显示不可用，不调用本机 Claude Code 代替。

Claude 适配器是按需启动、随运行时关闭的进程，不是常驻 Panes 服务端。

### 终端型智能体

智能体运行工具页面中通过终端启动的 CLI，统一在当前项目的 SSH PTY 中运行。启动命令、工作目录、环境变量和权限均来自当前项目所属远端机器。此类工具没有独立监听接口时不人为增加 HTTP 服务；它们仍通过统一 `SshRuntimeGateway` 的 PTY 通道执行。

每个执行目标维护独立运行时：

```text
EngineRuntimeRegistry
├── local
├── ssh:<连接1>
└── ssh:<连接2>
```

机器能力缓存的键为：

```text
(ExecutionTarget, engine_id)
```

对话运行时的键为：

```text
(ExecutionTarget, engine_id, runtime_scope)
```

`runtime_scope` 由 CLI 启动描述声明：能够同时承载多个工作目录的服务使用 `machine`；启动时绑定工作目录的服务使用 `workspace_id`。这是运行时生命周期差异，不是 SSH 连接逻辑分叉。

不同机器的 CLI 进程、版本、安装状态、模型列表、思考强度、权限、登录账户、用量、MCP、Skills、扩展能力和 CLI 会话 ID 完全隔离。不能再使用只按 `engine_id` 建立的全局单例或全局缓存。远端连接断开后只报错，不允许调用本机 CLI 继续会话。

连接删除、禁用、应用退出或隧道异常时，运行时注册表必须关闭对应远端进程、SSH 转发进程、事件订阅和待处理请求。连接恢复或重新启用后按需创建新运行时。

## 八、模型和输入框

当前项目决定输入框使用哪个运行时：

- 本地项目读取本地 CLI 能力。
- 远端项目读取对应远端机器的 CLI 能力。
- 切换项目时同步切换模型、思考强度、权限、健康状态、登录账号和用量。
- 每台机器分别缓存能力列表，缓存键包含 `ExecutionTarget` 和 `engine_id`。
- 会话中的模型配置使用所属机器的能力进行校验。

模型、健康状态、登录账号、用量、MCP、Skills 和扩展能力均通过项目所属机器上的 CLI 获取，再经 SSH 隧道返回。前端不能使用本机查询结果填充远端项目，也不能在远端查询失败时继续显示为可用状态。

切换项目时按以下顺序处理：

1. 根据项目切换当前 `ExecutionTarget`。
2. 立即切换到该目标已有缓存；没有缓存时显示加载状态，不能暂用上一台机器的数据。
3. 通过该目标的运行时刷新能力和健康状态。
4. 校验当前线程保存的模型、思考强度和权限；远端不支持时提示用户重新选择，不能偷偷改用本机默认值。

发送消息时，模型、思考强度和权限随线程一起提交到所属机器的运行时。后端必须再次校验线程、项目和运行时目标一致，防止项目切换期间把消息发到错误机器。

## 九、智能体运行工具页面

页面按照以下顺序分组：

```text
本地
远端 1
远端 2
……
```

每组独立显示当前 Panes 已支持的 CLI/智能体是否安装、版本、检测错误和启动按钮。

启动行为沿用现有 Panes：以左侧当前选中的项目作为工作区和执行目标，不增加“最近打开的远端项目”或额外的项目选择流程。当前项目是本地项目时使用本地目标；当前项目是远端项目时使用该项目绑定的 SSH 连接。智能体页面的分组用于展示各执行目标的状态，不能改变当前项目的执行目标，也不能把命令发到其他机器。

当前选中项目属于某个分组时，仅该分组的启动按钮可执行；其他分组仍显示安装和版本状态，但启动按钮禁用，并提示“请先选择该机器上的项目”。这样分组不会变成绕过项目绑定关系的第二个目标选择器。

## 十、错误处理与安全

需要区分并直接显示：

- 本机未安装 `ssh`。
- SSH config 不存在或解析失败。
- IdentityFile 不存在。
- `Permission denied (publickey)`。
- 远端要求密码或键盘交互认证。
- DNS、端口、超时错误。
- 远端不是 Linux。
- 远端目录不存在或无权限。
- 远端 CLI 未安装或版本不兼容。
- SSH 端口转发建立失败、异常退出或本地端口被占用。
- 远端 CLI 服务只启动进程但未在回环端口就绪。
- Claude 远端适配器缺失、上传失败、版本校验失败或运行环境不兼容。
- 手动连接 Host Key 缺失、不匹配或校验失败。
- SSH 连接已禁用或已删除。
- 会话期间 SSH 断开。

不输入、不保存 SSH 密码，不保存私钥内容。所有参数单独转义，日志不能记录密钥、认证信息或完整敏感环境变量。远端删除和覆盖操作必须经过项目根目录范围校验。远端错误不能触发本地 CLI、本地文件或本地 Git 的后备执行。

所有隧道启动均需要就绪检查，不能把“SSH 进程仍在运行”当成 CLI 已可用。就绪条件至少包括：SSH 未退出、本地转发端口可连接、远端服务完成协议级健康检查。超时后必须结束 SSH 和远端服务进程，并返回原始错误及 Panes 的错误分类。

## 十一、预计代码范围

后端主要涉及：

- SSH config 解析、手动连接参数生成、独立 Host Key 文件和连接检测。
- 连接软删除与恢复、项目位置字段、部分唯一索引和定时任务暂停字段的数据迁移。
- `ExecutionTargetResolver`，以及 Workspace、文件、Git、终端、聊天和定时任务的目标路由。
- `SshRuntimeGateway` 提供的命令、数据、PTY、端口转发、就绪检测和生命周期管理。
- 按目标、引擎和运行范围隔离的 CLI 运行时注册表。
- 当前全部 CLI/智能体的远端启动描述与现有通信协议适配。
- Codex WebSocket、OpenCode HTTP/SSE 和 Claude 远端适配器的 SSH 端口转发。
- 附件远端暂存。
- 远端 Git 状态刷新。
- 远端路径大小写敏感处理，以及现有裸路径接口向 `workspace_id` 路由的迁移。

前端主要涉及：

- 设置中的“连接”页面。
- 添加 SSH 连接弹窗。
- 本地/远端项目创建弹窗。
- 远端目录浏览器。
- 项目远端标识。
- 按机器分组的智能体页面。
- 按执行目标隔离的 engine store、模型、账号、用量和健康状态缓存。
- 已删除连接列表、连接恢复和定时任务暂停状态。
- 远端错误提示和状态显示。

新增页面按完整组件封装 DOM、行为和样式，不抽离孤立的通用 CSS。

## 十二、实施顺序

实施按业务模块拆成六个阶段，一个阶段交付一块可操作、可验收的业务功能。数据库迁移、目标解析器、SSH 网关和运行时等公共技术能力不单独划为阶段，而是在第一个需要它的业务阶段中实现，供后续阶段复用。

1. [SSH 连接管理](ssh-remote-project/01-ssh-connection-management.md)：设置中的连接页面、config 扫描、手动添加、检测、启用、禁用、软删除和恢复。
2. [项目管理](ssh-remote-project/02-project-management.md)：本地/远端项目类型选择、远端主机选择、远端目录浏览、项目目标绑定和项目显示。
3. [项目工作区](ssh-remote-project/03-project-workspace.md)：远端文件、Git、工作树、终端和项目启动配置。
4. [对话管理](ssh-remote-project/04-conversation-management.md)：远端 CLI 隧道、消息、附件、模型、思考强度、权限、账号、用量和健康状态。
5. [智能体运行工具](ssh-remote-project/05-agent-runtime-tools.md)：本地及每台远端电脑的工具分组、检测、启动设置和终端启动。
6. [定时任务管理](ssh-remote-project/06-scheduled-task-management.md)：按项目目标执行任务，以及连接禁用、删除、恢复引起的可恢复暂停。

阶段总览、依赖和整体验收见 [SSH 远端功能实施计划](ssh-remote-project-implementation-plan.md)。

必须先由用户完成实际测试并明确确认通过，之后才能按用户后续指令推送。

## 十三、验收标准

### 连接

- 能扫描 SSH config 中的明确 Host，并正确处理 Include。
- 能批量导入主机。
- 能手动添加使用 IdentityFile 并独立维护完整 OpenSSH 公钥 Host Key 的连接。
- 需要密码的主机返回明确错误，界面不出现密码输入框。
- 未知 Host Key、Host Key 不匹配和仅填写指纹时返回明确错误，不自动接受或覆盖。
- 重启 Panes 后连接配置仍然存在。
- 删除连接后，关联项目不再显示，运行时、隧道和终端全部关闭。
- 已删除连接可以恢复，恢复前后的 `ssh_connection_id` 完全一致。

### 项目

- 创建项目时可以选择本地或远端。
- 切换远端主机后，目录列表来自新选择的主机。
- 不同主机可以添加相同的 Linux 路径。
- 同一主机的远端路径按 Linux 大小写规则判断，`/Code/App` 与 `/code/app` 不被本机 Windows 规则错误合并。
- 远端项目在侧边栏具有明确标识。
- 删除并恢复连接后，项目 ID、归档状态和显示名称保持不变。

### 远端能力

- 文件浏览、编辑、创建、删除和搜索均发生在远端。
- Git 和工作树操作均发生在远端。
- 终端进入远端项目目录。
- 当前 Panes 已支持的全部 CLI/智能体均由远端 CLI 处理消息。
- Codex、OpenCode 和 Claude 的持续连接接口均只监听远端回环地址，并通过 SSH 本地端口转发连接。
- 模型、思考强度、权限、CLI 状态、登录账号和用量来自项目所属机器。
- 切换机器时不会短暂显示或提交上一台机器的模型、账号、用量和权限数据。
- 附件能够上传到远端并被 CLI 使用。
- 远端项目的定时任务通过远端 CLI 执行；连接删除或禁用时暂停，恢复后不补跑历史触发点。
- SSH 断开或 CLI 不存在时不调用本机能力。

### 数据恢复

- 删除连接前后，项目、仓库、对话、消息、操作记录、定时任务和执行历史的数据库记录仍然存在。
- 恢复连接前后，`ssh_connection_id`、`workspace_id`、`repo_id`、`thread_id`、`message_id`、`scheduled_task_id` 和已保存的 `engine_thread_id` 均保持不变。
- 恢复后，原来未归档的项目重新显示，原来已归档的项目仍保持归档。
- 定时任务恢复后保留原 `enabled` 状态；已启用任务从当前时间计算下一次执行时间。

### 兼容性

- 原有本地项目无需重新添加。
- 本地项目的文件、Git、终端和聊天行为保持不变。
- 现有手机远程访问功能保持独立并继续可用。
- 多台远端机器的运行时、模型和会话互不串用。
- 同一台远端机器的多个项目不会因工作目录或 CLI 会话切换而串用上下文。

## 十四、已确认的默认处理

1. 手动添加 SSH 连接保留独立 Host Key 管理；接受完整 OpenSSH 公钥，不接受只有 SHA256 指纹的输入。Host Key 保存于 Panes 并用于独立身份校验，不自动获取、替换或覆盖。
2. SSH config 导入连接以 alias 为执行入口，沿用 ProxyJump、IdentityFile、ssh-agent 和系统 Host Key 配置，但始终使用非交互模式。
3. SSH 连接删除采用软删除；项目、对话、消息和定时任务只隐藏或暂停，不物理删除。
4. 连接恢复必须复用原 `ssh_connection_id`，全部关联记录和原 ID 保持不变。
5. 首版不提供永久删除 SSH 连接关联数据。
6. 连接禁用后保留并显示项目，但项目不可执行；禁用与删除是两个独立操作。
7. 需要持续连接的远端 CLI 服务统一使用远端回环监听和 SSH 本地端口转发，不按 CLI 单独建设 SSH 业务连接。
8. 模型、思考强度、权限、健康状态、账号、用量、MCP、Skills 和扩展能力均来自项目所属机器。
9. 远端项目不支持调用本地资源管理器或本地默认应用。
10. 智能体启动使用左侧当前选中的项目，不新增最近远端项目或额外项目选择逻辑。
11. SSH config 的通配符 Host 不展示，只展示可明确选择的别名。
12. 远端错误绝不触发本地 CLI、本地文件、本地 Git 或本地附件路径的后备执行。

## 十五、文档状态

- 方案分支：`codex/ssh-remote-project`。
- 方案工作树当前提交：`08301f41f8174d35c043bce1e3936865e66c166f`。
- 本地 `master` 在修订时的提交：`3c9462404c8e51ca6abd9fcdfb03407c4e97a048`；进入实现前需要先明确并同步实施基线。
- 当前工作树 Git 元数据仍指向搬迁前目录并被标记为 `prunable`；进入实现前必须修复并重新验证。
- 修订日期：`2026-08-11`。
- 当前状态：v2 修订版，待审核。
- 用户明确回复“方案通过，可以开始”前，不进入功能实现。
