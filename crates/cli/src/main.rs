//! `aevum` 命令行入口(骨架)。
//!
//! 串起四个核心 crate,先把命令骨架立起来,各子命令的完整流程标 TODO。
//! 第一个里程碑(见 docs/guides/01-rust-implementation-kickoff.md §3):
//! `closure-builder → store → generation` 串成"装一个 rg 并回滚"的最小闭环。

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aevum",
    version,
    about = "AI-native、可复现、原子化的 Linux 用户态系统层 / 包管理器"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 求解意图为确定性闭包并产出 lock(见 solver/intent crate、ADR-0003/0005)。
    Resolve {
        /// 顶层包名(可多个,显式约束,无 AI)。与 --intent 二选一。
        packages: Vec<String>,
        /// 自然语言意图(走 AI 增强层翻译成约束,DeepSeek 或离线 Mock)。
        #[arg(long)]
        intent: Option<String>,
        /// TS 配置前端(ADR-0004):`aevum.config.ts`,沙箱求值产约束。与包名/--intent 三选一。
        #[arg(long)]
        config: Option<String>,
        /// TS 配置的显式输入(JSON 对象字符串,如 `{"role":"developer"}`),记录进 lock 供复现。
        #[arg(long)]
        inputs: Option<String>,
        /// 无 DeepSeek key 时用离线 Mock 翻译(确定性规则,演示/离线降级)。
        #[arg(long)]
        mock: bool,
        /// 跳过交互确认,直接产 lock(自动化/CI 用)。默认会摊开约束等你确认。
        #[arg(long)]
        yes: bool,
        /// lock 文件名(默认取第一个包名或 "intent")。
        #[arg(long)]
        name: Option<String>,
        /// 自动应用 repair 方案A:对有单一共存版本的冲突包放宽求解(默认只检测+建议)。
        #[arg(long)]
        repair: bool,
    },
    /// 配置漂移检测:用 lock 记录的 inputs 重跑源 TS 配置,比对 closure_id 是否仍一致(审计/CI)。
    AuditConfig {
        /// 源 TS 配置路径(aevum.config.ts)。
        config: String,
        /// 被审计的 lock 名(读 locks/<name>.lock 的 closure_id 与 ts_inputs)。
        #[arg(long)]
        against: String,
        /// 覆盖 lock 记录的 inputs(对比"换输入会怎样");默认用 lock 记录值。
        #[arg(long)]
        inputs: Option<String>,
    },
    /// 从 Nix binary cache(镜像)拉取包及其传递依赖到 /nix/store(Nix 包源集成)。
    NixFetch {
        /// 包的 store hash(32 字符,如 `f4y36sn7m173qvdija8a1p6v81py66ns`)。
        /// 或用 --resolve 从包名查找。
        hash: Option<String>,
        /// 从 store-paths 按包名查找 hash(需要 --channel)。
        #[arg(long)]
        resolve: Option<String>,
        /// Nix binary cache 镜像 URL。
        #[arg(long, default_value = "https://mirrors.ustc.edu.cn/nix-channels/store")]
        mirror: String,
        /// Nix channel URL(用于 --resolve 查包名,如 nixpkgs-unstable)。
        #[arg(long, default_value = "https://mirrors.ustc.edu.cn/nix-channels/nixpkgs-unstable")]
        channel: String,
        /// 目标 store 目录(默认 /nix/store)。
        #[arg(long, default_value = "/nix/store")]
        store_dir: String,
        /// 拉取后将根包的 bin/ 链到 profile/bin(可直接通过 PATH 使用)。
        #[arg(long)]
        activate: bool,
    },
    /// 初始化 Aevum root:建目录骨架 + 写引导 env.sh,可选随即拉取索引。
    Init {
        /// 初始化后顺带跑一次 `update` 拉 Debian 索引。
        #[arg(long)]
        update: bool,
        /// update 用的镜像(仅 --update 时)。
        #[arg(long, default_value = "http://mirrors.ustc.edu.cn/debian")]
        mirror: String,
    },
    /// 更新 Debian 包索引(重新下载 Packages 文件)。
    Update {
        /// 镜像 URL。
        #[arg(long, default_value = "http://mirrors.ustc.edu.cn/debian")]
        mirror: String,
        /// 架构(默认 amd64)。
        #[arg(long, default_value = "amd64")]
        arch: String,
        /// 发行版(默认 trixie/main)。
        #[arg(long, default_value = "trixie")]
        dist: String,
    },
    /// AI 统一入口:自然语言对话,自动判断意图(装包/解释/搜索/清理...),支持多轮历史。
    Ai {
        /// 你想说的话(自然语言)。省略 + --reset 时只清历史。
        message: Vec<String>,
        /// 清空对话历史,开新话题。
        #[arg(long)]
        reset: bool,
        /// 跳过有副作用动作(装包/卸载/清理)的确认。
        #[arg(long)]
        yes: bool,
    },
    /// AI 解释错误或给出建议(用配置的 AI 模型分析问题并给出人话解释)。
    Explain {
        /// 要解释的内容(错误信息、包名、概念等)。
        message: String,
    },
    /// 搜索可安装的包(grep Debian 索引)。
    Search {
        /// 搜索关键词(包名或描述中的子串)。
        keyword: String,
        /// 最多显示多少条结果。
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 列出当前活跃世代安装的包。
    List,
    /// 从当前世代中移除包(建一个不含该包的新世代)。
    Remove {
        /// 要移除的包名(一个或多个)。
        packages: Vec<String>,
        /// Debian 镜像。
        #[arg(long, default_value = "http://mirrors.ustc.edu.cn/debian")]
        mirror: String,
    },
    /// 把一个包补全为完整运行闭包(四源合一,见 closure-builder / PoC-5)。
    Build {
        /// 包名(须已解包到 $AEVUM_ROOT/unpacked/<package>)。
        package: String,
        /// 主二进制相对包根的路径(默认 usr/bin/<package>)。
        #[arg(long)]
        bin: Option<String>,
        /// 运行时目录(相对包根,可多次/逗号分隔):复杂包的标准库/插件目录,整体纳入闭包。
        /// 例: --runtime-dir usr/lib/python3.14 (PoC-5)
        #[arg(long = "runtime-dir", value_delimiter = ',')]
        runtime_dir: Vec<String>,
    },
    /// 切换 active 世代(原子 symlink+rename,见 generation / PoC-7)。
    Switch {
        /// 目标世代 id。
        generation: u64,
    },
    /// 回滚到指定世代(指针回指,不重建)。
    Rollback {
        /// 目标世代 id。
        generation: u64,
    },
    /// 导出一个包的运行闭包为自包含 rootfs 目录(供全裸容器运行,里程碑8)。
    ExportRootfs {
        /// 包名(须已 install/解包到 $AEVUM_ROOT/unpacked/<pkg>)。
        package: String,
        /// 主二进制相对包根路径(默认 usr/bin/<package>)。
        #[arg(long)]
        bin: Option<String>,
        /// 输出目录(默认 $AEVUM_ROOT/rootfs-<package>)。
        #[arg(long)]
        out: Option<String>,
    },
    /// 把多个已存世代引用的包合并组成一个新世代(引擎驱动,供可引导用)。
    ComposeGeneration {
        /// 源世代 id(逗号分隔,如 7,10)。
        #[arg(long = "from", value_delimiter = ',', required = true)]
        from: Vec<u64>,
        /// 新世代 id。
        #[arg(long = "into")]
        into: u64,
    },
    /// 从一个真实世代导出可作系统根的 bootroot(ADR-0006 阶段2,引擎驱动)。
    ExportBootroot {
        /// 世代 id。
        generation: u64,
        /// 输出目录(默认 $AEVUM_ROOT/bootroot-<gen>)。
        #[arg(long)]
        out: Option<String>,
    },
    /// 导出可运行系统 rootfs(nspawn/chroot 可直接进入的完整世代)。路线3:证明 Aevum 管理的用户态真能运行。
    ExportSystem {
        /// 世代 id(须已 install/maintain 建好)。
        generation: u64,
        /// 输出目录(默认 $AEVUM_ROOT/system-<gen>)。
        #[arg(long)]
        out: Option<String>,
    },
    /// 真装:resolve(可含 AI+确认)→ 下载 .deb(SHA256 校验)→ 解包入 store → 造世代。
    Install {
        /// 顶层包名(可多个)。从真实 Debian 索引求解依赖闭包后下载。
        packages: Vec<String>,
        /// 只真下载安装这些包(默认全装,大闭包建议指定;避免过重)。
        #[arg(long = "only", value_delimiter = ',')]
        only: Vec<String>,
        /// Debian 镜像(默认 deb.debian.org;国内慢可换 mirrors.ustc.edu.cn/debian)。
        #[arg(long)]
        mirror: Option<String>,
        /// 目标世代 id(默认 0 = 自动选下一个)。
        #[arg(long, default_value_t = 0)]
        generation: u64,
    },
    /// 可达性 GC:回收无世代引用的 store 对象(共享依赖不误删,PoC-7)。
    Gc {
        /// 保留这些世代 id 引用的对象(可多次);未列出世代的独占对象将被回收。
        #[arg(long = "keep", value_delimiter = ',')]
        keep: Vec<u64>,
    },
    /// 渲染 bootloader 多世代菜单(syslinux.cfg,ADR-0006 阶段3,引擎驱动)。
    /// 第一个世代作 DEFAULT(active)。build-bootimage.sh 调它,switch/rollback 改它的 DEFAULT。
    BootMenu {
        /// 入菜单的世代 id(逗号分隔,如 50,51)。第一个作 DEFAULT。
        #[arg(long = "gens", value_delimiter = ',', required = true)]
        gens: Vec<u64>,
        /// 共享内核文件名(FAT 根内,默认 vmlinuz)。
        #[arg(long, default_value = "vmlinuz")]
        kernel: String,
        /// 自动引导倒计时(1/10 秒,syslinux TIMEOUT,默认 30 = 3 秒)。
        #[arg(long, default_value_t = 30)]
        timeout: u32,
        /// 内核命令行 APPEND(默认串口控制台 + rdinit)。
        #[arg(long, default_value = "console=ttyS0 rdinit=/init panic=1")]
        append: String,
        /// 输出 syslinux.cfg 路径(默认 $AEVUM_ROOT/boot3-build/stage/syslinux.cfg)。
        #[arg(long)]
        out: Option<String>,
    },
    /// 服务编译(ADR-0006 阶段4b):纯数据 TOML 服务声明 → s6 scandir 服务目录。
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// 系统配置(ADR-0006 阶段4c):纯数据 TOML → /etc 基底文件树(不可变,随世代走)。
    Etc {
        #[command(subcommand)]
        action: EtcAction,
    },
    /// AI maintainer 安全闸门(ADR-0003,C 主线):独立机器判定一个候选世代能否激活。
    /// 五判据:完整性/闭合性/层约束(待办)/版本回退/CVE(待办)。版本回退强制人工确认。
    Verify {
        /// 候选世代 id(其 lock.txt 提供 store 对象做完整性校验)。
        #[arg(long = "gen")]
        generation: u64,
        /// 候选 lock 名(locks/<name>.lock,提供版本语义做闭合性/回退;默认取世代号无从得知,需显式)。
        #[arg(long = "lock")]
        lock: String,
        /// 当前 active 的 lock 名,用于版本回退比较(省略则跳过回退判据,适合首装)。
        #[arg(long = "active-lock")]
        active_lock: Option<String>,
        /// foundation manifest TOML 路径(启用判据3:required 在场+版本精确;并入闭合提供集)。
        #[arg(long = "foundation")]
        foundation: Option<String>,
    },
    /// verify 门禁激活(C 主线,ADR-0003 安全模型闭合):先 verify,通过才原子切 active。
    /// 与 `switch`(机械切换,不校验)并列的安全路径——它永不绕过 verify。
    Activate {
        /// 候选世代 id。
        #[arg(long = "gen")]
        generation: u64,
        /// 候选 lock 名(locks/<name>.lock)。
        #[arg(long = "lock")]
        lock: String,
        /// 当前 active 的 lock 名(版本回退比较;省略则跳过回退判据)。
        #[arg(long = "active-lock")]
        active_lock: Option<String>,
        /// foundation manifest TOML 路径(启用判据3)。
        #[arg(long = "foundation")]
        foundation: Option<String>,
        /// 人类确认放行安全判据(版本回退)。**不能**放行硬性失败(完整性/闭合/层)。
        #[arg(long)]
        confirm: bool,
    },
    /// AI maintainer 端到端主循环(C 主线总成,见 ai/01):
    /// 求解 → propose 候选世代 → verify 门禁 → 激活,一条命令跑通。
    Maintain {
        /// 顶层包名(显式约束,确定性求解,无 AI 选 hash)。与 --intent/--config 三选一。
        packages: Vec<String>,
        /// 自然语言意图(走 AI 增强层翻译成约束,DeepSeek 或离线 Mock)。与包名/--config 三选一。
        #[arg(long)]
        intent: Option<String>,
        /// TS 配置前端(ADR-0004):沙箱求值+模板展开产约束,走完主循环。与包名/--intent 三选一。
        #[arg(long)]
        config: Option<String>,
        /// TS 配置的显式输入(JSON,记录进 lock;仅与 --config 搭配)。
        #[arg(long)]
        inputs: Option<String>,
        /// 无 DeepSeek key 时用离线 Mock 翻译(确定性规则,演示/离线降级)。
        #[arg(long)]
        mock: bool,
        /// 跳过意图约束的交互确认(自动化/CI 用)。
        #[arg(long)]
        yes: bool,
        /// 候选世代 id。
        #[arg(long = "gen")]
        generation: u64,
        /// Debian 镜像地址(下载 .deb)。
        #[arg(long)]
        mirror: String,
        /// lock 文件名(默认 "maintain")。
        #[arg(long, default_value = "maintain")]
        lock: String,
        /// 当前 active 的 lock 名(版本回退比较;省略则跳过)。
        #[arg(long = "active-lock")]
        active_lock: Option<String>,
        /// foundation manifest TOML 路径(启用判据3)。
        #[arg(long = "foundation")]
        foundation: Option<String>,
        /// 自动应用 repair 方案A:求解阶段对有单一共存版本的冲突包放宽(默认只检测+建议)。
        #[arg(long)]
        repair: bool,
        /// 人类确认放行安全判据(版本回退)。
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand)]
enum EtcAction {
    /// 把 TOML 系统配置编译成 /etc 基底文件树(运行时作 overlay 只读 lower)。
    Build {
        /// TOML 系统配置文件(可多个,后者覆盖前者同名文件)。
        files: Vec<String>,
        /// 输出 /etc 基底目录。
        #[arg(long)]
        out: String,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    /// 把 TOML 服务声明编译进 scandir(每个声明 → <scandir>/<name>/{run,type,dependencies})。
    Compile {
        /// TOML 服务声明文件(可多个)。
        files: Vec<String>,
        /// 输出 scandir 目录(每服务一个子目录)。
        #[arg(long)]
        scandir: String,
        /// run 脚本里的 LD_LIBRARY_PATH(世代自带库路径)。
        #[arg(long, default_value = "/usr/lib/x86_64-linux-gnu:/usr/lib")]
        lib_path: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let layout = aevum_cli::Layout::from_env();

    // P1-5 并发锁:变更类命令开头取 $AEVUM_ROOT 排他锁,持有到命令结束(_lock 生命周期覆盖整个 match)。
    // 只读命令(list/search/explain/resolve/audit-config/build/export 等)不取锁,可并发。
    let _lock = if is_mutating(&cli.command) {
        Some(aevum_cli::FsLock::acquire(&layout).map_err(|e| anyhow::anyhow!("{e}"))?)
    } else {
        None
    };

    match cli.command {
        Command::Resolve { packages, intent, config, inputs, mock, yes, name, repair } => {
            if let Some(config_path) = config {
                // TS 配置前端(ADR-0004):沙箱求值 → (模板展开 + 直接包)合并约束 → 同一套 resolve/lock。
                let lock_name = name.unwrap_or_else(|| "config".to_string());
                let ts_source = std::fs::read_to_string(&config_path)
                    .map_err(|e| anyhow::anyhow!("读 TS 配置失败 {config_path}: {e}"))?;
                let outcome = aevum_config_ts::eval_to_outcome(&ts_source, inputs.as_deref())
                    .map_err(|e| anyhow::anyhow!("TS 配置求值失败: {e}"))?;
                let templates_selected = outcome.templates.clone();

                // 共享逻辑:沙箱求值 → 模板展开 + 合并 → (约束, 模板记录)。
                let (constraints, templates_record) =
                    aevum_cli::ts_config_to_constraints(&layout, &ts_source, inputs.as_deref())
                        .map_err(|e| anyhow::anyhow!("{e}"))?;

                // 摊开最终约束(让用户看清 TS 配置 + 模板展开后到底意味着什么)。
                println!("TS 配置: {config_path}");
                if let Some(ref inp) = inputs {
                    println!("  显式输入: {inp}");
                }
                if !templates_selected.is_empty() {
                    println!("  选用模板: {templates_selected:?}");
                }
                println!("求值+模板展开产出 {} 条约束:", constraints.len());
                for c in &constraints {
                    let ver = match (&c.op, &c.ver) {
                        (Some(_), Some(v)) => format!(" (= {v})"),
                        _ => String::new(),
                    };
                    println!("    - {}{}", c.name, ver);
                }
                if !yes && !confirm("以上约束是否求解并产 lock?") {
                    println!("已取消。");
                    return Ok(());
                }
                let lock = aevum_cli::resolve_constraints_opt(&layout, &constraints, &lock_name, None, repair, inputs.as_deref(), templates_record.as_deref())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!(
                    "[resolve] (TS) → closure_id={} ({} 包, 未解析 {})",
                    lock.closure_id,
                    lock.package_count,
                    lock.diagnostics.unresolved.len()
                );
                if !lock.diagnostics.unresolved.is_empty() {
                    let names: Vec<&str> = lock.diagnostics.unresolved.iter().map(|u| u.name.as_str()).take(10).collect();
                    println!("  ⚠ 未解析(前10): {names:?}");
                }
                warn_conflicts(&lock);
                ai_assist_conflicts(&layout, &lock);
            } else if let Some(intent_text) = intent {
                // AI 增强路径,人在回路(ADR-0003 边界3):翻译 → 摊开约束 → 确认 → 求解。
                let lock_name = name.unwrap_or_else(|| "intent".to_string());
                let intent_obj = aevum_intent::Intent::NaturalLanguage(intent_text.clone());
                let use_mock = mock || aevum_intent::DeepSeekResolver::from_env().is_none();

                // 第一步:仅翻译(不求解、不写 lock)。
                use aevum_intent::IntentResolver;
                let outcome = if use_mock {
                    aevum_intent::MockIntentResolver::with_defaults().resolve_intent(&intent_obj)
                } else {
                    aevum_intent::DeepSeekResolver::from_env().unwrap().resolve_intent(&intent_obj)
                }
                .map_err(|e| anyhow::anyhow!("{e}"))?;

                // 第二步:摊开 AI 翻译出的约束,让用户看清"AI 替我选了什么"。
                println!("意图: \"{intent_text}\"");
                println!(
                    "AI({}/{}) 翻译出 {} 条约束:",
                    if outcome.assist.ai_involved { "介入" } else { "未介入" },
                    outcome.assist.model_id,
                    outcome.constraints.len()
                );
                for c in &outcome.constraints {
                    let ver = match (&c.op, &c.ver) {
                        (Some(_), Some(v)) => format!(" (>= {v})"),
                        _ => String::new(),
                    };
                    println!("    - {}{}", c.name, ver);
                }

                // 第三步:交互确认(--yes 跳过,自动化用)。
                if !yes && !confirm("以上约束是否求解并产 lock?") {
                    println!("已取消。可改措辞重新表达意图,或用 `aevum resolve <包名...>` 显式指定。");
                    return Ok(());
                }

                // 第四步:确认后才求解写 lock。
                let lock = aevum_cli::resolve_constraints_opt(
                    &layout,
                    &outcome.constraints,
                    &lock_name,
                    Some(&outcome.assist),
                    repair,
                    None,
                    None,
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!(
                    "[resolve] → closure_id={} ({} 包, 未解析 {})",
                    lock.closure_id,
                    lock.package_count,
                    lock.diagnostics.unresolved.len()
                );
                if !lock.diagnostics.unresolved.is_empty() {
                    let names: Vec<&str> = lock
                        .diagnostics
                        .unresolved
                        .iter()
                        .map(|u| u.name.as_str())
                        .take(10)
                        .collect();
                    println!("  ⚠ 未解析(前10): {names:?}");
                }
                warn_conflicts(&lock);
                ai_assist_conflicts(&layout, &lock);
            } else {
                // 显式包名路径(里程碑5,无 AI)。
                if packages.is_empty() {
                    return Err(anyhow::anyhow!("需提供包名或 --intent"));
                }
                let lock_name = name.unwrap_or_else(|| packages[0].clone());
                let constraints: Vec<aevum_solver::Constraint> =
                    packages.iter().map(|p| aevum_solver::Constraint::unconstrained(p.as_str())).collect();
                let lock = aevum_cli::resolve_constraints_opt(&layout, &constraints, &lock_name, None, repair, None, None)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!(
                    "[resolve] {:?} → closure_id={} ({} 包, 未解析 {})",
                    packages,
                    lock.closure_id,
                    lock.package_count,
                    lock.diagnostics.unresolved.len()
                );
                if !lock.diagnostics.unresolved.is_empty() {
                    let names: Vec<&str> = lock
                        .diagnostics
                        .unresolved
                        .iter()
                        .map(|u| u.name.as_str())
                        .take(10)
                        .collect();
                    println!("  未解析(前10): {names:?}");
                }
                warn_conflicts(&lock);
                ai_assist_conflicts(&layout, &lock);
            }
        }
        Command::AuditConfig { config, against, inputs } => {
            let ts_source = std::fs::read_to_string(&config)
                .map_err(|e| anyhow::anyhow!("读 TS 配置失败 {config}: {e}"))?;
            let report = aevum_cli::audit_config(&layout, &ts_source, &against, inputs.as_deref())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("审计 {config} ←→ lock {against}");
            if let Some(ref inp) = report.used_inputs {
                println!("  重放输入: {inp}");
            }
            println!("{report}");
            if report.drifted {
                // CI 友好:漂移时非零退出码。
                std::process::exit(1);
            }
        }
        Command::NixFetch { hash, resolve, mirror, channel, store_dir, activate } => {
            let store_path = std::path::PathBuf::from(&store_dir);
            let target_hash = if let Some(name) = resolve {
                println!("[nix-fetch] 从 channel 查找包 '{name}'...");
                let h = aevum_nix_source::cache::NixCacheClient::resolve_package(&channel, &name)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("  找到: {h}-{name}*");
                h
            } else if let Some(h) = hash {
                h
            } else {
                return Err(anyhow::anyhow!("需要 <hash> 或 --resolve <name>"));
            };

            println!("[nix-fetch] 从 {mirror} 递归拉取 {target_hash} 及依赖...");
            let client = aevum_nix_source::NixCacheClient::new(&mirror, &store_path);
            let results = client.fetch_closure(&target_hash)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("  ✓ {} 个包已拉取到 {}", results.len(), store_dir);
            for info in results.iter().take(10) {
                println!("    - {} ({})", info.name(), info.store_path);
            }
            if results.len() > 10 {
                println!("    ... 及其余 {} 个依赖", results.len() - 10);
            }

            // --activate:把根包的 bin/ 下可执行文件链到 profile/bin
            let root_info = results.iter().find(|i| i.hash() == target_hash);
            if let Some(ri) = root_info {
                println!("\n  运行: {}/bin/<binary>", ri.store_path);

                if activate {
                    let ref_name = ri.store_path.strip_prefix("/nix/store/").unwrap_or(&ri.store_path);
                    let bin_dir = store_path.join(ref_name).join("bin");
                    let profile_bin = layout.profile_bin_dir();
                    std::fs::create_dir_all(&profile_bin)?;

                    if bin_dir.is_dir() {
                        let mut linked = 0;
                        for entry in std::fs::read_dir(&bin_dir)? {
                            let entry = entry?;
                            let name = entry.file_name();
                            let link = profile_bin.join(&name);
                            let target = entry.path();
                            let _ = std::fs::remove_file(&link);
                            #[cfg(unix)]
                            std::os::unix::fs::symlink(&target, &link)?;
                            linked += 1;
                        }
                        println!("  [activate] profile/bin: {linked} 个可执行文件已链接");
                        println!("  确保 PATH 含: {}", profile_bin.display());
                    } else {
                        println!("  ⚠ 根包无 bin/ 目录,跳过 activate");
                    }
                }
            }
        }
        Command::Init { update, mirror } => {
            aevum_cli::init_layout(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("[init] ✓ 已初始化 {}", layout.root.display());
            println!("  profile/bin、profile/lib、locks/、generations/ 已建");
            let env_sh = layout.root.join("profile").join("env.sh");
            println!("  把这行加进 ~/.bashrc:  source {}", env_sh.display());
            if update {
                println!("[init] 顺带拉取 Debian 索引...");
                // 复用 Update 逻辑:直接递归调用同一处理需要重组,简单起见提示用户或内联。
                let url = format!("{}/dists/trixie/main/binary-amd64/Packages.gz", mirror);
                let index_path = layout.index_file();
                std::fs::create_dir_all(index_path.parent().unwrap())?;
                let tmp = index_path.parent().unwrap().join(format!("Packages.tmp.{}", std::process::id()));
                let f = std::fs::File::create(&tmp)?;
                let mut curl = std::process::Command::new("curl")
                    .args(["-sL", "--fail", &url]).stdout(std::process::Stdio::piped()).spawn()
                    .map_err(|e| anyhow::anyhow!("curl spawn 失败: {e}"))?;
                let out = curl.stdout.take().unwrap();
                let gz = std::process::Command::new("gunzip").stdin(out).stdout(f).status()
                    .map_err(|e| anyhow::anyhow!("gunzip 失败: {e}"))?;
                let cu = curl.wait()?;
                if cu.success() && gz.success() && std::fs::metadata(&tmp).map(|m| m.len() > 0).unwrap_or(false) {
                    std::fs::rename(&tmp, &index_path)?;
                    println!("  ✓ 索引已拉取: {}", index_path.display());
                } else {
                    let _ = std::fs::remove_file(&tmp);
                    println!("  ⚠ 索引拉取失败(可稍后单独跑 `aevum update`),init 其余已完成");
                }
            } else {
                println!("  下一步: aevum update  然后  aevum install <pkg>");
            }
        }
        Command::Update { mirror, arch, dist } => {
            // 下载 Packages.gz → 解压 → 写到 $AEVUM_ROOT/index/Packages
            let url = format!("{}/dists/{}/main/binary-{}/Packages.gz", mirror, dist, arch);
            println!("[update] 从 {url} 更新索引...");
            let index_path = layout.index_file();
            let index_dir = index_path.parent().unwrap().to_path_buf();
            std::fs::create_dir_all(&index_dir)?;

            // 安全/健壮(P1-3):不经 `sh -c "curl | gunzip > index"`。
            // 旧实现两个坑:(1) 管道退出码只看 gunzip,curl 失败(404/断网)被吞;
            // (2) `>` 先把正式索引截断成空文件,再发现下载失败 → 索引已毁。
            // 这里:argv 形式 spawn curl(--fail)| gunzip → 写**临时文件**,
            // 两端都成功且产出非空才原子 rename 覆盖正式索引;任一步失败保留旧索引。
            let tmp_path = index_dir.join(format!("Packages.tmp.{}", std::process::id()));
            let tmp = std::fs::File::create(&tmp_path)
                .map_err(|e| anyhow::anyhow!("建临时索引失败: {e}"))?;

            let mut curl = std::process::Command::new("curl")
                .args(["-sL", "--fail", &url])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| anyhow::anyhow!("curl spawn 失败: {e}"))?;
            let curl_out = curl.stdout.take().unwrap();
            let gunzip_status = std::process::Command::new("gunzip")
                .stdin(curl_out)
                .stdout(tmp)
                .status()
                .map_err(|e| anyhow::anyhow!("gunzip 执行失败: {e}"))?;
            let curl_status = curl.wait().map_err(|e| anyhow::anyhow!("curl wait 失败: {e}"))?;

            let fail = |reason: String| -> anyhow::Error {
                let _ = std::fs::remove_file(&tmp_path); // 失败不留半截临时文件
                anyhow::anyhow!("更新索引失败({reason});旧索引保留未动")
            };
            if !curl_status.success() {
                return Err(fail(format!("curl 退出码 {curl_status}(镜像不可达/404?)")));
            }
            if !gunzip_status.success() {
                return Err(fail(format!("gunzip 退出码 {gunzip_status}(响应非 gzip?)")));
            }
            let tmp_size = std::fs::metadata(&tmp_path).map(|m| m.len()).unwrap_or(0);
            if tmp_size == 0 {
                return Err(fail("解压后索引为空".into()));
            }
            // 原子替换:同目录 rename(POSIX 原子),旧索引到此刻才被覆盖。
            std::fs::rename(&tmp_path, &index_path)
                .map_err(|e| anyhow::anyhow!("替换索引失败: {e}(临时文件 {})", tmp_path.display()))?;
            println!("  ✓ 索引已更新: {} ({:.1} MB)", index_path.display(), tmp_size as f64 / 1_048_576.0);
        }
        Command::Ai { message, reset, yes } => {
            use aevum_intent::ai_client::{AiConfig, ChatHistory};
            let hist_path = layout.root.join("ai-history.txt");

            if reset {
                ChatHistory::reset(&hist_path);
                println!("已清空对话历史。");
                if message.is_empty() {
                    return Ok(());
                }
            }
            let user_input = message.join(" ");
            if user_input.trim().is_empty() {
                return Err(anyhow::anyhow!("请说点什么,如 `aevum ai \"我要个 python 环境\"`"));
            }

            let config_path = layout.root.join("config.toml");
            let ai_cfg = AiConfig::load(&config_path);
            if !ai_cfg.is_available() {
                return Err(anyhow::anyhow!(
                    "AI 不可用。配置:设 AEVUM_AI_KEY 环境变量,或编辑 {}/config.toml 的 [ai] 段(provider/api_key)。本地可用 ollama 无需 key。",
                    layout.root.display()
                ));
            }

            // 加载历史 + 判断意图
            let mut history = ChatHistory::load(&hist_path);
            let action = aevum_intent::ai_client::ai_dispatch(&ai_cfg, &history.messages, &user_input)
                .map_err(|e| anyhow::anyhow!("AI 调用失败: {e}"))?;

            // 回复用户
            println!("\n💬 {}\n", action.reply);

            // 记录这轮对话
            history.push("user", &user_input);
            history.push("assistant", &action.reply);
            history.save(&hist_path, 20);

            // 分发到动作
            match action.intent.as_str() {
                "install" if !action.packages.is_empty() => {
                    println!("→ 意图: 安装 {:?}", action.packages);
                    if !yes && !confirm("确认安装?") {
                        println!("已取消。");
                        return Ok(());
                    }
                    let gen_id = aevum_cli::next_generation_id(&layout);
                    // AI 选的包走 verify 门禁(ADR-0005:AI 不能自我放行,须独立复核)。
                    // --yes 同时作为门禁的 confirm:放行版本回退类安全判据(用户已知情拍板)。
                    do_install(&layout, &action.packages, &[], &aevum_cli::configured_mirror(&layout), gen_id, true, yes)?;
                }
                "remove" if !action.packages.is_empty() => {
                    println!("→ 意图: 移除 {:?}(用 `aevum remove` 执行)", action.packages);
                }
                "search" if !action.query.is_empty() => {
                    println!("→ 意图: 搜索 '{}'(用 `aevum search {}` 执行)", action.query, action.query);
                }
                "list" => println!("→ 意图: 列出已装包(用 `aevum list` 执行)"),
                "gc" => println!("→ 意图: 清理旧世代(用 `aevum gc --keep N` 执行)"),
                "explain" | "repair" | "chat" => { /* reply 已显示,无副作用 */ }
                _ => { /* 已显示 reply */ }
            }
        }
        Command::Explain { message } => {
            let config_path = layout.root.join("config.toml");
            let ai_cfg = aevum_intent::ai_client::AiConfig::load(&config_path);
            if !ai_cfg.is_available() {
                return Err(anyhow::anyhow!(
                    "AI 不可用。配置方法:\n  1. 设环境变量 AEVUM_AI_KEY=<your-key>\n  2. 或编辑 {}/config.toml 的 [ai] 段\n  \n  支持: deepseek / openai / claude / ollama(本地无需 key)",
                    layout.root.display()
                ));
            }
            println!("[explain] 使用 {} ({})...", ai_cfg.provider, ai_cfg.model);
            let system_prompt = "你是 Aevum 包管理器的 AI 助手。用户遇到了问题或有疑问。\
                请用简洁的中文解释问题原因,并给出具体的解决建议。\
                如果涉及包名,用真实 Debian/Nix 包名。格式:先一句话总结,再分点给建议。";
            match aevum_intent::ai_client::ai_chat(&ai_cfg, system_prompt, &message) {
                Ok(response) => println!("\n{response}"),
                Err(e) => println!("AI 调用失败: {e}"),
            }
        }
        Command::Search { keyword, limit } => {
            let index_path = layout.index_file();
            if !index_path.exists() {
                return Err(anyhow::anyhow!("索引不存在,先 `aevum update`"));
            }
            let text = std::fs::read_to_string(&index_path)?;
            let mut results = Vec::new();
            let mut cur_pkg = String::new();
            let mut cur_ver = String::new();
            let mut cur_desc = String::new();
            for line in text.lines() {
                if let Some(name) = line.strip_prefix("Package: ") {
                    if !cur_pkg.is_empty() && (cur_pkg.contains(&keyword) || cur_desc.to_lowercase().contains(&keyword.to_lowercase())) {
                        results.push((cur_pkg.clone(), cur_ver.clone(), cur_desc.clone()));
                    }
                    cur_pkg = name.to_string();
                    cur_ver.clear();
                    cur_desc.clear();
                } else if let Some(v) = line.strip_prefix("Version: ") {
                    cur_ver = v.to_string();
                } else if let Some(d) = line.strip_prefix("Description: ") {
                    cur_desc = d.to_string();
                }
            }
            // 最后一条
            if !cur_pkg.is_empty() && (cur_pkg.contains(&keyword) || cur_desc.to_lowercase().contains(&keyword.to_lowercase())) {
                results.push((cur_pkg, cur_ver, cur_desc));
            }
            if results.is_empty() {
                println!("未找到含 '{keyword}' 的包");
            } else {
                println!("找到 {} 个包(显示前 {limit}):", results.len());
                for (name, ver, desc) in results.iter().take(limit) {
                    println!("  {name} ({ver}) — {desc}");
                }
            }
        }
        Command::List => {
            // 列出**当前 active 世代**的顶层包(认 active 指针,不瞎猜最新 lock)。
            let gens = aevum_cli::open_generations(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            let Some(active_id) = gens.active_generation().map_err(|e| anyhow::anyhow!("{e}"))? else {
                return Err(anyhow::anyhow!("无活跃世代(先 aevum install 或 switch)"));
            };
            match aevum_cli::active_lock_name(&layout).map_err(|e| anyhow::anyhow!("{e}"))? {
                Some(lock_name) => {
                    let lock_path = layout.locks_dir().join(format!("{lock_name}.lock"));
                    let lock = aevum_cli::parse_lock_file(&lock_path)
                        .map_err(|e| anyhow::anyhow!("读 gen-{active_id} 的 lock '{lock_name}' 失败: {e}"))?;
                    println!("当前世代 gen-{active_id} ({}, {} 个包):", lock.closure_id, lock.package_count);
                    for p in &lock.locked {
                        println!("  {} @ {}", p.name, p.version);
                    }
                }
                None => println!("gen-{active_id}: 无对应 lock 文件"),
            }
        }
        Command::Remove { packages, mirror } => {
            if packages.is_empty() {
                return Err(anyhow::anyhow!("需指定要移除的包名"));
            }
            // 取**当前 active 世代**的 lock(认 active 指针,不瞎猜最新 lock):
            // rollback 后 active 是旧世代,最新 lock 可能是别的——remove 必须基于真实在用的包集。
            let lock_name = aevum_cli::active_lock_name(&layout)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .ok_or_else(|| anyhow::anyhow!("无活跃世代(先 aevum install),无法确定当前包集"))?;
            let lock_path = layout.locks_dir().join(format!("{lock_name}.lock"));
            let lock = aevum_cli::parse_lock_file(&lock_path)
                .map_err(|e| anyhow::anyhow!("读 active 世代的 lock '{lock_name}' 失败: {e}"))?;

            // 从 lock 中取顶层包名(去掉被 remove 的)
            let remaining: Vec<String> = lock.locked.iter()
                .map(|p| p.name.clone())
                .filter(|n| !packages.contains(n))
                .collect();

            let removed: Vec<&String> = packages.iter()
                .filter(|p| lock.locked.iter().any(|l| &l.name == *p))
                .collect();
            if removed.is_empty() {
                println!("指定的包都不在当前世代中: {:?}", packages);
                return Ok(());
            }
            println!("[remove] 移除: {:?}", removed);
            println!("  剩余 {} 个包,重建世代...", remaining.len());

            let gen_id = aevum_cli::next_generation_id(&layout);
            let lock_new = aevum_cli::resolve(&layout, &remaining, "removed")
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("  求解: {} 包", lock_new.package_count);

            let report = aevum_cli::install(&layout, &lock_new, &mirror, &[], gen_id)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Err(e) = aevum_cli::record_generation_lock(&layout, gen_id, "removed") {
                println!("  ⚠ 记录世代 lock 指针失败: {e}");
            }
            println!("  gen-{} 已激活({} store 对象)", report.generation, report.store_objects);
            match aevum_cli::refresh_profile(&layout) {
                Ok(n) => println!("  profile/bin: {n} 个可执行文件"),
                Err(e) => println!("  ⚠ profile: {e}"),
            }
        }
        Command::Build { package, bin, runtime_dir } => {
            let bin_rel = bin.unwrap_or_else(|| format!("usr/bin/{package}"));
            let runtime_dirs: Vec<std::path::PathBuf> =
                runtime_dir.iter().map(std::path::PathBuf::from).collect();
            let built = aevum_cli::build_with(
                &layout,
                &package,
                std::path::Path::new(&bin_rel),
                &runtime_dirs,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            let store = aevum_cli::open_store(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            let ingested =
                aevum_cli::ingest_closure(&store, &built).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "[build] {package}: 扫 {} ELF,闭包 {} 库 + {} loader,缺失 {} 个",
                built.scanned_elf_count,
                built.libs.len(),
                built.interpreter.is_some() as u8,
                built.missing.len()
            );
            if !built.missing.is_empty() {
                println!("  缺失: {}", built.missing.join(", "));
            }
            println!(
                "  入库 {} 个对象(含运行时 {} 个)",
                ingested.refs.len(),
                ingested.runtime_objs.len()
            );
        }
        Command::Switch { generation } => {
            let gens = aevum_cli::open_generations(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            gens.set_active(generation)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("[switch] active → gen-{generation}");
            sync_boot_default(&layout, generation);
            // 刷新 profile/bin(路线1:PATH 里的程序随世代切换而变)。
            match aevum_cli::refresh_profile(&layout) {
                Ok(n) => println!("  profile/bin: {n} 个可执行文件已就绪"),
                Err(e) => println!("  ⚠ profile 刷新失败(不阻塞): {e}"),
            }
        }
        Command::Rollback { generation } => {
            let gens = aevum_cli::open_generations(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            gens.rollback(generation)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("[rollback] active → gen-{generation}");
            sync_boot_default(&layout, generation);
            // 刷新 profile/bin(回滚后也要更新)。
            match aevum_cli::refresh_profile(&layout) {
                Ok(n) => println!("  profile/bin: {n} 个可执行文件已就绪"),
                Err(e) => println!("  ⚠ profile 刷新失败(不阻塞): {e}"),
            }
        }
        Command::Gc { keep } => {
            let gens = aevum_cli::open_generations(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            let store = aevum_cli::open_store(&layout).map_err(|e| anyhow::anyhow!("{e}"))?;
            let all = store.list_objects().map_err(|e| anyhow::anyhow!("{e}"))?;
            let keep_ids = if keep.is_empty() {
                // 默认保留当前 active 世代
                gens.active_generation()
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .into_iter()
                    .collect()
            } else {
                keep
            };
            let plan = gens
                .compute_garbage(&keep_ids, &all)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "[gc] 保留世代 {:?}: 回收 {} 个,保留 {} 个共享对象",
                keep_ids,
                plan.garbage.len(),
                plan.kept.len()
            );
        }
        Command::Install { packages, only, mirror, generation } => {
            if packages.is_empty() {
                return Err(anyhow::anyhow!("需提供包名,如 `aevum install hello`"));
            }
            let mirror = mirror.unwrap_or_else(|| aevum_cli::configured_mirror(&layout));
            let gen_id = if generation == 0 {
                aevum_cli::next_generation_id(&layout)
            } else {
                generation
            };
            do_install(&layout, &packages, &only, &mirror, gen_id, false, false)?;
        }
        Command::ExportRootfs { package, bin, out } => {
            let bin_rel = bin.unwrap_or_else(|| format!("usr/bin/{package}"));
            let out_dir = out
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| layout.root.join(format!("rootfs-{package}")));
            // 1. 补运行闭包(主二进制 + 库 + loader)。
            let built = aevum_cli::build_with(
                &layout,
                &package,
                std::path::Path::new(&bin_rel),
                &[],
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            if !built.missing.is_empty() {
                println!("  ⚠ 缺失库(全裸运行可能受影响): {:?}", built.missing);
            }
            // 2. 导出自包含 rootfs(复制实体文件 + loader 注入命令)。
            let export = aevum_cli::export_rootfs(&built, &out_dir)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "[export-rootfs] {} → {}",
                export.main_name,
                export.dir.display()
            );
            println!("  运行命令(rootfs 内): {}", export.run_argv.join(" "));
            println!("  全裸容器: FROM scratch + COPY 此目录 + CMD 上述命令");
        }
        Command::ComposeGeneration { from, into } => {
            let n = aevum_cli::compose_generation(&layout, &from, into)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("[compose-generation] 合并世代 {from:?} → gen-{into}({n} 个包)");
        }
        Command::ExportBootroot { generation, out } => {
            let out_dir = out
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| layout.root.join(format!("bootroot-{generation}")));
            let copied = aevum_cli::export_bootroot(&layout, generation, &out_dir)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "[export-bootroot] gen-{generation} → {}({copied} 个文件,引擎从世代 store 对象产出)",
                out_dir.display()
            );
            println!("  根标志: {}/AEVUM_GENERATION_ROOT", out_dir.display());
        }
        Command::ExportSystem { generation, out } => {
            let out_dir = out
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| layout.root.join(format!("system-{generation}")));
            let report = aevum_cli::export_system(&layout, generation, &out_dir)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "[export-system] gen-{generation} → {}({} 个文件)",
                report.dest.display(), report.file_count
            );
            if report.shell_found {
                println!("  ✓ /bin/sh 已就绪");
                println!("  运行: sudo systemd-nspawn -D {}", report.dest.display());
                println!("  或:   sudo chroot {} /bin/sh", report.dest.display());
            } else {
                println!("  ⚠ 未找到 shell(busybox/bash);chroot 需手动提供 /bin/sh");
            }
        }
        Command::BootMenu { gens, kernel, timeout, append, out } => {
            use aevum_generation::bootloader::{BootEntry, BootMenu};
            if gens.is_empty() {
                return Err(anyhow::anyhow!("需 --gens,如 --gens 50,51"));
            }
            let cfg = out
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| layout.boot_menu_cfg());
            if let Some(parent) = cfg.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let menu = BootMenu {
                default: gens[0],
                kernel,
                timeout,
                append,
                entries: gens
                    .iter()
                    .map(|&g| BootEntry { generation: g, initrd: format!("initrd-{g}.gz") })
                    .collect(),
            };
            menu.write_to(&cfg)?;
            println!(
                "[boot-menu] 渲染 {} 个世代 {:?},DEFAULT=gen-{} → {}",
                gens.len(),
                gens,
                gens[0],
                cfg.display()
            );
        }
        Command::Service { action } => match action {
            ServiceAction::Compile { files, scandir, lib_path } => {
                use aevum_service_compiler::{compile_service, Service};
                if files.is_empty() {
                    return Err(anyhow::anyhow!("需至少一个 TOML 服务声明文件"));
                }
                let scandir = std::path::PathBuf::from(&scandir);
                std::fs::create_dir_all(&scandir)?;
                for f in &files {
                    let text = std::fs::read_to_string(f)
                        .map_err(|e| anyhow::anyhow!("读 {f}: {e}"))?;
                    let svc = Service::parse(&text)
                        .map_err(|e| anyhow::anyhow!("解析 {f}: {e}"))?;
                    let c = compile_service(&svc, &lib_path);
                    let dir = scandir.join(&c.name);
                    std::fs::create_dir_all(&dir)?;
                    // run 须可执行(s6-supervise exec 它)。
                    let run = dir.join("run");
                    std::fs::write(&run, &c.run)?;
                    set_executable(&run)?;
                    std::fs::write(dir.join("type"), &c.type_file)?;
                    if let Some(deps) = &c.dependencies {
                        std::fs::write(dir.join("dependencies"), deps)?;
                    }
                    println!("[service] 编译 {} → {}/run (type={})", c.name, dir.display(), c.type_file.trim());
                }
                println!("[service] scandir: {}", scandir.display());
            }
        },
        Command::Etc { action } => match action {
            EtcAction::Build { files, out } => {
                use aevum_etc_builder::build_etc;
                if files.is_empty() {
                    return Err(anyhow::anyhow!("需至少一个 TOML 系统配置文件"));
                }
                // 多文件按序合并(后者覆盖同名 /etc 文件)。
                let mut merged: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
                for f in &files {
                    let text = std::fs::read_to_string(f)
                        .map_err(|e| anyhow::anyhow!("读 {f}: {e}"))?;
                    let base = build_etc(&text)
                        .map_err(|e| anyhow::anyhow!("编译 {f}: {e}"))?;
                    for ef in base.files {
                        merged.insert(ef.rel_path, ef.content);
                    }
                }
                let out = std::path::PathBuf::from(&out);
                std::fs::create_dir_all(&out)?;
                for (rel, content) in &merged {
                    let dst = out.join(rel);
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dst, content)?;
                    println!("[etc] 生成 /etc/{rel}");
                }
                println!("[etc] 基底: {} 个文件 → {}", merged.len(), out.display());
            }
        },
        Command::Verify { generation, lock, active_lock, foundation } => {
            let report = aevum_cli::verify_generation(
                &layout,
                &lock,
                generation,
                active_lock.as_deref(),
                foundation.as_deref().map(std::path::Path::new),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;

            println!("[verify] 候选 gen-{generation} (lock={lock})");
            // 判据1:完整性。
            if report.integrity_failures.is_empty() {
                println!("  ✓ 完整性: store 对象全部校验通过");
            } else {
                println!("  ✗ 完整性: {} 个对象失败", report.integrity_failures.len());
                for f in &report.integrity_failures {
                    println!("      {} — {}", f.object_id, f.reason);
                }
            }
            // 判据2:闭合性。
            if report.unclosed_deps.is_empty() {
                println!("  ✓ 闭合性: 闭包内无未满足依赖");
            } else {
                println!("  ✗ 闭合性: {} 条依赖未满足", report.unclosed_deps.len());
                for d in &report.unclosed_deps {
                    println!("      {} 需要 {}", d.package, d.requirement);
                }
            }
            // 判据4②:版本回退。
            if report.version_rollbacks.is_empty() {
                println!("  ✓ 版本回退: 无");
            } else {
                println!("  ⚠ 版本回退: {} 个包低于 active", report.version_rollbacks.len());
                for r in &report.version_rollbacks {
                    println!(
                        "      {}: {} < {}(active)",
                        r.package, r.candidate_version, r.active_version
                    );
                }
            }
            // 判据3/4①:诚实标注本轮未实现。
            println!("  · 层约束(判据3)、CVE(判据4①): 本轮未实现,见 maintainer crate 待办");

            // 结论:passed(硬性)与 needs_user_confirm(安全)两个独立维度。
            println!("---");
            println!(
                "  passed={}  needs_user_confirm={}  → {}",
                report.passed,
                report.needs_user_confirm,
                if report.auto_activatable() {
                    "可自动激活"
                } else if report.passed {
                    "校验通过但需人工确认(版本回退)"
                } else {
                    "校验未通过,不可激活"
                }
            );
            // 退出码:可自动激活 0;需确认 2;硬性失败 1(供脚本/CI 据此分流)。
            if !report.passed {
                std::process::exit(1);
            } else if report.needs_user_confirm {
                std::process::exit(2);
            }
        }
        Command::Activate { generation, lock, active_lock, foundation, confirm } => {
            activate_cmd(&layout, generation, &lock, active_lock.as_deref(), foundation.as_deref(), confirm)?;
        }
        Command::Maintain { packages, intent, config, inputs, mock, yes, generation, mirror, lock, active_lock, foundation, repair, confirm } => {
            maintain_cmd(
                &layout, &packages, intent.as_deref(), config.as_deref(), inputs.as_deref(),
                mock, yes, &mirror, &lock, generation,
                active_lock.as_deref(), foundation.as_deref(), repair, confirm,
            )?;
        }
    }
    Ok(())
}

/// 执行 verify 门禁激活子命令。unix 专有(set_active 是 symlink+rename)。
#[cfg(unix)]
fn activate_cmd(
    layout: &aevum_cli::Layout,
    generation: u64,
    lock: &str,
    active_lock: Option<&str>,
    foundation: Option<&str>,
    confirm: bool,
) -> anyhow::Result<()> {
    use aevum_cli::ActivateBlocked;
    let outcome = aevum_cli::activate_verified(
        layout,
        lock,
        generation,
        active_lock,
        foundation.map(std::path::Path::new),
        confirm,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let report = &outcome.report;

    println!("[activate] 门禁校验 gen-{generation} (lock={lock})");
    // 摊开判据(与 verify 一致,便于看清拒绝原因)。
    if !report.integrity_failures.is_empty() {
        println!("  ✗ 完整性: {} 个对象失败", report.integrity_failures.len());
        for f in &report.integrity_failures {
            println!("      {} — {}", f.object_id, f.reason);
        }
    }
    if !report.unclosed_deps.is_empty() {
        println!("  ✗ 闭合性: {} 条依赖未满足", report.unclosed_deps.len());
        for d in &report.unclosed_deps {
            println!("      {} 需要 {}", d.package, d.requirement);
        }
    }
    if !report.foundation_violations.is_empty() {
        println!("  ✗ 层约束(判据3): {} 项", report.foundation_violations.len());
        for v in &report.foundation_violations {
            println!("      {v}");
        }
    }
    if !report.version_rollbacks.is_empty() {
        println!("  ⚠ 版本回退: {} 个包低于 active", report.version_rollbacks.len());
        for r in &report.version_rollbacks {
            println!("      {}: {} < {}(active)", r.package, r.candidate_version, r.active_version);
        }
    }

    if outcome.activated {
        println!("  ✓ 通过门禁 → active 已切到 gen-{generation}{}",
            if confirm && report.needs_user_confirm { "(经人工确认放行版本回退)" } else { "" });
        sync_boot_default(layout, generation);
        return Ok(());
    }

    // 被拒:打印原因 + 据此定退出码(active 未动)。
    match outcome.blocked_reason {
        Some(ActivateBlocked::HardFail) => {
            eprintln!("  ✗ 拒绝激活: 硬性校验未通过(完整性/闭合),active 不动。confirm 也无法放行损坏世代。");
            std::process::exit(1);
        }
        Some(ActivateBlocked::NeedsConfirm) => {
            eprintln!("  ⚠ 拒绝激活: 触发版本回退,需人工确认。确认无误后加 --confirm 重试。active 不动。");
            std::process::exit(2);
        }
        None => unreachable!("未激活必有 blocked_reason"),
    }
}

#[cfg(not(unix))]
fn activate_cmd(
    _layout: &aevum_cli::Layout,
    _generation: u64,
    _lock: &str,
    _active_lock: Option<&str>,
    _foundation: Option<&str>,
    _confirm: bool,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("aevum activate 需要 unix(set_active 是 symlink+rename)。请在 Linux/WSL 运行"))
}

/// 执行 maintain 端到端主循环子命令。unix 专有。
#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn maintain_cmd(
    layout: &aevum_cli::Layout,
    packages: &[String],
    intent: Option<&str>,
    config: Option<&str>,
    inputs: Option<&str>,
    mock: bool,
    yes: bool,
    mirror: &str,
    lock: &str,
    generation: u64,
    active_lock: Option<&str>,
    foundation: Option<&str>,
    repair: bool,
    confirm: bool,
) -> anyhow::Result<()> {
    use aevum_cli::ActivateBlocked;

    // 入口分流:TS 配置 / 意图(AI 翻译)/ 显式包名 → 写 lock → 主循环后半段。
    let outcome = if let Some(config_path) = config {
        // TS 配置路径(ADR-0004):沙箱求值 + 模板展开 → 约束 → 求解写 lock → 主循环后半段。
        let ts_source = std::fs::read_to_string(config_path)
            .map_err(|e| anyhow::anyhow!("读 TS 配置失败 {config_path}: {e}"))?;
        let (constraints, templates_record) =
            aevum_cli::ts_config_to_constraints(layout, &ts_source, inputs)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

        println!("[maintain] TS 配置: {config_path}");
        if let Some(inp) = inputs {
            println!("  显式输入: {inp}");
        }
        println!("  求值+模板展开产出 {} 条约束:", constraints.len());
        for c in &constraints {
            let ver = match (&c.op, &c.ver) {
                (Some(_), Some(v)) => format!(" (= {v})"),
                _ => String::new(),
            };
            println!("    - {}{}", c.name, ver);
        }
        if !yes && !crate::confirm("以上约束是否求解并跑主循环?") {
            println!("已取消。");
            return Ok(());
        }
        aevum_cli::resolve_constraints_opt(layout, &constraints, lock, None, repair, inputs, templates_record.as_deref())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("  → gen-{generation}(从 TS 配置求解的 lock 起跑主循环)");
        aevum_cli::maintain_from_lock(
            layout, lock, mirror, generation,
            active_lock, foundation.map(std::path::Path::new), confirm,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?
    } else if let Some(intent_text) = intent {
        // AI 增强路径,人在回路(ADR-0003 边界3):翻译 → 摊开约束 → 确认 → 写 lock。
        // AI 只在 lock 之前介入;propose/verify/激活全程无 AI(可复现只来自 lock)。
        use aevum_intent::IntentResolver;
        let intent_obj = aevum_intent::Intent::NaturalLanguage(intent_text.to_string());
        let use_mock = mock || aevum_intent::DeepSeekResolver::from_env().is_none();
        let translated = if use_mock {
            aevum_intent::MockIntentResolver::with_defaults().resolve_intent(&intent_obj)
        } else {
            aevum_intent::DeepSeekResolver::from_env().unwrap().resolve_intent(&intent_obj)
        }
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        println!("[maintain] 意图: \"{intent_text}\"");
        println!(
            "  AI({}/{}) 翻译出 {} 条约束:",
            if translated.assist.ai_involved { "介入" } else { "未介入" },
            translated.assist.model_id,
            translated.constraints.len()
        );
        for c in &translated.constraints {
            println!("    - {}", c.name);
        }
        if !yes && !crate::confirm("以上约束是否求解并跑主循环?") {
            println!("已取消。可改措辞,或用 `aevum maintain <包名...>` 显式指定。");
            return Ok(());
        }
        // 确认后求解写 lock(AI 翻译的约束 → 确定性闭包;repair=true 自动应用方案A)。
        aevum_cli::resolve_constraints_opt(layout, &translated.constraints, lock, Some(&translated.assist), repair, None, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("  → gen-{generation}(从意图求解的 lock 起跑主循环)");
        // 走主循环后半段:propose → verify 门禁 → 激活。
        aevum_cli::maintain_from_lock(
            layout, lock, mirror, generation,
            active_lock, foundation.map(std::path::Path::new), confirm,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?
    } else {
        if packages.is_empty() {
            return Err(anyhow::anyhow!("maintain 需要包名、--intent 或 --config"));
        }
        println!("[maintain] 主循环: {} 个顶层包 → gen-{generation}", packages.len());
        aevum_cli::maintain(
            layout, packages, mirror, lock, generation,
            active_lock, foundation.map(std::path::Path::new), repair, confirm,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?
    };

    println!("  ① 求解: 闭包 {} 个包 → locks/{lock}.lock", outcome.resolved_packages);
    println!("  ② propose: 候选 gen-{} 已造({} 个 store 对象,未激活)", outcome.candidate_gen, outcome.store_objects);
    let report = &outcome.activation.report;
    print!("  ③ verify: ");
    if report.passed {
        println!("硬性校验通过(完整性/闭合/层)");
    } else {
        println!("硬性校验失败");
        for f in &report.integrity_failures { println!("       完整性: {} — {}", f.object_id, f.reason); }
        for d in &report.unclosed_deps { println!("       闭合性: {} 需要 {}", d.package, d.requirement); }
        for v in &report.foundation_violations { println!("       层约束: {v}"); }
    }
    for r in &report.version_rollbacks {
        println!("       ⚠ 版本回退: {}: {} < {}(active)", r.package, r.candidate_version, r.active_version);
    }

    if outcome.activation.activated {
        println!("  ④ 激活: ✓ active 已切到 gen-{generation}{}",
            if confirm && report.needs_user_confirm { "(经人工确认放行版本回退)" } else { "" });
        sync_boot_default(layout, generation);
        return Ok(());
    }

    match outcome.activation.blocked_reason {
        Some(ActivateBlocked::HardFail) => {
            eprintln!("  ④ 激活: ✗ 拒绝(硬性校验未通过),候选世代保留待修,active 不动。");
            std::process::exit(1);
        }
        Some(ActivateBlocked::NeedsConfirm) => {
            eprintln!("  ④ 激活: ⚠ 拒绝(版本回退需人工确认),确认无误后加 --confirm 重试。active 不动。");
            std::process::exit(2);
        }
        None => unreachable!("未激活必有 blocked_reason"),
    }
}

#[cfg(not(unix))]
#[allow(clippy::too_many_arguments)]
fn maintain_cmd(
    _layout: &aevum_cli::Layout,
    _packages: &[String],
    _intent: Option<&str>,
    _config: Option<&str>,
    _inputs: Option<&str>,
    _mock: bool,
    _yes: bool,
    _mirror: &str,
    _lock: &str,
    _generation: u64,
    _active_lock: Option<&str>,
    _foundation: Option<&str>,
    _repair: bool,
    _confirm: bool,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("aevum maintain 需要 unix(解包/世代 symlink)。请在 Linux/WSL 运行"))
}

/// 该命令是否会改动 $AEVUM_ROOT 状态(store/世代/active/索引/profile)。
/// 变更类取排他锁串行化(P1-5);只读类可并发。
fn is_mutating(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::Init { .. }
            | Command::Update { .. }
            | Command::Install { .. }
            | Command::Remove { .. }
            | Command::Maintain { .. }
            | Command::NixFetch { .. }
            | Command::Switch { .. }
            | Command::Rollback { .. }
            | Command::Gc { .. }
            | Command::Activate { .. }
            | Command::ComposeGeneration { .. }
            | Command::Ai { .. }
    )
}

/// 设可执行位(0755)。unix 专有;非 unix 为 no-op(引导相关命令本就只在 Linux/WSL 真跑)。
#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}
#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

/// switch/rollback 后,若存在 bootloader 菜单配置源,同步把 DEFAULT 指向该世代。
/// 这是"回滚=真命令"的关键:世代 active 指针与开机默认项一起切。
/// 找不到配置(未造可引导镜像)则静默跳过——普通包管理场景不涉及 bootloader。
fn sync_boot_default(layout: &aevum_cli::Layout, generation: u64) {
    let cfg = layout.boot_menu_cfg();
    if !cfg.exists() {
        return;
    }
    match aevum_generation::bootloader::set_default(&cfg, generation) {
        Ok(()) => {
            println!("[boot] 菜单 DEFAULT → gen-{generation}({})", cfg.display());
            println!("  同步进可引导镜像: 重跑 build-bootimage.sh,或 mcopy -D o -i <img> {} ::/", cfg.display());
        }
        Err(e) => {
            // 菜单里无该世代等情况:明确告知,不静默吞(诚实)。
            eprintln!("[boot] ⚠ 未能更新菜单 DEFAULT: {e}");
            eprintln!("  (世代 active 指针已切;若要它出现在开机菜单,先 aevum boot-menu --gens ...,include gen-{generation})");
        }
    }
}

/// 安装包的共享核心:resolve → install → refresh_profile(供 Install 命令与 AI 分发共用)。
#[cfg(unix)]
fn do_install(
    layout: &aevum_cli::Layout,
    packages: &[String],
    only: &[String],
    mirror: &str,
    gen_id: u64,
    gated: bool,
    confirm: bool,
) -> anyhow::Result<()> {
    let lock = aevum_cli::resolve(layout, packages, &packages[0])
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "[install] 求解: closure_id={} ({} 包, 未解析 {})",
        lock.closure_id, lock.package_count, lock.diagnostics.unresolved.len()
    );
    let targets: Vec<&str> = if only.is_empty() {
        lock.locked.iter().map(|p| p.name.as_str()).collect()
    } else {
        only.iter().map(|s| s.as_str()).collect()
    };
    println!("将安装 {} 个包 → gen-{gen_id}", targets.len());
    println!("  镜像: {mirror}");

    if gated {
        // AI 选包:走 verify 门禁(ADR-0005)。propose 候选 → verify → 通过才激活。
        let (report, outcome) =
            aevum_cli::install_gated(layout, &lock, &packages[0], mirror, only, gen_id, confirm)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        if !outcome.activated {
            match outcome.blocked_reason {
                Some(aevum_cli::ActivateBlocked::HardFail) => {
                    return Err(anyhow::anyhow!(
                        "🛡 verify 门禁拒绝激活 gen-{gen_id}(完整性/闭合性硬失败)。候选世代已造但未激活,active 不动。"
                    ));
                }
                Some(aevum_cli::ActivateBlocked::NeedsConfirm) => {
                    return Err(anyhow::anyhow!(
                        "🛡 verify 门禁:检出版本回退,需人工确认。用 `aevum ai --yes \"...\"` 或显式 `aevum install` 放行。候选 gen-{gen_id} 未激活。"
                    ));
                }
                None => {}
            }
        }
        println!("[install] 🛡 verify 门禁通过 → gen-{} 已激活({} 个 store 对象)", report.generation, report.store_objects);
    } else {
        // 人类显式敲包名:便捷直装(历史行为)。
        let report = aevum_cli::install(layout, &lock, mirror, only, gen_id)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("[install] gen-{} 已激活({} 个 store 对象)", report.generation, report.store_objects);
    }

    // 记录该世代由哪个 lock 构建(供 list/remove 认 active 世代,而非瞎猜最新 lock)。
    if let Err(e) = aevum_cli::record_generation_lock(layout, gen_id, &packages[0]) {
        println!("  ⚠ 记录世代 lock 指针失败: {e}");
    }
    match aevum_cli::refresh_profile(layout) {
        Ok(n) => println!("  profile/bin: {n} 个可执行文件已就绪"),
        Err(e) => println!("  ⚠ profile 刷新: {e}"),
    }
    println!("\n  直接使用: {}", packages.join(" / "));
    Ok(())
}

#[cfg(not(unix))]
fn do_install(
    _l: &aevum_cli::Layout,
    _p: &[String],
    _o: &[String],
    _m: &str,
    _g: u64,
    _gated: bool,
    _confirm: bool,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("install 需要 unix"))
}

/// 打印版本冲突警告(ai/02 repair 触发依据)。无冲突则静默。
/// 冲突不阻断 lock 产出(已选版本仍确定),但醒目提示用户:某包被互斥约束要求,
/// 当前只满足了一方,另一方实际跑不起来——需 repair(放宽/升降级/保留两份)。
fn warn_conflicts(lock: &aevum_solver::Lock) {
    if lock.diagnostics.conflicts.is_empty() {
        return;
    }
    println!("  ⚠ 检出 {} 处版本冲突(互斥约束,需 repair):", lock.diagnostics.conflicts.len());
    for c in &lock.diagnostics.conflicts {
        println!(
            "      {} 已选 {},但 {} 要求 ({} {}) — 该约束未被满足",
            c.package, c.chosen_version, c.source, c.required_op, c.required_ver
        );
    }
    // repair 方案A 建议(放宽约束求单一共存版本)。
    for s in &lock.diagnostics.repair_suggestions {
        match &s.satisfying_version {
            Some(v) => println!(
                "      ↳ 方案A: {} 可放宽到 {}(同时满足 {:?})",
                s.package, v, s.constraints
            ),
            None => println!(
                "      ↳ 方案A 不适用: {} 无单一版本同时满足 {:?},需方案B/C(升降级/保留两份)",
                s.package, s.constraints
            ),
        }
    }
    // repair 方案B 建议(升级父包求兼容)。
    for b in &lock.diagnostics.repair_suggestions_b {
        println!(
            "      ↳ 方案B: 升级父包 {} 到 {} → {} 可取 {} 共存",
            b.parent, b.upgrade_parent_to, b.dependency, b.dependency_version
        );
    }
    // repair 方案C(保留两份):各方各用各版本,需用户确认(占盘+各自安全更新)。
    for c in &lock.diagnostics.keep_two_suggestions {
        println!(
            "      ↳ 方案C(需确认): {} 保留两份 — {} 给 {:?},{} 给 {:?}(占盘+两份各自跟进安全更新)",
            c.package, c.version_a, c.sources_a, c.version_b, c.sources_b
        );
    }
    // repair 方案D(隔离失败,需用户取舍):A/B/C 都无解,如实告知,绝不静默删一方。
    for u in &lock.diagnostics.unrepairable {
        println!(
            "      ✗ 方案D: {} 无法共存(约束 {:?} 来自 {:?})— 自动修复手段已穷尽,需你二选一取舍",
            u.package, u.constraints, u.sources
        );
    }
}

/// AI 评估依赖冲突并给出修复建议(当 config.toml 配了 AI 且有冲突时)。
/// 这是 AI-native 的核心:AI 分析多个修复方案,选最优并解释。
fn ai_assist_conflicts(layout: &aevum_cli::Layout, lock: &aevum_solver::Lock) {
    if lock.diagnostics.conflicts.is_empty() {
        return;
    }
    let config_path = layout.root.join("config.toml");
    let ai_cfg = aevum_intent::ai_client::AiConfig::load(&config_path);
    if !ai_cfg.is_available() {
        return; // 无 AI,跳过(已有确定性建议打印)
    }

    // 格式化冲突
    let conflicts: Vec<(String, String, String, String)> = lock.diagnostics.conflicts.iter()
        .map(|c| (c.package.clone(), c.chosen_version.clone(), c.source.clone(),
                  format!("{} {}", c.required_op, c.required_ver)))
        .collect();
    let conflicts_desc = aevum_intent::ai_client::format_conflicts(&conflicts);

    // 格式化各方案建议
    let plan_a: Vec<(String, Option<String>)> = lock.diagnostics.repair_suggestions.iter()
        .map(|s| (s.package.clone(), s.satisfying_version.clone()))
        .collect();
    let plan_b: Vec<(String, String, String, String)> = lock.diagnostics.repair_suggestions_b.iter()
        .map(|b| (b.parent.clone(), b.upgrade_parent_to.clone(), b.dependency.clone(), b.dependency_version.clone()))
        .collect();
    let plan_c: Vec<(String, String, String)> = lock.diagnostics.keep_two_suggestions.iter()
        .map(|c| (c.package.clone(), c.version_a.clone(), c.version_b.clone()))
        .collect();
    let suggestions_desc = aevum_intent::ai_client::format_suggestions(&plan_a, &plan_b, &plan_c);

    println!("\n  🤖 AI 分析冲突中({}/{})...", ai_cfg.provider, ai_cfg.model);
    match aevum_intent::ai_client::ai_evaluate_repair(&ai_cfg, &conflicts_desc, &suggestions_desc) {
        Ok(decision) => {
            println!("  AI 推荐方案 {}: {}", decision.chosen_plan, decision.action);
            println!("  理由: {}", decision.reasoning);
            if ai_cfg.auto_repair && decision.chosen_plan == "A" {
                println!("  (auto_repair 开启 + 方案A 安全 → 可用 --repair 自动应用)");
            } else if decision.chosen_plan != "A" {
                println!("  (方案 {} 需人工确认,不自动执行)", decision.chosen_plan);
            }
        }
        Err(e) => println!("  AI 分析失败(降级到上述确定性建议): {e}"),
    }
}

/// 交互确认:打印提示,从 stdin 读一行,y/yes(含中文"是")为确认。
/// 非交互环境(stdin 非 tty / 读不到)默认否,避免脚本里误判为确认。
fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => false, // EOF/无输入 → 否
        Ok(_) => {
            let a = line.trim().to_lowercase();
            matches!(a.as_str(), "y" | "yes" | "是" | "确认")
        }
    }
}
