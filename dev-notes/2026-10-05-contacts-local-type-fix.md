# 修复记录：联系人接口丢掉 local_type，好友与群陌生人无法区分

整理日期：2026-10-05。排查版本：agent-wechat `v0.15.1`（fork 基线
`96c2e90`，当前 HEAD `4eadd0e`）。
状态：已完成只读定位，尚未修改代码或部署修复。

## 现象与已确认原因

`GET /api/contacts` 只认 `limit`、`offset`；`GET /api/contacts/find` 只认
`name`。服务端已经从 `contact.db` 查出 `local_type`，但只用它计算
`contactType`，随后丢掉，响应里没有原始类型。

`contactType` 只有四档：`individual` / `official` / `chatroom` / `openim`。
分类规则是：`gh_` → 公众号，`@openim` 或 `local_type == 5` → 企业微信，
用户名以 `@chatroom` 结尾 → 群，其余个人一律 `individual`。真正的好友
（`local_type = 1`）和群陌生人（`local_type = 3`）都会变成 `individual`，
客户端分不开。

对照上游 `thisnick/agent-wechat` 的基线 `96c2e90`（package `0.15.1`）及
其后已合入本 fork 的媒体 / 语音 / 4.1.13.23 等提交，联系人接口、`Contact`
结构和分类规则均未改动。GitHub 上也没有针对该问题的 PR 或 issue。

## 排查证据

本轮只读对照源码与上游，没有查询真实 `contact.db`，没有重启容器或发送
消息。本文不含账号身份、wxid、备注或数据库路径。

- `packages/agent-server-rust/src/router/contacts.rs` 的 `ListParams` 仅
  有 `limit`（默认 200）和 `offset`；`FindParams` 仅有 `name`。
- `wechat_contacts.rs` 的 list / find SQL 都选出 `local_type`，过滤
  `local_type IN (1, 3, 5)` 且排除 `@chatroom`。代码注释记录的取值：
  `0` 系统通知、`1` 好友+公众号、`2` 群、`3` 通讯录联系人、`5` 企业微信。
- `classify_contact()` 只在 `local_type == 5` 时使用该字段；`1` 和 `3`
  都落入 `else` 分支，返回 `"individual"`。
- `Contact`（`ia/types.rs`，经 ts-rs 生成 `Contact.ts`）字段为
  `username`、`nickName`、`remark`、`alias`、`smallHeadUrl`、
  `contactType`，没有 `localType`。
- 共享客户端 `listContacts(limit?, offset?)`、`findContacts(name)` 与
  CLI `wx contacts list|find` 同样只暴露上述入参，列表输出只打印
  `contactType`。
- Wechaty puppet 的 `contactToContactPayload()` 把除 `official` 以外的
  全部映射为 `Contact.Individual`，并把 `friend` 写死为 `true`。
  `loadContacts()` 一次拉取最多 5000 条，没有按类型过滤。
  `type-map.test.ts` 覆盖了 `chatToContactPayload`，没有覆盖
  `contactToContactPayload` 的好友/陌生人区分。
- 公开 API 文档只登记了 `GET /api/contacts`，未说明查询参数、返回字段
  或 `GET /api/contacts/find`。

## 需要修改的位置

| 文件 / 函数 | 当前问题 |
| --- | --- |
| `packages/agent-server-rust/src/ia/types.rs`：`Contact` | 响应类型没有 `localType`；`contactType` 注释只列出四档。 |
| `packages/agent-server-rust/src/tools/wechat_contacts.rs`：`classify_contact()` | `local_type` 为 `1` 与 `3` 时都返回 `individual`。 |
| 同文件：`list_contacts()` / `find_contacts()` | 查出 `local_type` 后只传入分类函数，不写入返回值；查询不能按类型过滤。 |
| `packages/agent-server-rust/src/router/contacts.rs`：`ListParams` / `FindParams` | 列表只认 `limit`/`offset`，搜索只认 `name`。 |
| `packages/shared/src/types/generated/Contact.ts` | 由 Rust 生成，缺 `localType`；需随类型变更重新生成。 |
| `packages/shared/src/client.ts`：`listContacts` / `findContacts` | 客户端方法未传递类型过滤参数。 |
| `packages/cli/src/cli.ts`：`cmdContacts` / `cmdContactsFind` | 输出只显示 `contactType`，无法看出好友与陌生人。 |
| `packages/wechaty-puppet/src/type-map.ts`：`contactToContactPayload()` | `friend: true` 写死；非公众号一律 `Individual`。 |
| `packages/wechaty-puppet/src/puppet-agent-wechat.ts`：`loadContacts()` | 依赖上述映射，通讯录里的非好友也会被标成好友。 |
| `docs/src/content/docs/reference/api.mdx` | 联系人接口说明不完整。 |

## 修复建议

1. 在 `Contact` 上增加 `localType`（数值，与 `contact.db` 一致），list /
   find 都原样返回。保留现有 `contactType` 四档，避免破坏已有客户端；
   不要用更细的字符串档位悄悄替换现有值。变更后运行 `pnpm generate-types`。
2. 列表和搜索增加可选的 `localType` 过滤。未传时保持当前
   `IN (1, 3, 5)` 且排除 `@chatroom` 的行为。过滤值需要校验，未知取值
   返回明确错误，不要静默忽略。
3. 共享客户端、CLI 与公开 API 文档同步：查询参数、返回字段、
   `local_type` 已知取值，以及 `1` 与 `3` 都会出现在默认列表中。CLI
   文本输出应能看出 `localType`（JSON 模式会随类型自动带上）。
4. puppet 用 `localType === 1`（再排除 `gh_` 公众号）设置 `friend`；
   `openim` 仍按现有 `contactType` 处理。`chatToContactPayload` 没有
   `localType`，保持现有行为，不要把会话列表里的人一律改成非好友。
5. 不要在本修复中扩大 SQL 范围（例如加入 `local_type = 2` 的群），
   也不要改写微信 `contact.db`。分类规则仍以用户名后缀 / 前缀为主，
   `local_type` 作为附加字段而不是替换 `contactType`。

## 验收条件

- Rust 侧用 fixture 或构造行覆盖：`local_type` 为 `1`、`3`、`5`，以及
  `gh_`、`@openim`、系统账号过滤。断言响应同时包含 `contactType` 与
  `localType`；`1` 与 `3` 的 `contactType` 仍为 `individual`，但
  `localType` 不同。
- `GET /api/contacts?localType=1` 只返回该类型；缺省查询仍包含 `1/3/5`。
  非法 `localType` 不得当成成功空列表。
- `pnpm generate-types` 后 TypeScript `Contact` 含 `localType`；共享
  客户端与 CLI 能传过滤参数，文本列表能显示该字段。
- puppet 单测覆盖 `contactToContactPayload`：`localType = 1` 时
  `friend === true`，`localType = 3` 时 `friend === false`；公众号仍为
  `Official`。现有 `chatToContactPayload` 用例不回归。
- 在隔离测试容器中用测试账号确认 list / find 返回的 `localType` 与
  通讯录观感一致（好友为 `1`，仅群内出现的人为 `3`）。不要对生产容器
  或线上机器人做这次验证。
- 用户可见行为修复完成后，按仓库 `AGENTS.md` 添加对应 changeset。

## 下游客户端问题（另行修复）

下游若已经把 `contactType === "individual"` 当成“好友”，仅增加
`localType` 不会自动纠正错误缓存。Wechaty 的 `friendship*` API 仍是
未实现桩，本修复只纠正通讯录载荷里的 `friend` 标志，不提供加好友 /
通过好友请求能力。
