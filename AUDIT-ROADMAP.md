# Aevum v0.1.0 审计路线图

> 来源:8 维度并行审计(code-quality / test-gaps / usability-gaps / docs-consistency / release-readiness / security / robustness / architecture-adr),共 **57 条**发现,已跨维度去重合并。
> 三档按 影响/工作量 排序。effort 取值:trivial / small / medium / large。

> **进度(2026-06-26)**:P0 七条已处理 —— P0-1 ✅(609a32d)、P0-2 ✅完整性/⏳签名(b2af2f7)、P0-3 ✅(609a32d)、P0-4 ✅(aa4e62a)、P0-5/6 ✅(2f7b8b2)、P0-7 ✅(a49cfd1)。剩 P0-2 签名半边(需 vendor ed25519)+ 全部 P1/P2。
>
> **进度(2026-06-27)P1 批次**:P1-1 CI ✅(3d9f041→6bade99,build-test 绿)、P1-2 clean-clone ✅(2302407)、P1-3 update 不毁索引 ✅(c735587)、P1-4 aevum init ✅(5a15431)、P1-5 并发锁 ✅(2bf937c)、P1-6 NAR 中断恢复 ✅(5970cce)、P1-7 make_generation 原子构建 ✅(da060c5)。剩 fmt/clippy 转阻断(本地装不上组件)、P0-2 签名半边。
>
> **进度(2026-06-27)P1 批次2**:P1-13/14/15/16 AI JSON 加固 ✅(2c9cb22)、P1-8 elf 正向测试 ✅(89f5c68)、P1-9 NAR 穿越/边界测试 ✅(c84915a)、P1-10 store symlink 测试 ✅(1ad67d8)、P1-11 跨源阻断断言 ✅(bbfe07a)。全部 CI build-test 绿。剩 fmt/clippy 转阻断、P0-2 签名、P1-12 已在更早修复、零散 P2。
>
> **进度(2026-06-28)P1 批次3**：P1-17 key 不走 argv ✅(3f50249)、P1-19 put_file 解 symlink mode ✅(7e76f61)、P1-20/21 curl 超时重试 ✅(3406c75)、P1-24 镜像读 config ✅(93b51a4)、P1-18 Foundation 自动发现 ✅(4321988)。P1 全清。

---

## P0(发布前必修)

> critical/high 级安全、正确性 bug、ADR 红线违反。这些不修就发,等于把"可复现、AI 在笼子里、回滚可信"三大承诺直接打穿。

### P0-1 narinfo StorePath 路径穿越 → 任意文件写入【critical / small】
- **为什么重要**:`StorePath` 直接来自下载的 narinfo,未校验;Unix 下 `Path::join` 遇绝对路径会丢弃 base,`../` 不被清理,恶意/被劫持镜像可把包内容(含可执行位、symlink)写到任意可写路径(如 `/etc/cron.d/evil`)。把任意不可信镜像变成任意写。
- **修复**:用前校验 StorePath 必须以 `/nix/store/` 开头,剩余 `<hash>-<name>` 段拒绝 `/`、`..`、`.`、空段;unpack 前断言 canonical dest 仍在 store_dir 内;narinfo 的 hash 必须等于请求 hash。
- **位置**:`crates/nix-source/src/cache.rs:72-73`(配合 `narinfo.rs:48`)

### P0-2 Nix 二进制缓存零完整性/签名校验【critical / medium】(security#2 + robustness#2 合并)— ✅ 完整性已修(b2af2f7)/ ⏳ 签名待 vendor
- **为什么重要**:`FileHash`/`NarHash`/`Sig` 都被解析进结构体却从不使用;`curl -sL | xz -d` 直接喂给 `nar::unpack`,无哈希对比、无签名校验、`-L` 跟随重定向且无证书固定。与已校验 SHA256 的 Debian 路径(`lib.rs:796-804`)严重不对称,直接架空 ADR-0001 "可复现来自 lock"。
- **修复**:下载后对压缩流比对 FileHash、对解压 NAR 比对 NarHash(不匹配即中止+删除);用配置的可信公钥(ed25519,同 Nix `trusted-public-keys`)验 `Sig`;未签名/不可验证的 narinfo 默认致命,除非显式 insecure 开关。
- **位置**:`crates/nix-source/src/cache.rs:70-118`;`narinfo.rs:51,53,61`

### P0-3 resolve_package / update 经 `sh -c` 注入【high / small】(security#3 + code-quality#5 + robustness#8 合并)
- **为什么重要**:`format!("curl -sL '{}' | xz -d | grep -F -- '-{}'", url, name)` 把 `name`/`channel_url` 插进单引号 shell 串,一个单引号即可逃逸(`x'; rm -rf ~; echo '`)。今天输入是 operator CLI 参数,但 AI dispatch 层已经在产出包名字符串,一旦接入即 RCE。
- **修复**:弃用 `sh -c`,像 `fetch_and_unpack` 那样 argv 形式 spawn curl + Stdio::piped() 接 xz/gunzip,匹配在 Rust 里做;`name` 仅作数据传入。
- **位置**:`crates/nix-source/src/cache.rs:166-171`;`crates/cli/src/main.rs:567-568`

### P0-4 `aevum ai "install ..."` 绕过 verify 门禁【high / medium】(ADR-0003/0005 漂移)
- **为什么重要**:旗舰卖点是"AI 维护者关在笼子里,决策过独立 verify 机器"。但最 AI 驱动的入口直接 `do_install → install() = propose_generation + set_active`,完全不走 verify_generation/activate_verified。AI 选出的包集若构成版本回退(或将来 CVE 命中)会被静默激活,无独立判定、无强制确认。
- **修复**:把 ai install 意图路由到 gated maintain_from_lock/activate_verified(带对 active lock 的回退检测);至少在 set_active 前跑 verify_generation,needs_user_confirm 时拒绝/强制确认。
- **位置**:`crates/cli/src/main.rs:617-637`;`crates/cli/src/lib.rs:1003-1014`

### P0-5 `aevum list`/`remove` 读"最近修改的 lock"而非 active 世代,回滚后撒谎【high / medium】(正确性)
- **为什么重要**:两个 handler 都按 mtime 取 `locks.first()`,不解析 `generations/active`。`aevum rollback 1` 后 active 是 gen-1,但磁盘上最新 lock 仍是被回滚掉的那个 → `list` 显示错误包集,`remove` 从错误基线重建。直接打穿"回滚回到已知状态"的核心承诺。
- **修复**:每个世代构建时把 lock 名/内容持久化进 `gen-NNN/lock.txt`,List/Remove 用与 refresh_profile 相同逻辑解析 active symlink 再读该世代 lock;active 世代无 lock 则显式报错。
- **位置**:`crates/cli/src/main.rs:694-719`(List)、`720-764`(Remove)

### P0-6 `aevum remove` 从完整闭包重解析,留下全部孤儿依赖【high / medium】(正确性)
- **为什么重要**:`lock.locked` 是完整传递闭包而非顶层意图。`remove foot` 会把 foot 拉进来的每个库当成顶层约束保留,新世代几乎没瘦身;若用户删的是只作依赖存在的包则被静默过滤。结果 remove 完全不像 `apt remove`。还硬编码 lock 名 `"removed"`,跨操作碰撞并喂给上面的坏启发式。
- **修复**:按世代记录用户顶层包列表(与解析闭包分开),remove 操作顶层集后从缩减后的顶层集重解析闭包;lock 按世代命名而非固定 `removed`。
- **位置**:`crates/cli/src/main.rs:736-758`

### P0-7 `gc --keep` 文档语义错误,会误删本想保留的世代【high / trivial】(docs,破坏性)
- **为什么重要**:`--keep` 实为"保留这些世代 id",但 README 写成"保留最近 N 个世代"。`aevum gc --keep 3` 实际只留 gen#3、回收其余。GC 是破坏性操作,照文档走会丢数据 —— 最危险的文档错。
- **修复**:README 命令表与教程改为 `aevum gc --keep <gen-id,...>` → "保留指定世代 id 引用的对象,共享依赖不误删",示例改 `--keep 1,2,3`。
- **位置**:`README.md:94, 230-231` vs `crates/cli/src/main.rs:198-203, 819-841`

---

## P1(高价值)

> 错误恢复、零 CI、关键路径测试缺口、已确认文档 bug、ADR 次级漂移。发布可带病,但应紧随其后。

### P1-1 全仓零 CI/CD,所有 unix/network 测试形同虚设【high / medium】(test-gaps#1 + release#1 合并)
- **为什么重要**:无 `.github/workflows`。整个安全故事(setuid 位、symlink 保留、原子切换、dlopen 闭包)都在 `#[cfg(unix)]` 与 `#[ignore]` 测试里;开发机是 Windows,`cargo test` 全跳过。setuid 处理或原子 rename 的回归可绿色落地。
- **修复**:加 ubuntu-latest workflow 跑 `cargo build/test --workspace`、`cargo clippy -- -D warnings`、`cargo fmt --check`(钉 Rust 1.85);另设 gated job 跑 `cargo test -p aevum-nix-source --test nix_e2e -- --ignored`;fixture-gated 测试在期望有 fixture 的 runner 里静默跳过时让 build 失败。
- **位置**:`.github/`(缺失);`crates/nix-source/tests/nix_e2e.rs:23,34,60`

### P1-2 离线 vendor 的 `.cargo/config.toml` 让任何 clean checkout 编译失败【high / small】
- **为什么重要**:提交的 cargo config 强制所有依赖走被 gitignore 的 `vendor/`,fresh clone 里不存在;README 的 `git clone && cargo build` 对作者预 vendor 的 WSL 之外所有人直接硬报错。CI 也会撞同一面墙。
- **修复**:(a) 不提交 source-replacement,改本地 opt-in(`.cargo/config.toml.offline`)并文档化 `cargo vendor`;或 (b) 提交 vendor 树;或 (c) 条件化。至少在 README 注明 clean online 构建需移除/覆盖该 config 或先 `cargo vendor`。
- **位置**:`.cargo/config.toml:3-7`;`.gitignore:8`

### P1-3 `aevum update` 管道吞 curl 失败 + `>` 重定向先截断索引【high / small】(usability#1 + robustness#5 合并)
- **为什么重要**:新用户跑的第一条命令。`sh -c "curl -sL '' | gunzip > '{}'"` 只看到 gunzip 退出码(curl 失败被掩盖),且 `>` 在 fetch 前已截断目标。镜像不可达/404/掉线 → 索引空或截断,后续所有读索引命令以混乱的下游错误失败。
- **修复**:curl 与 gunzip 分两步,`curl --fail -sL -o tmp.gz` 校验成功,gunzip 到临时文件,断言含至少一条 `Package:` 且字节数合理后原子 rename 覆盖;加 `--max-time`;失败时报"index download failed from <url>"。
- **位置**:`crates/cli/src/main.rs:561-577`

### P1-4 无 `aevum init`,README quick-start 在文件创建前就 source env.sh【high / medium】(usability#4 + docs#6 合并)
- **为什么重要**:`env.sh` 只在 refresh_profile(install/switch/rollback 时)写;quick-start 步骤 1 `aevum update` 后即 `source $AEVUM_ROOT/profile/env.sh`,此时文件不存在 → `No such file or directory`。首次用户最大的"开箱即崩"。
- **修复**:加 `aevum init`:建 root、写空 profile/bin + env.sh(让首装前 PATH/source 可用)、拷 `examples/config.toml`、跑 update;README 以它开篇并统一 PATH 机制。
- **位置**:`crates/cli/src/main.rs:20-310`(无 Init);`crates/cli/src/lib.rs:1432-1440`;`README.md:20-22`

### P1-5 无并发锁,两个 aevum 进程竞争 store/世代目录/active 指针【high / medium】
- **为什么重要**:全仓无 advisory file lock。`next_generation_id` 取 max+1,两个并发 install 算出同一 id 并向同一 `gen-NNN/packages` 交错写 symlink;共享 per-package unpacked 目录互相 `remove_dir_all`。set_active 的 rename 是原子的,但其前的一切都不是。
- **修复**:任何变更命令(install/maintain/remove/update/gc)开头对 `$AEVUM_ROOT` 加排他 flock,结束释放;gen id 在锁内分配。
- **位置**:`crates/cli/src/lib.rs:735-750, 888-998`;`crates/generation/src/lib.rs:84-145`

### P1-6 NAR 下载中断后跳过完整性检查,把半成品目录当完整【high / medium】
- **为什么重要**:`if dest.exists() { return Ok(0); }`,仅在 xz 非零退出时清理。SIGKILL/断电/磁盘满 中途留下半写 dest,下次 `exists()` 即跳过当完整 —— 静默损坏的 store 对象被链入世代。(与 P0-2 互补:那条是校验缺失,这条是中断恢复)
- **修复**:unpack 进临时目录,xz 退出 0 且 NarHash/NarSize 校验通过后原子 rename;或写 `.complete` 标记,无标记目录视为不完整。
- **位置**:`crates/nix-source/src/cache.rs:70-118`

### P1-7 make_generation 原地构建,中断留半填充 gen-NNN【medium / medium】
- **为什么重要**:`gen-NNN/packages` 直接建并循环 symlink,`lock.txt` 最后才写,无 temp+rename。中途崩溃留下部分链接、无 lock.txt 的 gen-NNN;`next_generation_id` 仍计入,后续 compose/export/refresh 读到不完整闭包。
- **修复**:建进 `gen-NNN.tmp.<pid>`,写完 lock.txt 后 rename;缺 lock.txt 的 gen-NNN 视为不完整(在 next_generation_id/refs 中跳过)。
- **位置**:`crates/generation/src/lib.rs:84-118`

### P1-8 ELF DT_NEEDED/PT_INTERP/scan_dir 解析无正向单测【high / medium】
- **为什么重要**:elf crate 是 dlopen 闭包补全(PoC-5 铁律)的地基,却只有两个负向测试。"扫全包 ELF 而非仅主二进制 DT_NEEDED"这个最重要的正确性属性从未被正向断言;NEEDED 提取或 symlink-skip 分支的 bug 只会在可能永不发生的 WSL fixture 运行里冒头。
- **修复**:提交一个小静态 ELF fixture(或用 object/goblin 构造),断言 parse_bytes 返回期望 needed/interpreter/soname;建含 2 ELF + 1 非ELF + 1 symlink 的临时目录,断言 scan_dir 仅返回 2 个真 ELF 且有序。
- **位置**:`crates/elf/src/lib.rs:50-143,149-159`

### P1-9 NAR symlink unpack 分支与全部 hardening 限制无测试【high / medium】
- **为什么重要**:NAR 摄入来自远程镜像的不可信字节。目录项名守卫(拒 `/`、`..` 的路径穿越防御)无测试证明恶意名被拒;几乎每个真实 Nix 包都用的 symlink 节点完全无单测;padding 与 depth/limit 边界正是手写二进制解析器崩的地方。
- **修复**:构造 NAR 字节流单测:symlink 节点、名为 `..`/含 `/` 的目录项(断言 NarError::Format)、超 MAX_DEPTH、超长 name/target、非 utf8 内容、截断流、size%8==0(pad==0 边界)。
- **位置**:`crates/nix-source/src/nar.rs:107-122,45-47,99,110,131-141`

### P1-10 store symlink 往返与 get() symlink 校验路径即使在 unix 也无测试【high / small】
- **为什么重要**:PoC-5 铁律是 symlink 保留不解引用。`put_symlink`、get 的 `is_symlink` 内容校验分支、ingest_dir 的 symlink 处理全无测试,虽是可在内联模块跑的 `#[cfg(unix)]` 代码。setuid 往返都有专测,symlink 同为铁律同样可测却没有。
- **修复**:加 cfg(unix) 内联测试:put_symlink 后 get() 成功(往返 + loadtime 哈希过链接目标);篡改链接目标须 HashMismatch;ingest_dir over 含 symlink 的目录须产出 meta.is_symlink=true 且未解引用。
- **位置**:`crates/store/src/lib.rs:135,180-206,262,454-456`

### P1-11 closure-builder Strict 跨源阻断从未被断言真的返回 Err(CrossSource)【high / medium】
- **为什么重要**:PoC-4 铁律(同源闭包、硬阻断跨源,坑在 ABI)。现有测试自承"仅确认空包 Ok",从未驱动 BFS 解析外源库并断言 Err。把 Strict 翻回 Lenient 或跳过策略检查的回归会通过测试套件。
- **修复**:造一个返回 provenance != input.source 库的 fake LibResolver(无需真 ELF),seed needed_libs,断言 Strict 返 Err(CrossSource) 而 Lenient 记 CrossSourceHit。
- **位置**:`crates/closure-builder/src/lib.rs:417-488,862-888`

### P1-12 README export-system 用 `--generation` flag,CLI 实为位置参数【high / trivial】
- **为什么重要**:`generation: u64` 是裸位置字段;README 命令表与教程都写 `--generation <N>`,clap 直接拒为未知 flag。复制粘贴即 parse error;export 家族系统性文档错。
- **修复**:README 改 `aevum export-system <N> --out <dir>`(位置 id),如 `aevum export-system 1 --out /tmp/my-rootfs`。
- **位置**:`README.md:93,238` vs `crates/cli/src/main.rs:176-183`

### P1-13 extract_content 引号配对在闭引号前的转义反斜杠上失效【high / small】(code-quality)
- **为什么重要**:`extract_openai/claude_content` 只回看一字节判断闭引号转义,未实现奇偶反斜杠规则。AI 响应内容合法地以反斜杠结尾(代码片段、Windows 路径、正则)时,提取器吞掉闭引号继续吃字节,返回含下游字段的垃圾,可污染 reply 乃至 PACKAGES 行。
- **修复**:数连续前导反斜杠,偶数才视为终止符;更好是复用 `lib.rs::extract_content`(276-306)那个正确的逐字符 unescaper,别维护第二个有 bug 的扫描器。
- **位置**:`crates/intent/src/ai_client.rs:228-235,246-255`

### P1-14 两套分歧的 JSON 提取实现,AI 实际路径用的是错的那套【medium / medium】(code-quality)
- **为什么重要**:ai_client 用顺序相关且错误的 `.replace()` 链(`\\n` 先被变成真换行),lib.rs 用正确的逐字符扫描;live AI dispatch 走的是错的。两者都不解码 `\uXXXX`,而工作语言是中文,unicode 转义的 CJK 被留成字面 `u00e9`。
- **修复**:收敛到单个共享 `extract_json_string`/`unescape_json`(以 lib.rs 扫描器为基),加 `\u` 分支解码 4 位十六进制,四处调用点统一。
- **位置**:`crates/intent/src/ai_client.rs:235,255` vs `crates/intent/src/lib.rs:276-306`

### P1-15 多字节响应在错误路径上按字节切片导致 panic(DoS)【medium / trivial】(code-quality#3 + security#5 + test#6 合并)
- **为什么重要**:`&resp[..resp.len().min(200)]` 在第 200 字节落在多字节 UTF-8 中间时 panic(中文错误体极可能)。这正发生在响应畸形的失败路径,直接打穿 ADR-0005 "AI 可选、优雅降级"——本该返 Err 让 caller 回退显式约束,却 abort 进程。可被攻击者影响的网络输入触发。
- **修复**:按字符边界截断,`resp.chars().take(200).collect::<String>()` 或 `floor_char_boundary`,用于所有不可信响应文本的诊断切片。
- **位置**:`crates/intent/src/ai_client.rs:221,242`;`crates/intent/src/lib.rs:281`

### P1-16 json_escape 漏掉 <0x20 控制字符,产出非法 JSON 请求体【medium / small】(code-quality#4 + security#6 合并)
- **为什么重要**:只处理 `\ " \n \r \t`,其余 C0 控制字节原样进 `"content":"..."`,RFC 8259 禁止,API 以 parse error 拒绝。用户输入/聊天历史含杂散控制字节(粘贴产物、终端转义)即让调用失败。
- **修复**:所有 `<0x20` 转 `\u00XX`(理想上含 U+2028/2029);跨 ai_client 与 lib.rs 共用一个 escaper。
- **位置**:`crates/intent/src/ai_client.rs:259-265`;`crates/intent/src/lib.rs:309-322`

### P1-17 API key 作为 curl argv 传递,暴露给所有本地进程【medium / medium】(security)
- **为什么重要**:`-H "Authorization: Bearer {key}"` 作进程参数,curl 运行期间 `/proc/<pid>/cmdline`、`ps -ef` 世界可读。多用户/容器共享主机上是真实凭据泄露。
- **修复**:经 stdin 用 `-H @-` 或 `--config` 文件(管道/`/dev/fd`)传 auth header;或写 0600 临时文件给 `--config` 用后删。别把密钥放 argv。
- **位置**:`crates/intent/src/lib.rs:257-258`;`crates/intent/src/ai_client.rs:167-168,199,709,735`

### P1-18 Foundation 封印(boundary 2)默认从不被机器强制【medium / medium】(ADR-0003)
- **为什么重要**:verify 只在 `foundation: Some` 时跑判据 3;所有 CLI 命令的 `--foundation` 默认 None,Install 子命令根本没这个 flag,且无默认 foundation.toml 自动加载。ADR-0003 列为不可违反的封印在代码里存在却休眠 —— 默认运行时没有任何东西阻止候选世代删改核心组件。
- **修复**:从 layout 自动发现默认 manifest(如 `$AEVUM_ROOT/foundation.toml`)喂给每次 verify_generation,除非显式禁用;install 路径也加 --foundation 或默认开。
- **位置**:`crates/cli/src/lib.rs:1740-1747`;`crates/maintainer/src/lib.rs:197-225`;`crates/cli/src/main.rs:184-197`

### P1-19 put_file 对 symlink 解析的库/loader 丢失 PoC-6 权限位语义【medium / small】(ADR / PoC-6)
- **为什么重要**:put_file 用 `fs::read`(跟随 symlink 读目标内容)但用 `symlink_metadata` 取 mode(symlink 自身 0o777)。HostLibResolver 不 canonicalize,host 解析的 soname(如 libc.so.6)常是 symlink。普通 0o755 库尚可容忍,但经 symlink 解析的 setuid 二进制会丢 setuid 位 —— 正是 PoC-6 警告的失败。
- **修复**:put_file 在 read_meta 前 canonicalize(或 stat 解析后的真文件)让 mode 反映目标二进制;或让 HostLibResolver 像 PackageLibResolver 一样 canonicalize。
- **位置**:`crates/cli/src/lib.rs:308-319`;`crates/store/src/lib.rs:288-310`;`crates/closure-builder/src/lib.rs:172-181`

### P1-20 Nix cache 的 curl 调用无超时,可无限挂起【medium / trivial】(robustness)
- **为什么重要**:fetch_narinfo/fetch_and_unpack/resolve_package 都无 `--max-time`,对比 download_deb 已设 `--max-time 120`。fetch_closure 里跨整个依赖 BFS 放大,一个卡住的依赖挂死整个闭包获取。
- **修复**:所有 cache.rs curl 加 `--max-time`(理想加 `--connect-timeout` + `--retry`),匹配 download_deb;resolve_package 别用裸管道掩盖 curl 失败(pipefail 或先 fetch 再处理)。
- **位置**:`crates/nix-source/src/cache.rs:49-51,81-83,167-170`

### P1-21 全程无下载重试,单次瞬时网络抖动即中止整个安装/闭包【medium / small】(robustness)
- **为什么重要**:download_deb 与 cache.rs fetch_* 都只跑一次 curl。项目明确瞄准 python(77 扩展)、imagemagick(137 插件)的数百包闭包,真实网络下每轮极可能至少一个包失败,且无在途世代恢复,propose_generation 从头重来。
- **修复**:curl 加 `--retry 3 --retry-delay 2 --retry-connrefused`(最省),和/或把 download_deb/fetch_one 包进有界重试循环;已下载对象经 exists() 跳过部分恢复,但须校验部分对象。
- **位置**:`crates/cli/src/lib.rs:782-794`;`crates/nix-source/src/cache.rs:49-117`

### P1-22 workspace repository URL 指向错误/不存在的 org【medium / trivial】(release)
- **为什么重要**:`repository = "github.com/aevum/aevum"`,实际在 `github.com/ailiheizi/Aevum`。任何 crate 元数据消费者(crates.io、docs.rs、IDE 链接)被导向占位/不存在仓库。模板拷贝产物,发布/打 tag 前须改。
- **修复**:`[workspace.package]` 里设 `repository = "https://github.com/ailiheizi/Aevum"`(可加 homepage/documentation)。
- **位置**:`Cargo.toml:29`

### P1-23 install 即时激活、无确认无安全提示,与 ai/maintain 路径不一致【low / small】(usability)
- **为什么重要**:install 调 install() 立即 set_active,不走 verify 门禁也无 confirm(),而 Resolve/Maintain 都 gate 在 confirm。最显眼的 `aevum install` 反而最不设防,下载到激活之间无完整性/闭包检查。(与 P0-4 同源安全模型问题)
- **修复**:在便捷 install 路径 set_active 前跑 verify 门禁,或打印解析闭包摘要并要求确认(除非 `--yes`),与 AI 路径一致。
- **位置**:`crates/cli/src/main.rs:842-853`;`crates/cli/src/lib.rs:1003-1014`

### P1-24 `aevum ai` 硬编码 USTC 中国镜像,无视用户/地区【medium / small】(usability)
- **为什么重要**:推荐的日常入口 `aevum ai` 无条件传 MIRROR_USTC,中国境外用户得到慢/被阻下载且无从 AI 路径覆盖;config.toml 也无 mirror 设置。
- **修复**:从 config.toml `[source] mirror` 读默认,回退 `deb.debian.org`(CDN)而非地域镜像;贯穿 AI dispatch 与 install。
- **位置**:`crates/cli/src/main.rs:624-625`

### P1-25 包索引无陈旧处理,prep-index.sh 发的是冻结 PoC 快照【medium / medium】(usability)
- **为什么重要**:prep-index.sh 指向 PoC 目录里冻结的 Packages.gz 且"已存在就跳过"从不刷新;update 不记时间戳/版本,resolve/install 不查索引年龄。用户对几个月前的索引解析却无警告。
- **修复**:停止发/拒绝冻结快照;update 总拉新并盖 fetch 时间+dist+arch 戳;resolve/install 在索引超 N 天或 dist 不匹配时警告。
- **位置**:`scripts/prep-index.sh:9,23-28`;`crates/cli/src/main.rs:561-577`

### P1-26 install 无部分失败恢复,闭包中途出错即整体中止且留残留态【medium / medium】(usability)
- **为什么重要**:propose_generation 循环里任一 `?` 在 make_generation 前中止整个函数,部分失败不创建世代,用户一无所获;大闭包(README 宣传 250 包 weston、242 包 niri)单次瞬时失败=全有或全无重启,进度不透明。
- **修复**:加 per-package 进度(N/M)、瞬时错误继续并最后汇总失败、跳过已摄入包的重试路径;仅当某包重试后仍无法获取才整体失败。
- **位置**:`crates/cli/src/lib.rs:888-998,762-807`

---

## P2(锦上添花)

> low 级或与发布无关的完善项;有余力再做。

### 文档
- **缺 verify/activate 命令文档**【medium / small】:ADR-0003 安全门禁的两个核心命令不在命令表。`README.md:78-96` vs `main.rs:235-267`。
- **crate 数标题写 12 实为 13**【low / trivial】:`README.md:322` 改 "13 个"。
- **build/service/etc 等已实现命令未文档化**【low / small】:加"高级/引擎驱动命令"小节或显式注明内部命令。`main.rs:126-233`。

### 发布/治理
- **缺 CONTRIBUTING/CODE_OF_CONDUCT/SECURITY**【medium / small】:AI 层处理 API key + 任意包安装,应给安全披露渠道;CONTRIBUTING 记录 WSL-only 构建约束。
- **无非 Rust 用户安装路径**【medium / medium】:仅源码编译;CI 就绪后加 release workflow 产 Linux 二进制并 cargo install 文档。`README.md:99-128`。
- **无 crates.io 可发布元数据 / 无 publish=false**【medium / small】:13 crate 都缺 description;决定发布策略,prototype 阶段建议 workspace 级 `publish = false`。
- **README 引用不存在的 README.zh-CN.md**【low / small】:创建或移除 `README.md:13` 的语言切换链。

### 代码质量
- **DeepSeekResolver::from_env().unwrap() 在独立 is_none() 检查后(双读+unwrap)**【low / trivial】:`match` 一次构造进 Option。`main.rs:403,1229`。
- **resolve_package hash 提取靠原始字节索引**【low / trivial】:用 `split_once('-')`/`splitn` 替固定字节偏移。`cache.rs:182-186`。
- **#[cfg(unix)]/not(unix) 命令 handler stub 重复**【low / small】:整体 gate 一个 unix 模块替 per-function twin(已知平台权衡,低优)。`main.rs:1085-1394`。
- **stringly-typed intent/plan 字段 + 静默 fallback**【low / medium】:intent/plan 建模为带 Unknown 变体的枚举;parse_repair_response 别默认到 'A'。`ai_client.rs:484-511,657-681`。

### 测试
- **Generation set_active 错误路径与 private-objects GC 可达性缺单测**【low / trivial】:跨平台纯计算路径,写 private-objects.txt 断言被 compute_garbage 保留;set_active(unknown) 返 NotFound。`generation/src/lib.rs:124-128,216-236,355-382`。

### 健壮性/可移植性
- **架构/dist 硬编码 amd64+单 dist**【low / medium】:loader basename 与 multiarch triplet 从实际 ELF 的 PT_INTERP 推导,别硬编码 `ld-linux-x86-64.so.2`/`x86_64-linux-gnu`;或显式限定 amd64 并文档化。`main.rs:85-89,334`;`lib.rs:973-983,1222`。
- **无磁盘满/部分写守卫;.deb 全文件读进内存**【low / small】:store 对象写临时路径再 rename;大 deb 流式哈希替 read-to-Vec。`lib.rs:809-814`;`store/src/lib.rs:101-116`。

### ADR
- **CVE 判据(verify 4①)未实现**【low / large】:ADR-0005 第 5 点的 CVE 半边目前零强制(诚实标注为 TODO);接入 CVE 源(哪怕静态离线 allow/deny 名单)让 needs_user_confirm 在 CVE 命中时也强制 true。`maintainer/src/lib.rs:248,18-21`。

---

## 接下来最该做的 3 件事

1. **堵死 Nix 供应链(P0-1 + P0-2)**:校验 narinfo StorePath 防路径穿越 + 加 NAR 哈希/签名校验。这两条合起来把"任意不可信镜像 → 任意文件写入 + 任意内容激活"这个最严重的发布阻断关掉,且与 Debian 路径已有的 SHA256 校验对齐。

2. **修复核心安全模型与正确性(P0-4 + P0-5 + P0-6)**:让 `aevum ai install` 走 verify 门禁,并让 list/remove 读 active 世代、按顶层意图 remove。这三条直接关系到项目对外的两大承诺——"AI 在笼子里"和"回滚回到已知状态"——目前都被打穿。

3. **建 CI 并修复 clean-clone 构建(P1-1 + P1-2)**:加 ubuntu CI 跑 test/clippy/fmt + gated e2e,并解决 offline vendor 让 fresh clone 能编译。否则所有 `#[cfg(unix)]`/`#[ignore]` 安全测试在 Windows 开发机上永不执行,setuid/原子切换/dlopen 闭包的回归会绿色落地——上面所有修复也无法被自动验证。
