//! 补闭包(closure completion):给定一个包,产出它的完整运行闭包。
//!
//! 参照:`poc/poc4-arch-isolation/build_closure.py`(递归补闭包骨架)
//! + PoC-5 报告的算法修正。设计:`docs/architecture/foundations/05-multi-source-and-isolation.md`。
//!
//! # PoC-5 铁律:四源合一(照直觉只递归主二进制 NEEDED 会崩)
//! 复杂包(python 77 扩展、imagemagick 137 插件)靠运行时 `dlopen`,补闭包必须合并四个来源:
//!   1. 主二进制 `DT_NEEDED` 递归;
//!   2. 扫**全包所有 ELF**(插件/扩展),各自解 NEEDED 纳入其依赖;
//!   3. 上游元数据声明的运行时目录(标准库 / 插件路径)整体纳入;
//!   4. 写死的数据路径整目录纳入。
//!
//! # PoC-4 铁律:同源补闭包
//! 库必须从包自己那一源取(Arch 包带 Arch 的 glibc),**绝不跨源拼库**——坑在 ABI 不在路径。
//!
//! 本文件先落骨架与数据结构,把四源合并逐项标 TODO(里程碑1/2 实现),
//! 但**结构上强制四源**,避免后续退化成"只递归主二进制"。

use aevum_elf::ElfInfo;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClosureError {
    #[error("ELF 分析失败: {0}")]
    Elf(#[from] aevum_elf::ElfError),
    #[error("跨源拼库被拒绝(PoC-4 铁律:补闭包必须同源): 需求 {needed} 来自源 {want}, 但只在源 {have} 找到")]
    CrossSource { needed: String, want: String, have: String },
    #[error("运行闭包不完整: 库 {0} 在同源内未找到")]
    MissingLib(String),
}

type Result<T> = std::result::Result<T, ClosureError>;

/// 包来源(PoC-4:同源约束的载体)。补闭包时库只能从同一 `Source` 取。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Nix,
    Arch,
    Debian,
    /// 其它源,带标识。
    Other(String),
}

/// 同源策略(块4→里程碑4:从"诊断可见"升级为可选硬约束)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePolicy {
    /// 宽松(里程碑1-3 默认):跨源仅记 `cross_source` 诊断,不阻断。
    Lenient,
    /// 严格(PoC-4 铁律):库来自异于包 source 的源 → `Err(CrossSource)` 硬阻断。
    Strict,
}

impl Default for SourcePolicy {
    fn default() -> Self {
        SourcePolicy::Lenient
    }
}

/// 补闭包的输入:一个待补全的包。
#[derive(Debug, Clone)]
pub struct PackageInput {
    pub name: String,
    /// 包的来源(决定从哪一源找库,PoC-4)。
    pub source: Source,
    /// 包解包后的根目录(用于扫全包 ELF / 纳入运行时目录)。
    pub root: PathBuf,
    /// 主二进制相对包根的路径(四源之一的起点)。
    pub main_binary: Option<PathBuf>,
    /// 上游元数据声明的运行时目录(标准库/插件路径,四源之三),相对包根。
    pub runtime_dirs: Vec<PathBuf>,
    /// 写死的数据路径(四源之四),相对包根。
    pub data_dirs: Vec<PathBuf>,
}

/// 补闭包结果:运行该包所需的全部库(soname)与需整体纳入的目录。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Closure {
    /// 需要的共享库 soname 集合(去重,确定性排序)。
    pub needed_libs: BTreeSet<String>,
    /// 需整体纳入闭包的目录(运行时目录 + 数据目录,绝对路径)。
    pub included_dirs: BTreeSet<PathBuf>,
    /// 参与扫描的 ELF 文件数(诊断:验证"扫了全包"而非只主二进制)。
    pub scanned_elf_count: usize,
    /// soname → 同源解析到的真实库文件路径(由 [`build_closure_resolved`] 填充)。
    pub resolved_libs: BTreeMap<String, PathBuf>,
    /// `PT_INTERP` 解析到的真实动态链接器文件(可执行包才有)。
    pub interpreter: Option<PathBuf>,
    /// 同源内未找到的 soname(成本信号:需从上游另取,PoC-4)。
    pub missing_libs: BTreeSet<String>,
    /// 跨源命中(块4 诊断,PoC-4 同源铁律的可观测):
    /// 库实际来自异于包 `source` 的源(如 Arch 包的库从 Debian 宿主取)。
    /// 严格模式下收集为诊断,**不硬阻断**——真正多源 store 路由留里程碑4。
    pub cross_source: Vec<CrossSourceHit>,
}

/// 一次跨源命中:soname 从哪个源解出,而期望源是什么(PoC-4 同源铁律的诊断)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossSourceHit {
    pub soname: String,
    pub want: Source,
    pub got: Source,
    pub path: PathBuf,
}

/// 库解析器:把 soname 解析到同源内的具体提供方。PoC-4 同源约束在此强制。
///
/// 骨架阶段为 trait,完整实现(从同源索引/store 查找)留待里程碑1。
pub trait LibResolver {
    /// 在指定源内解析一个 soname。跨源应返回 `None`,由调用方报 [`ClosureError::CrossSource`]。
    fn resolve_in_source(&self, soname: &str, source: &Source) -> Option<PathBuf>;

    /// 该 resolver 提供的库实际来自哪个源(块4:同源诊断)。
    /// 默认 `None`(来源未知,等价里程碑1/2 的放宽);具体 resolver 应 override 标注。
    fn provenance(&self) -> Option<Source> {
        None
    }

    /// 解析并报告来源(块4):返回 `(真实路径, 该库来自的源)`。
    /// 默认实现 = `resolve_in_source` + 本 resolver 的 `provenance`。
    /// 组合型 resolver([`ChainResolver`])应 override,返回**实际命中**子 resolver 的来源。
    fn resolve_with_provenance(
        &self,
        soname: &str,
        source: &Source,
    ) -> Option<(PathBuf, Option<Source>)> {
        self.resolve_in_source(soname, source)
            .map(|p| (p, self.provenance()))
    }
}

/// 从宿主标准库路径解析 soname(PoC-4 `find_lib` 直译)。
///
/// # 里程碑1 同源简化(显式标注,里程碑2 收紧)
/// PoC-4 铁律是"补闭包必须同源"。里程碑1 验证机制闭环,**把宿主当作唯一源**——
/// 即便目标包(Arch rg)与宿主(Debian)不同源,库也统一从宿主标准路径取。
/// 因此这里忽略 `source` 参数、不做 `CrossSource` 检查。
/// 接真实多源(里程碑2)时,应按 `source` 路由到对应源的索引/store,并恢复同源校验。
pub struct HostLibResolver {
    search_dirs: Vec<PathBuf>,
}

impl HostLibResolver {
    /// 用 PoC-4 的四个标准库路径构造(x86_64 Debian/Ubuntu 布局)。
    pub fn new() -> Self {
        HostLibResolver {
            search_dirs: [
                "/lib/x86_64-linux-gnu",
                "/usr/lib/x86_64-linux-gnu",
                "/lib64",
                "/usr/lib",
            ]
            .iter()
            .map(PathBuf::from)
            .collect(),
        }
    }

    /// 用自定义搜索路径构造(测试注入临时目录)。
    pub fn with_dirs(dirs: Vec<PathBuf>) -> Self {
        HostLibResolver { search_dirs: dirs }
    }
}

impl Default for HostLibResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LibResolver for HostLibResolver {
    fn resolve_in_source(&self, soname: &str, _source: &Source) -> Option<PathBuf> {
        for dir in &self.search_dirs {
            let p = dir.join(soname);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// 宿主库来自 Debian(本环境的宿主发行版)。块4 据此判断跨源。
    fn provenance(&self) -> Option<Source> {
        Some(Source::Debian)
    }
}

/// 从**包内**库目录解析 soname(PoC-5:复杂包自带库,如 python 的 `libpython3.14.so.1.0`)。
///
/// 复杂包把核心库放在包内(`usr/lib/`),宿主没有 → 必须先查包内,否则误报 missing。
/// 与 [`HostLibResolver`] 经 [`ChainResolver`] 组合:包内优先、宿主兜底。
///
/// 解析时跟进符号链接到真实文件(`libpython3.14.so` → `.so.1.0`),
/// 让入库拿到真正的内容(软链本身在整目录入库时由 ingest_dir 保留)。
pub struct PackageLibResolver {
    /// 包内库搜索目录(绝对路径,如 `<root>/usr/lib`、`<root>/usr/lib/python3.14/lib-dynload`)。
    pkg_dirs: Vec<PathBuf>,
    /// 包自身的源(块4:包内库即来自包的 source)。
    source: Source,
}

impl PackageLibResolver {
    /// 默认源 Arch(本项目 fixture 包均为 Arch);如需指定用 [`Self::with_source`]。
    pub fn new(pkg_dirs: Vec<PathBuf>) -> Self {
        PackageLibResolver {
            pkg_dirs,
            source: Source::Arch,
        }
    }

    pub fn with_source(pkg_dirs: Vec<PathBuf>, source: Source) -> Self {
        PackageLibResolver { pkg_dirs, source }
    }
}

impl LibResolver for PackageLibResolver {
    fn resolve_in_source(&self, soname: &str, _source: &Source) -> Option<PathBuf> {
        for dir in &self.pkg_dirs {
            let p = dir.join(soname);
            if p.exists() {
                // 跟进软链到真实文件(libfoo.so → libfoo.so.1.0),取真正内容。
                return Some(std::fs::canonicalize(&p).unwrap_or(p));
            }
        }
        None
    }

    /// 包内库来自包自身的源。
    fn provenance(&self) -> Option<Source> {
        Some(self.source.clone())
    }
}

/// 按序组合多个 resolver:第一个命中即返回(PoC-5:包内优先,宿主兜底)。
pub struct ChainResolver {
    resolvers: Vec<Box<dyn LibResolver>>,
}

impl ChainResolver {
    pub fn new(resolvers: Vec<Box<dyn LibResolver>>) -> Self {
        ChainResolver { resolvers }
    }
}

impl LibResolver for ChainResolver {
    fn resolve_in_source(&self, soname: &str, source: &Source) -> Option<PathBuf> {
        for r in &self.resolvers {
            if let Some(p) = r.resolve_in_source(soname, source) {
                return Some(p);
            }
        }
        None
    }

    /// 透传**实际命中**子 resolver 的来源(块4:跨源诊断的关键)。
    fn resolve_with_provenance(
        &self,
        soname: &str,
        source: &Source,
    ) -> Option<(PathBuf, Option<Source>)> {
        for r in &self.resolvers {
            if let Some(p) = r.resolve_in_source(soname, source) {
                return Some((p, r.provenance()));
            }
        }
        None
    }
}

/// 按 soname 的**期望源**路由到对应源的 resolver(里程碑4 块3:多源路由机制)。
///
/// # 诚实标注:机制演示,非真实多源验证
/// 本仓库只有 3 个 Arch 包,无 Nix/Debian 同名包数据。此 resolver 实现多源**机制**
/// (按 soname→源 的映射路由到对应源目录),用构造数据单测验证路由正确性。
/// 真实多源验证需带同名包的多源数据,本仓库暂无,留待有数据时。
///
/// 路由表 `routes: soname → Source`;每个 `Source` 对应一个子 resolver。
/// 未在路由表的 soname 走 `default` resolver。
pub struct SourceRoutedResolver {
    routes: BTreeMap<String, Source>,
    by_source: BTreeMap<Source, Box<dyn LibResolver>>,
    default: Option<Box<dyn LibResolver>>,
}

impl SourceRoutedResolver {
    pub fn new() -> Self {
        SourceRoutedResolver {
            routes: BTreeMap::new(),
            by_source: BTreeMap::new(),
            default: None,
        }
    }

    /// 注册某源的 resolver。
    pub fn with_source_resolver(mut self, source: Source, r: Box<dyn LibResolver>) -> Self {
        self.by_source.insert(source, r);
        self
    }

    /// 把一个 soname 路由到指定源。
    pub fn route(mut self, soname: impl Into<String>, source: Source) -> Self {
        self.routes.insert(soname.into(), source);
        self
    }

    /// 未路由 soname 的兜底 resolver。
    pub fn with_default(mut self, r: Box<dyn LibResolver>) -> Self {
        self.default = Some(r);
        self
    }
}

impl Default for SourceRoutedResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LibResolver for SourceRoutedResolver {
    fn resolve_in_source(&self, soname: &str, source: &Source) -> Option<PathBuf> {
        self.resolve_with_provenance(soname, source).map(|(p, _)| p)
    }

    fn resolve_with_provenance(
        &self,
        soname: &str,
        source: &Source,
    ) -> Option<(PathBuf, Option<Source>)> {
        // soname 有显式路由 → 用该源的 resolver,provenance = 路由目标源。
        if let Some(routed) = self.routes.get(soname) {
            if let Some(r) = self.by_source.get(routed) {
                return r
                    .resolve_in_source(soname, source)
                    .map(|p| (p, Some(routed.clone())));
            }
            return None;
        }
        // 未路由 → 兜底 resolver(透传其 provenance)。
        self.default
            .as_ref()
            .and_then(|r| r.resolve_in_source(soname, source).map(|p| (p, r.provenance())))
    }
}

/// 补闭包主入口。**结构上强制四源合并**——每一源都纳入 `needed_libs`/`included_dirs`。
///
/// 当前为骨架:四源的合并框架已就位,各源的深度递归/同源查找标 TODO。
/// 完整实现后,这里产出的闭包须能让 python/imagemagick 这类 dlopen 包运行时不崩(PoC-5)。
pub fn build_closure(input: &PackageInput) -> Result<Closure> {
    let mut closure = Closure::default();

    // —— 源 1:主二进制 DT_NEEDED 递归 ——
    if let Some(main_rel) = &input.main_binary {
        let main_path = input.root.join(main_rel);
        match aevum_elf::parse_file(&main_path) {
            Ok(info) => {
                add_needed(&mut closure, &info);
                closure.scanned_elf_count += 1;
                // TODO(里程碑1): 递归——对每个 NEEDED 在同源内定位其文件,再解其 NEEDED,
                // 直到不动点。用 LibResolver::resolve_in_source 保证同源(PoC-4)。
            }
            Err(aevum_elf::ElfError::NotElf(_)) => {} // 主"二进制"可能是脚本,跳过
            Err(e) => return Err(e.into()),
        }
    }

    // —— 源 2:扫全包所有 ELF(插件/扩展,dlopen 目标)——
    // PoC-5 关键:python 77 扩展 / imagemagick 137 插件不在主二进制 NEEDED 里。
    let all_elf: Vec<ElfInfo> = aevum_elf::scan_dir(&input.root)?;
    for info in &all_elf {
        add_needed(&mut closure, info);
    }
    closure.scanned_elf_count = all_elf.len();
    // TODO(里程碑2): 对每个插件 ELF 的 NEEDED 同样递归补全(同源)。

    // —— 源 3:上游元数据声明的运行时目录,整体纳入 ——
    for rel in &input.runtime_dirs {
        closure.included_dirs.insert(input.root.join(rel));
    }

    // —— 源 4:写死的数据路径,整目录纳入 ——
    for rel in &input.data_dirs {
        closure.included_dirs.insert(input.root.join(rel));
    }

    // 注:同源校验(把 needed_libs 全部解析到 input.source 内、跨源报 CrossSource)
    // 由 build_closure_resolved 用 LibResolver 完成;本函数只做"扫全包产 soname 集合"。

    Ok(closure)
}

/// 递归补闭包并解析到真实库文件(PoC-4 `closure_walk` 直译 + PoC-5 扫全包)。
///
/// 在 [`build_closure`](只产 soname 集合)之上:
/// 1. 跑 `build_closure` 拿**全包** soname 集合(保 PoC-5:含 dlopen 插件的 NEEDED,不退化为只主二进制);
/// 2. 解 `main_binary` 的 `PT_INTERP` → 解析到真实 loader 文件;
/// 3. BFS 工作队列(初始 = 全包 soname 全集),每个 soname 用 `resolver` 解到真实文件,
///    再解该库自身的 NEEDED 入队(同源递归),直到不动点;
/// 4. 解不到的进 `missing_libs`(成本信号)。
///
/// BFS + `seen` 去重保证终止;`BTreeMap`/`BTreeSet` 保证确定性输出。
///
/// 默认 [`SourcePolicy::Lenient`](里程碑1-3 行为:跨源仅诊断)。
/// 需硬约束(PoC-4 铁律)用 [`build_closure_resolved_with_policy`]。
pub fn build_closure_resolved(
    input: &PackageInput,
    resolver: &dyn LibResolver,
) -> Result<Closure> {
    build_closure_resolved_with_policy(input, resolver, SourcePolicy::Lenient)
}

/// 同上,但显式指定同源策略(里程碑4 块3)。
///
/// [`SourcePolicy::Strict`] 下,任一库来自异于 `input.source` 的源 →
/// `Err(ClosureError::CrossSource)`(落地 PoC-4 同源铁律的硬阻断)。
pub fn build_closure_resolved_with_policy(
    input: &PackageInput,
    resolver: &dyn LibResolver,
    policy: SourcePolicy,
) -> Result<Closure> {
    let mut closure = build_closure(input)?;

    // —— interpreter:PT_INTERP 是绝对路径整串,不是 soname ——
    if let Some(main_rel) = &input.main_binary {
        let main_path = input.root.join(main_rel);
        if let Ok(info) = aevum_elf::parse_file(&main_path) {
            if let Some(interp) = &info.interpreter {
                let abs = PathBuf::from(interp);
                let resolved = if abs.exists() {
                    Some(abs)
                } else {
                    // 绝对路径在本环境不存在 → 取 basename 走同源解析(PoC-4 interp_real)。
                    abs.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| resolver.resolve_in_source(n, &input.source))
                };
                closure.interpreter = resolved;
            }
        }
    }

    // —— BFS 递归补闭包:初始队列 = 全包 soname 全集(PoC-5 不退化)——
    let mut queue: Vec<String> = closure.needed_libs.iter().cloned().collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(soname) = queue.pop() {
        if !seen.insert(soname.clone()) {
            continue;
        }
        match resolver.resolve_with_provenance(&soname, &input.source) {
            Some((real, prov)) => {
                // 块4 同源诊断:命中源异于包 source → 记跨源。
                // Lenient(默认)仅记录;Strict(里程碑4)硬阻断(PoC-4 铁律)。
                if let Some(got) = prov {
                    if got != input.source {
                        if policy == SourcePolicy::Strict {
                            return Err(ClosureError::CrossSource {
                                needed: soname.clone(),
                                want: format!("{:?}", input.source),
                                have: format!("{got:?}"),
                            });
                        }
                        closure.cross_source.push(CrossSourceHit {
                            soname: soname.clone(),
                            want: input.source.clone(),
                            got,
                            path: real.clone(),
                        });
                    }
                }
                // 解该库自身 NEEDED,入队递归(同源)。
                if let Ok(info) = aevum_elf::parse_file(&real) {
                    for dep in info.needed {
                        if !seen.contains(&dep) {
                            queue.push(dep);
                        }
                    }
                }
                closure.resolved_libs.insert(soname, real);
            }
            None => {
                closure.missing_libs.insert(soname);
            }
        }
    }

    Ok(closure)
}

/// 把一个 ELF 的 NEEDED 列表并入闭包(去重由 BTreeSet 保证,顺序确定)。
fn add_needed(closure: &mut Closure, info: &ElfInfo) {
    for lib in &info.needed {
        closure.needed_libs.insert(lib.clone());
    }
}

/// 自动推断包的运行时目录(PoC-5 源3/4 的元数据来源)。
///
/// # 为什么用布局启发式而非读元数据
/// Arch `.PKGINFO` 只有 `pkgname`/`provides`/`depend`(包依赖名),**无运行时路径字段**——
/// "标准库在 usr/lib/python3.14"这种信息上游没标(实测确认)。因此只能扫包内布局推断:
/// 找**插件目录**(含大量 `.so` 的目录:python `lib-dynload`、im `coders`),
/// 取其**运行时根**(再上溯到含标准库/config 的目录),整体纳入闭包。
///
/// 接 nixpkgs 这类带路径元数据的源时(里程碑4),可换更精确的来源。
///
/// 判定:目录名含 `lib-dynload`/`coders`/`modules` 关键词,**或**目录直接含 ≥ `MIN_SO` 个 `.so`。
/// 命中后纳入其"运行时根":若命中目录在 `.../<runtime>/.../plugins`,上溯到 `<runtime>`
/// (取包内 `usr/lib/<X>` 这一层),保证标准库 .py / config 一起进。
pub fn infer_runtime_dirs(root: impl AsRef<Path>) -> Vec<PathBuf> {
    const MIN_SO: usize = 5;
    let root = root.as_ref();
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();

    // 递归走目录,统计每个目录直接含的 .so 数。
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut so_count = 0usize;
        for e in entries.flatten() {
            let path = e.path();
            let ft = match std::fs::symlink_metadata(&path) {
                Ok(m) => m.file_type(),
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue; // 不跟进软链(PoC-5)
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("so")
            {
                so_count += 1;
            }
        }
        let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let is_plugin_dir = matches!(name, "lib-dynload" | "coders" | "modules")
            || name.starts_with("modules-");
        if is_plugin_dir || so_count >= MIN_SO {
            if let Some(rel_root) = runtime_root_of(root, &dir) {
                roots.insert(rel_root);
            }
        }
    }
    roots.into_iter().collect()
}

/// 由命中的插件目录上溯到"运行时根"(相对 root)。
///
/// 取 `usr/lib/<X>` 这一层作运行时根(如 `usr/lib/python3.14`、`usr/lib/ImageMagick-7.1.2`),
/// 这样标准库 .py / config-* 与插件一起纳入。命中目录若本身就是 `usr/lib/<X>` 则取自身;
/// 更深(`usr/lib/<X>/modules-Q16HDRI/coders`)则上溯到 `usr/lib/<X>`。
fn runtime_root_of(root: &Path, hit: &Path) -> Option<PathBuf> {
    let rel = hit.strip_prefix(root).ok()?;
    let comps: Vec<_> = rel.components().collect();
    // 期望前缀 usr/lib/<X>...;取前 3 段作运行时根。
    if comps.len() >= 3 {
        let r: PathBuf = comps[..3].iter().collect();
        // 校验前两段确是 usr/lib(避免误纳别处的 .so 堆)。
        if comps[0].as_os_str() == "usr" && comps[1].as_os_str() == "lib" {
            return Some(r);
        }
    }
    // 退化:命中目录本身相对路径(如直接 usr/lib 下大量 .so,少见)。
    Some(rel.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_package_empty_closure() {
        let root = std::env::temp_dir().join(format!("aevum-clo-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let input = PackageInput {
            name: "empty".into(),
            source: Source::Debian,
            root: root.clone(),
            main_binary: None,
            runtime_dirs: vec![],
            data_dirs: vec![],
        };
        let c = build_closure(&input).unwrap();
        assert!(c.needed_libs.is_empty());
        assert_eq!(c.scanned_elf_count, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn runtime_and_data_dirs_included() {
        // 源 3/4:即使没有 ELF,声明的运行时/数据目录也必须纳入闭包(PoC-5)。
        let root = std::env::temp_dir().join(format!("aevum-clo-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib/python3.11")).unwrap();
        std::fs::create_dir_all(root.join("share/data")).unwrap();
        let input = PackageInput {
            name: "python3".into(),
            source: Source::Debian,
            root: root.clone(),
            main_binary: None,
            runtime_dirs: vec![PathBuf::from("lib/python3.11")],
            data_dirs: vec![PathBuf::from("share/data")],
        };
        let c = build_closure(&input).unwrap();
        assert!(c.included_dirs.contains(&root.join("lib/python3.11")));
        assert!(c.included_dirs.contains(&root.join("share/data")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_resolver_finds_in_search_order() {
        // HostLibResolver 按 search_dirs 顺序返回第一个存在的 <dir>/<soname>。
        let root = std::env::temp_dir().join(format!("aevum-clo-resolv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir_a = root.join("a");
        let dir_b = root.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        // libfoo 只在 b;libbar 两处都有(应取 a,因 a 在前)
        std::fs::write(dir_b.join("libfoo.so.1"), b"foo").unwrap();
        std::fs::write(dir_a.join("libbar.so.1"), b"bar-a").unwrap();
        std::fs::write(dir_b.join("libbar.so.1"), b"bar-b").unwrap();

        let resolver = HostLibResolver::with_dirs(vec![dir_a.clone(), dir_b.clone()]);
        assert_eq!(
            resolver.resolve_in_source("libfoo.so.1", &Source::Debian),
            Some(dir_b.join("libfoo.so.1"))
        );
        assert_eq!(
            resolver.resolve_in_source("libbar.so.1", &Source::Debian),
            Some(dir_a.join("libbar.so.1")), // 搜索顺序优先
        );
        assert_eq!(
            resolver.resolve_in_source("libmissing.so.9", &Source::Debian),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolved_closure_empty_package() {
        // 空包(无 main_binary、无 ELF):build_closure_resolved 不崩,各集合为空。
        let root = std::env::temp_dir().join(format!("aevum-clo-rempty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let input = PackageInput {
            name: "empty".into(),
            source: Source::Debian,
            root: root.clone(),
            main_binary: None,
            runtime_dirs: vec![],
            data_dirs: vec![],
        };
        let resolver = HostLibResolver::with_dirs(vec![]);
        let c = build_closure_resolved(&input, &resolver).unwrap();
        assert!(c.resolved_libs.is_empty());
        assert!(c.missing_libs.is_empty());
        assert!(c.interpreter.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn chain_resolver_package_first() {
        // PoC-5:包内库优先于宿主(libpython 在包里,宿主没有)。
        let root = std::env::temp_dir().join(format!("aevum-clo-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pkg = root.join("pkg-lib");
        let host = root.join("host-lib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::create_dir_all(&host).unwrap();
        // libshared 两处都有(应取包内);libhostonly 只在宿主
        std::fs::write(pkg.join("libshared.so.1"), b"pkg-ver").unwrap();
        std::fs::write(host.join("libshared.so.1"), b"host-ver").unwrap();
        std::fs::write(host.join("libhostonly.so.1"), b"host").unwrap();

        let chain = ChainResolver::new(vec![
            Box::new(PackageLibResolver::new(vec![pkg.clone()])),
            Box::new(HostLibResolver::with_dirs(vec![host.clone()])),
        ]);
        // 共享库:包内优先(比内容,避开 canonicalize 的 verbatim 前缀差异)。
        let shared = chain
            .resolve_in_source("libshared.so.1", &Source::Arch)
            .expect("应命中");
        assert_eq!(
            std::fs::read(&shared).unwrap(),
            b"pkg-ver",
            "应取包内版本而非宿主"
        );
        // 仅宿主有:兜底命中
        let hostonly = chain
            .resolve_in_source("libhostonly.so.1", &Source::Arch)
            .expect("应兜底命中");
        assert_eq!(std::fs::read(&hostonly).unwrap(), b"host");
        // 都没有:None
        assert_eq!(chain.resolve_in_source("libnope.so", &Source::Arch), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn package_resolver_follows_symlink() {
        // libfoo.so → libfoo.so.1.0:resolver 跟进软链取真实文件内容。
        #[cfg(unix)]
        {
            let root =
                std::env::temp_dir().join(format!("aevum-clo-sym-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let dir = root.join("lib");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("libfoo.so.1.0"), b"real").unwrap();
            std::os::unix::fs::symlink("libfoo.so.1.0", dir.join("libfoo.so")).unwrap();
            let r = PackageLibResolver::new(vec![dir.clone()]);
            let got = r.resolve_in_source("libfoo.so", &Source::Arch).unwrap();
            // canonicalize 后指向真实文件
            assert!(got.ends_with("libfoo.so.1.0"), "应跟进软链到真实文件,实得 {got:?}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn infer_runtime_dirs_python_layout() {
        // 模拟 python 布局:usr/lib/python3.14/lib-dynload 含多个 .so → 推断出 usr/lib/python3.14。
        let root = std::env::temp_dir().join(format!("aevum-infer-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dynload = root.join("usr/lib/python3.14/lib-dynload");
        std::fs::create_dir_all(&dynload).unwrap();
        for i in 0..8 {
            std::fs::write(dynload.join(format!("_mod{i}.so")), b"x").unwrap();
        }
        std::fs::write(root.join("usr/lib/python3.14/os.py"), b"# stdlib").unwrap();
        let dirs = infer_runtime_dirs(&root);
        assert!(
            dirs.contains(&PathBuf::from("usr/lib/python3.14")),
            "应推断出运行时根 usr/lib/python3.14,实得 {dirs:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn infer_runtime_dirs_imagemagick_layout() {
        // im 布局:usr/lib/ImageMagick-7.1.2/modules-Q16HDRI/coders 含多个 .so → 上溯到运行时根。
        let root = std::env::temp_dir().join(format!("aevum-infer-im-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let coders = root.join("usr/lib/ImageMagick-7.1.2/modules-Q16HDRI/coders");
        std::fs::create_dir_all(&coders).unwrap();
        for i in 0..10 {
            std::fs::write(coders.join(format!("c{i}.so")), b"x").unwrap();
        }
        let dirs = infer_runtime_dirs(&root);
        assert!(
            dirs.contains(&PathBuf::from("usr/lib/ImageMagick-7.1.2")),
            "应上溯到运行时根 usr/lib/ImageMagick-7.1.2,实得 {dirs:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn infer_runtime_dirs_ignores_plain_lib() {
        // 普通包(少量 .so、无插件关键词目录)不应被误纳。
        let root = std::env::temp_dir().join(format!("aevum-infer-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let lib = root.join("usr/lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("libfoo.so.1"), b"x").unwrap();
        std::fs::write(lib.join("libbar.so.1"), b"x").unwrap();
        let dirs = infer_runtime_dirs(&root);
        assert!(dirs.is_empty(), "普通 lib 目录不应被推断,实得 {dirs:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_source_diagnostic_recorded() {
        // 块4:Arch 包的库从 Debian 宿主解出 → 记为 cross_source(诊断,不阻断)。
        // 构造一个假"包"+假"宿主库",主二进制 NEEDED 指向宿主库。
        // 注:需真实 ELF 才能解 NEEDED,这里只验证 resolver 层的 provenance 透传 + 跨源判定。
        let root = std::env::temp_dir().join(format!("aevum-xsrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let host = root.join("host");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::write(host.join("libc.so.6"), b"host-libc").unwrap();

        // chain:包内(Arch,空)→ 宿主(Debian,有 libc)
        let chain = ChainResolver::new(vec![
            Box::new(PackageLibResolver::with_source(vec![], Source::Arch)),
            Box::new(HostLibResolver::with_dirs(vec![host.clone()])),
        ]);
        // 解 libc:包内没有 → 宿主命中,provenance=Debian
        let (path, prov) = chain
            .resolve_with_provenance("libc.so.6", &Source::Arch)
            .expect("应宿主命中");
        assert_eq!(std::fs::read(&path).unwrap(), b"host-libc");
        assert_eq!(prov, Some(Source::Debian), "应透传宿主源 Debian");
        // 期望源 Arch ≠ 命中源 Debian → 这就是跨源(build_closure_resolved 会记 cross_source)
        assert_ne!(prov.unwrap(), Source::Arch);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_source_no_cross_hit() {
        // 包内命中(Arch=Arch)不算跨源。
        let root = std::env::temp_dir().join(format!("aevum-samesrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pkg = root.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("libpython3.14.so.1.0"), b"pkg-lib").unwrap();
        let chain = ChainResolver::new(vec![
            Box::new(PackageLibResolver::with_source(vec![pkg.clone()], Source::Arch)),
            Box::new(HostLibResolver::with_dirs(vec![])),
        ]);
        let (_p, prov) = chain
            .resolve_with_provenance("libpython3.14.so.1.0", &Source::Arch)
            .expect("应包内命中");
        assert_eq!(prov, Some(Source::Arch), "包内命中源=Arch=包源,非跨源");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn source_routed_resolver_routes_by_source() {
        // 块3(机制演示,非真实多源):按 soname 期望源路由到对应源目录。
        let root = std::env::temp_dir().join(format!("aevum-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let arch_dir = root.join("arch");
        let nix_dir = root.join("nix");
        std::fs::create_dir_all(&arch_dir).unwrap();
        std::fs::create_dir_all(&nix_dir).unwrap();
        std::fs::write(arch_dir.join("liba.so.1"), b"arch-a").unwrap();
        std::fs::write(nix_dir.join("libn.so.1"), b"nix-n").unwrap();

        let routed = SourceRoutedResolver::new()
            .with_source_resolver(
                Source::Arch,
                Box::new(HostLibResolver::with_dirs(vec![arch_dir.clone()])),
            )
            .with_source_resolver(
                Source::Nix,
                Box::new(HostLibResolver::with_dirs(vec![nix_dir.clone()])),
            )
            .route("liba.so.1", Source::Arch)
            .route("libn.so.1", Source::Nix);

        // liba 路由到 Arch 源,provenance=Arch(注:HostLibResolver provenance 是 Debian,
        // 但 routed 用路由目标源标注 provenance)。
        let (pa, pva) = routed
            .resolve_with_provenance("liba.so.1", &Source::Arch)
            .expect("liba 应路由命中");
        assert_eq!(std::fs::read(&pa).unwrap(), b"arch-a");
        assert_eq!(pva, Some(Source::Arch));
        // libn 路由到 Nix 源
        let (pn, pvn) = routed
            .resolve_with_provenance("libn.so.1", &Source::Nix)
            .expect("libn 应路由命中");
        assert_eq!(std::fs::read(&pn).unwrap(), b"nix-n");
        assert_eq!(pvn, Some(Source::Nix));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn strict_policy_blocks_cross_source() {
        // 块3:Strict 模式下,库来自异于包 source 的源 → 硬阻断 Err(CrossSource)。
        // 构造:包(Arch)无 main_binary,但 needed_libs 经 build_closure 来自扫描——
        // 这里直接构造一个 PackageInput 让 BFS 解析到宿主(Debian)库,验证 Strict 报错。
        let root = std::env::temp_dir().join(format!("aevum-strict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // 用宿主 resolver(Debian)解一个库;包 source=Arch → 跨源。
        // 但 build_closure 需要 needed_libs 来源(ELF 扫描),空包 needed 为空,
        // 故此处用单元级:直接验证 resolver 层 + 策略判定逻辑已由 build_closure_resolved_with_policy 覆盖。
        // 真实 Strict 阻断在 milestone4.rs 用真 rg(Arch 包+Debian 宿主库)验证。
        // 这里仅确认 Lenient(默认)不 Err、空包不崩。
        let input = PackageInput {
            name: "empty".into(),
            source: Source::Arch,
            root: root.clone(),
            main_binary: None,
            runtime_dirs: vec![],
            data_dirs: vec![],
        };
        let host = HostLibResolver::with_dirs(vec![]);
        // Lenient + Strict 对空包都应 Ok(无库可跨源)。
        assert!(build_closure_resolved_with_policy(&input, &host, SourcePolicy::Lenient).is_ok());
        assert!(build_closure_resolved_with_policy(&input, &host, SourcePolicy::Strict).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    // 真实复杂包(python/imagemagick)的全包 ELF 扫描 + dlopen 闭包完整性测试
    // 在 WSL/真 Linux 跑,fixture 用 poc/poc5-complex-pkg/ 的数据。这是里程碑2 的验收点。
    // build_closure_resolved 的 BFS 递归 + interpreter 解析需真实 ELF,
    // 用真实 rg 在 tests/milestone1.rs(WSL)验证。

    // ── P1-11:Strict 跨源**硬阻断**此前从未被真正断言(旧测试只验空包 Ok)──
    // 用 cc 编译带 DT_NEEDED 的真 .so 让 build_closure 填充 needed_libs,
    // 再用返回跨源 provenance 的 fake resolver 驱动 BFS,断言 Strict→Err、Lenient→记录。

    #[cfg(unix)]
    fn have_cc() -> bool {
        std::process::Command::new("cc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// fake resolver:任何 soname 都"解析成功",但 provenance 永远是 Debian。
    /// 配合 input.source=Arch → 必跨源。无需真实库文件(只看 provenance 判定)。
    #[cfg(unix)]
    struct CrossSourceResolver {
        fixed_path: PathBuf,
    }
    #[cfg(unix)]
    impl LibResolver for CrossSourceResolver {
        fn resolve_in_source(&self, _soname: &str, _source: &Source) -> Option<PathBuf> {
            Some(self.fixed_path.clone())
        }
        fn provenance(&self) -> Option<Source> {
            Some(Source::Debian) // 命中源恒为 Debian
        }
    }

    #[cfg(unix)]
    #[test]
    fn strict_blocks_real_cross_source_lenient_records() {
        if !have_cc() {
            eprintln!("SKIP strict_blocks_real_cross_source: 无 cc");
            return;
        }
        let root = std::env::temp_dir().join(format!("aevum-strict-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        // 编译一个依赖 libm 的真可执行,放进包 root → build_closure 扫到 DT_NEEDED(libm.so.6)。
        let src = root.join("m.c");
        std::fs::write(&src, "#include <math.h>\nint main(int c,char**v){return (int)sqrt((double)c);}\n").unwrap();
        let bin = root.join("usr/bin/mathprog");
        let ok = std::process::Command::new("cc").arg(&src).arg("-lm").arg("-o").arg(&bin).status().unwrap();
        assert!(ok.success(), "cc 编译失败");

        let input = PackageInput {
            name: "mathprog".into(),
            source: Source::Arch, // 包是 Arch
            root: root.clone(),
            main_binary: Some(PathBuf::from("usr/bin/mathprog")),
            runtime_dirs: vec![],
            data_dirs: vec![],
        };
        // resolver 把 libm 解析成"来自 Debian"→ 跨源(Arch ≠ Debian)。
        let resolver = CrossSourceResolver { fixed_path: bin.clone() };

        // Strict:必须硬阻断。
        let strict = build_closure_resolved_with_policy(&input, &resolver, SourcePolicy::Strict);
        assert!(
            matches!(strict, Err(ClosureError::CrossSource { .. })),
            "Strict 下跨源库必须 Err(CrossSource),实得 {strict:?}"
        );

        // Lenient:不阻断,但记 cross_source 诊断。
        let lenient = build_closure_resolved_with_policy(&input, &resolver, SourcePolicy::Lenient).unwrap();
        assert!(
            !lenient.cross_source.is_empty(),
            "Lenient 下跨源应被记入 cross_source 诊断"
        );
        assert!(lenient.cross_source.iter().any(|h| h.want == Source::Arch && h.got == Source::Debian));
        let _ = std::fs::remove_dir_all(&root);
    }
}
