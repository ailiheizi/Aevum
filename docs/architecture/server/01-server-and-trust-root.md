# 服务端架构 + 信任根

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:供给模型 [`../foundations/04-index-and-supply.md`](../foundations/04-index-and-supply.md)、远程缓存 [`../runtime/06-remote-cache.md`](../runtime/06-remote-cache.md)、Foundation [`../../layers/01-foundation.md`](../../layers/01-foundation.md)
> 回应:设计评审 H1/H2(索引信任根未定义)、A 类决策(索引分发与缓存的基础设施)

---

## 0. 设计哲学

> **中心服务端提供"可信的真理来源"与"快的缓存",但客户端永不把命脉交给它 ——
> 内容寻址 + 签名验证让客户端能独立验真,服务端宕了/不可信了,系统仍能用已有 lock 运行。**

决策(经确认):Aevum 走**中心服务端**路线(类比 cache.nixos.org),但保留去中心降级,不做强绑定。

---

## 1. 服务端职责

```text
┌─────────────────────────────────────────────────────────────┐
│                     Aevum 中心服务端                          │
├─────────────────────────────────────────────────────────────┤
│  ① Index Service    — 签名的包索引分发(name@ver→hash 映射)  │
│  ② Cache Service    — 内容寻址二进制缓存(substituter)       │
│  ③ Foundation Channel — 密封核心的签名升级通道                │
│  ④ Transparency Log — 签名透明日志(可审计、防偷偷换版本)    │
└─────────────────────────────────────────────────────────────┘
        │ 全部产物带签名,客户端独立验真
        ▼
   客户端:验签通过才用;失败/离线 → 降级用本地已有
```

| 服务 | 提供什么 | 对应文档 |
|---|---|---|
| Index | 供给管线产出的签名索引快照(`index_snapshot`) | [供给模型](../foundations/04-index-and-supply.md) |
| Cache | 预构建二进制,按 hash 寻址 | [远程缓存](../runtime/06-remote-cache.md) |
| Foundation Channel | foundation-manifest 升级 | [Foundation](../../layers/01-foundation.md) §5 |
| Transparency Log | 所有签名的 append-only 日志 | 本篇 §4 |

---

## 2. 中心化的边界:哪些依赖服务端,哪些不

**关键原则:服务端提供便利与新鲜度,不提供"系统能否运行"的命脉。**

| 操作 | 是否需要服务端 |
|---|---|
| 用已有 lock 重放/回滚历史世代 | ❌ 纯本地(见 [PoC-3](../../../poc/poc3-zero-ai-solver/REPORT.md)) |
| 用本地 store 已有包求解新世代 | ❌ 纯本地 |
| 获取新包 / 新索引快照 | ✅ 需要(或镜像/局域网) |
| 安全更新通知 | ✅ 需要 |
| Foundation 升级 | ✅ 需要(但旧版离线永久可用) |

→ 服务端宕机 = 不能装新东西,但**已装的一切照常运行、照常回滚**。这是和"把系统托管给云"的根本区别。

---

## 3. 信任根(回应 H1/H2)

评审 H1:内容寻址只保证"字节==hash",不保证"这个 hash 是 python 该有的那个";这层靠签名,但签名机制此前是占位级。本节定义它。

### 3.1 两层防护(各管一件事)

```text
内容寻址  → 防"内容和 hash 不符"(传输损坏/篡改)        [已有,见 store 文档]
签名      → 防"索引里登记了一个恶意 hash"(供应链投毒)   [本节定义]
```

### 3.2 签名层级:谁签什么

```text
Root Key（离线 HSM,极少使用）
   │ 签
   ▼
Intermediate / Channel Keys（stable / beta，定期轮换）
   │ 签
   ▼
- Index Snapshot 签名:对整个 name@ver→hash 映射快照签名
- Foundation Manifest 签名:见 Foundation 文档
- Cache 产物:内容寻址已自验,签名可选加强
```

### 3.3 多方签名 / 阈值(对关键映射)

- 普通包索引:Channel Key 单签即可。
- **Foundation 与高敏感包**:采用 **阈值签名(M-of-N)** —— 需要多个独立持钥方共同签署才生效,降低单点被攻破的风险。
- 阈值参数随发布策略配置,记录在信任策略文件。

### 3.4 首次信任建立(TOFU vs 出厂内置)

```text
不用 TOFU(首次盲信有中间人风险)。
采用:出厂内置 Root public key
   → 随客户端/安装介质分发,带外可校验指纹
   → index_snapshot / manifest 都锚定到这个内置根
```

### 3.5 密钥轮换与撤销

```text
轮换:Intermediate/Channel Key 定期轮换,新 key 由 Root 签发;
      客户端通过已签的"key 列表"获知当前有效 key 集。
撤销:被攻破的 key 进入签名的撤销列表(CRL 风格);
      客户端拒绝任何被撤销 key 的签名;
      撤销列表本身由 Root 签,且进透明日志。
```

### 3.6 index_snapshot 如何锚定到根

```text
index_snapshot（求解输入,写进 lock）
   ├─ 内含其内容摘要
   ├─ 由当前有效 Channel Key 签名
   └─ Channel Key 由 Root 签发
→ 客户端验签链:snapshot → Channel Key → Root(内置)
→ 锚定成立:lock 里记录的 snapshot 摘要可被独立验真
```

这补上了评审指出的"index_snapshot 如何锚定到信任根"的缺口。

---

## 4. 透明日志(Transparency Log)

```text
所有签名(index snapshot、foundation manifest、key 轮换/撤销)
   → 写入 append-only、可公开审计的透明日志(类比 Certificate Transparency)
作用:
  - 防"给个别用户偷偷推不同版本"(评审关注点):
    任何签名都留痕,用户/第三方可比对自己收到的与日志是否一致
  - 可被独立监控者审计,异常签名可被发现
```

这呼应 Foundation 文档"不给个别用户偷推不同版本"的承诺,并给它一个机制保证。

---

## 5. 去中心降级(不强绑定)

虽然走中心服务端,但设计保留逃生通道:

```text
- 镜像:任何人可镜像签名索引 + 缓存(签名让镜像无需被信任)
- 局域网/点对点:缓存可在内网/P2P 分发(内容寻址 + 签名兜底)
- 离线 bundle:导出一个世代的完整 closure 离线安装
- 自建 index:组织可运行自己的 Index Service,配自己的信任根
```

→ 中心服务端是默认与便利,不是垄断点。签名机制让"换一个不可信的分发渠道"也安全。

---

## 6. 与商业/运营的关系(诚实声明)

- 中心服务端有真实运营成本(存储、带宽、签名基础设施)。这是选择中心化要承担的。
- 承诺(与 Foundation 文档一致):不夹带遥测、不因商业下架核心包、不给个别用户偷推不同版本(由透明日志保证)。
- 商业模式(若有)不得侵蚀上述承诺;可持续性是真实课题,本篇标注但不在设计阶段定死。

---

## 7. 边界与待办

- 本篇定信任根的**架构与接口**;具体密码学选型(签名算法、HSM 方案、阈值方案实现)待实现期。
- 透明日志的具体数据结构(Merkle tree 等)待实现期。
- 服务端自身的高可用、抗 DDoS 等运维课题不在设计阶段。

---

## 8. 验收清单

- [ ] Index/Cache/Foundation Channel/Transparency Log 四服务职责清晰
- [ ] 中心化边界明确:服务端宕机不影响已有 lock 运行/回滚
- [ ] 两层防护(内容寻址 + 签名)各司其职
- [ ] 签名层级(Root→Channel→产物)+ 出厂内置根
- [ ] Foundation/高敏感包阈值签名
- [ ] 密钥轮换 + 撤销列表(Root 签 + 进日志)
- [ ] index_snapshot 验签链锚定到内置根
- [ ] 透明日志防"偷推不同版本"
- [ ] 去中心降级通道(镜像/P2P/离线/自建 index)
