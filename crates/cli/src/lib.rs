//! Aevum 库级编排:把 closure-builder / store / generation 串成可工作闭环。
//!
//! `main.rs`(bin)是这层的薄封装;`tests/milestone1.rs` 也调这层做端到端验收。
//!
//! # 路径约定
//! 所有状态放在 `$AEVUM_ROOT`(默认 `./.aevum`)下:
//! - `store/`        内容寻址存储
//! - `generations/`  世代 + active 指针
//! - `unpacked/<pkg>/` 解包后的包目录(由 scripts/prep-rg.sh 准备)

use std::path::{Path, PathBuf};

use aevum_closure_builder::{
    build_closure_resolved, ChainResolver, HostLibResolver, PackageInput, PackageLibResolver,
    Source,
};
use aevum_generation::{GenerationManager, PackageRef};
use aevum_store::{FileMeta, IngestedEntry, Store};

/// 派生 Aevum 的各状态目录(基于 `$AEVUM_ROOT`,默认 `./.aevum`)。
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn from_env() -> Self {
        let root = std::env::var_os("AEVUM_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./.aevum"));
        Layout { root }
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Layout { root: root.into() }
    }

    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }
    pub fn generations_dir(&self) -> PathBuf {
        self.root.join("generations")
    }
    pub fn unpacked_dir(&self, pkg: &str) -> PathBuf {
        self.root.join("unpacked").join(pkg)
    }
    /// 真实 Debian 索引(由 prep-index.sh 解压到此)。
    pub fn index_file(&self) -> PathBuf {
        self.root.join("index").join("Packages")
    }
    /// 求解产出的 lock 文件目录。
    pub fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }
    /// 模板目录(`<root>/templates/<name>.toml`,TS/TOML 前端共享的蓝图源)。
    pub fn templates_dir(&self) -> PathBuf {
        self.root.join("templates")
    }
    /// profile 目录(`<root>/profile/bin`):稳定 PATH 入口,symlink 指向 active 世代的可执行文件。
    /// 用户一次性 `export PATH="$AEVUM_ROOT/profile/bin:$PATH"` 加进 shell,之后 switch 自动生效。
    pub fn profile_bin_dir(&self) -> PathBuf {
        self.root.join("profile").join("bin")
    }
    /// 阶段3 可引导镜像构建目录(build-bootimage.sh 产 stage/ 与磁盘镜像)。
    pub fn boot_dir(&self) -> PathBuf {
        self.root.join("boot3-build")
    }
    /// bootloader 菜单配置源(stage/syslinux.cfg)。
    /// 引擎管理这份"配置源",switch/rollback 改它的 DEFAULT;脚本负责把它塞进 FAT 镜像。
    pub fn boot_menu_cfg(&self) -> PathBuf {
        self.boot_dir().join("stage").join("syslinux.cfg")
    }
}

/// 在轻隔离下运行一个二进制(PoC-2 直译):显式 loader + `--library-path`,
/// 不改 ELF、不依赖宿主标准 `/lib`。
///
/// `lib_dirs` 是各库所在的 store 对象目录(目录内文件名 == soname,ld 按 soname 命中)。
/// 命令形如 `<loader> --library-path <d1:d2:...> <bin> <args...>`。
///
/// unix 专有;非 unix 返回 [`std::io::Error`](Unsupported)。
#[cfg(unix)]
pub fn run_isolated(
    loader: &Path,
    bin: &Path,
    lib_dirs: &[PathBuf],
    args: &[&str],
) -> std::io::Result<std::process::Output> {
    let lib_path = lib_dirs
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    std::process::Command::new(loader)
        .arg("--library-path")
        .arg(&lib_path)
        .arg(bin)
        .args(args)
        .output()
}

#[cfg(not(unix))]
pub fn run_isolated(
    _loader: &Path,
    _bin: &Path,
    _lib_dirs: &[PathBuf],
    _args: &[&str],
) -> std::io::Result<std::process::Output> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "run_isolated 需要 unix(显式 loader + --library-path)。请在 Linux/WSL 运行",
    ))
}

/// 补闭包产出的、可入库的一组离散文件(rg 二进制 + 解析到的库 + loader)。
#[derive(Debug)]
pub struct BuiltClosure {
    /// 主二进制的宿主路径与逻辑名。
    pub main: (String, PathBuf),
    /// soname → 真实库文件路径。
    pub libs: Vec<(String, PathBuf)>,
    /// 动态链接器文件路径(若有)。
    pub interpreter: Option<PathBuf>,
    /// 同源内缺失的 soname(成本信号)。
    pub missing: Vec<String>,
    /// 元数据声明的运行时目录(绝对路径),整体纳入闭包(PoC-5 源3/4)。
    pub included_dirs: Vec<PathBuf>,
    /// 扫描的 ELF 总数(诊断:验证扫了全包而非只主二进制,PoC-5)。
    pub scanned_elf_count: usize,
}

/// 对一个已解包的包目录补闭包,解析到真实库文件(里程碑1:简单包,宿主作库源)。
pub fn build(
    layout: &Layout,
    pkg: &str,
    main_binary_rel: &Path,
) -> Result<BuiltClosure, Box<dyn std::error::Error>> {
    build_with(layout, pkg, main_binary_rel, &[])
}

/// 补闭包,支持复杂包(里程碑2):
/// - `runtime_dirs`:元数据声明的运行时目录(相对包根,如 `usr/lib/python3.14`),整体纳入闭包;
/// - 库解析用 [`ChainResolver`]:**包内优先**(libpython 在包里),宿主兜底(PoC-5)。
///
/// 包内库搜索路径自动包含:`usr/lib`、`usr/lib64`,以及各 runtime_dir 与其 `lib-dynload` 子目录。
pub fn build_with(
    layout: &Layout,
    pkg: &str,
    main_binary_rel: &Path,
    runtime_dirs: &[PathBuf],
) -> Result<BuiltClosure, Box<dyn std::error::Error>> {
    let root = layout.unpacked_dir(pkg);

    // runtime_dirs 为空 → 自动推断(块1,PoC-5 元数据来源:布局启发式)。显式传入则尊重。
    let runtime_dirs: Vec<PathBuf> = if runtime_dirs.is_empty() {
        aevum_closure_builder::infer_runtime_dirs(&root)
    } else {
        runtime_dirs.to_vec()
    };
    let runtime_dirs = &runtime_dirs[..];

    // 包内库搜索路径:标准 lib + 各运行时目录(及其 lib-dynload,python 扩展所在)。
    let mut pkg_lib_dirs = vec![root.join("usr/lib"), root.join("usr/lib64")];
    for rd in runtime_dirs {
        let abs = root.join(rd);
        pkg_lib_dirs.push(abs.join("lib-dynload"));
        pkg_lib_dirs.push(abs);
    }

    let input = PackageInput {
        name: pkg.to_string(),
        source: Source::Arch,
        root: root.clone(),
        main_binary: Some(main_binary_rel.to_path_buf()),
        runtime_dirs: runtime_dirs.to_vec(),
        data_dirs: vec![],
    };
    // 包内优先、宿主兜底(PoC-5:复杂包自带核心库)。
    let resolver = ChainResolver::new(vec![
        Box::new(PackageLibResolver::new(pkg_lib_dirs)),
        Box::new(HostLibResolver::new()),
    ]);
    let closure = build_closure_resolved(&input, &resolver)?;

    let main_abs = root.join(main_binary_rel);
    let main_name = main_binary_rel
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(pkg)
        .to_string();

    Ok(BuiltClosure {
        main: (main_name, main_abs),
        libs: closure.resolved_libs.into_iter().collect::<Vec<_>>(),
        interpreter: closure.interpreter,
        missing: closure.missing_libs.into_iter().collect(),
        included_dirs: closure.included_dirs.into_iter().collect(),
        scanned_elf_count: closure.scanned_elf_count,
    })
}

/// 把补闭包产出的离散文件内容寻址入库,返回可用于造世代的 [`PackageRef`] 列表
/// 与各对象的 store 目录(供轻隔离运行拼 `--library-path`)。
///
/// 名字策略:库以其 **soname** 为对象名(ld 按 soname 找,目录内文件名须等于 soname);
/// loader 以其 basename;主二进制以逻辑名。权限位读真实 mode(PoC-6,不硬编码)。
pub fn ingest_closure(
    store: &Store,
    built: &BuiltClosure,
) -> Result<IngestResult, Box<dyn std::error::Error>> {
    let mut refs = Vec::new();
    let mut lib_dirs = Vec::new();

    // 主二进制
    let (main_name, main_path) = &built.main;
    let main_dir = put_file(store, main_name, main_path)?;
    let main_obj = obj_id(&main_dir);

    // 库(以 soname 为名)
    for (soname, path) in &built.libs {
        let dir = put_file(store, soname, path)?;
        lib_dirs.push(dir.clone());
        refs.push(PackageRef {
            name: soname.clone(),
            object_id: obj_id(&dir),
            store_dir: dir,
            rel_path: Some(PathBuf::from("lib").join(soname)),
        });
    }

    // loader(以 basename 为名)
    let interpreter_dir = match &built.interpreter {
        Some(p) => {
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("ld.so")
                .to_string();
            let dir = put_file(store, &name, p)?;
            refs.push(PackageRef {
                name: name.clone(),
                object_id: obj_id(&dir),
                store_dir: dir.clone(),
                rel_path: Some(PathBuf::from("lib").join(&name)),
            });
            Some(dir)
        }
        None => None,
    };

    // 主二进制 ref 放最后,保持 refs 完整
    refs.push(PackageRef {
        name: main_name.clone(),
        object_id: main_obj.clone(),
        store_dir: main_dir.clone(),
        rel_path: Some(PathBuf::from("bin").join(&main_name)),
    });

    // —— 源3/4:运行时目录整体内容寻址入库(PoC-5:.py 标准库 + lib-dynload 软链)——
    // 用 ingest_dir 保留布局与符号链接(软链不解引用、不翻倍)。
    let mut runtime_objs = Vec::new();
    for dir in &built.included_dirs {
        if !dir.exists() {
            continue;
        }
        let entries = store.ingest_dir(dir)?;
        for e in &entries {
            // 每个运行时对象也作为世代引用,纳入 GC 可达性;保留其相对布局。
            refs.push(PackageRef {
                name: e
                    .rel_path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.object_id.clone()),
                object_id: e.object_id.clone(),
                store_dir: e.store_dir.clone(),
                rel_path: Some(e.rel_path.clone()),
            });
        }
        runtime_objs.extend(entries);
    }

    Ok(IngestResult {
        refs,
        main_store_dir: main_dir,
        main_name: main_name.clone(),
        lib_dirs,
        interpreter_dir,
        runtime_objs,
    })
}

/// [`ingest_closure`] 的产物。
#[derive(Debug)]
pub struct IngestResult {
    /// 造世代用的全部包引用。
    pub refs: Vec<PackageRef>,
    /// 主二进制的 store 对象目录。
    pub main_store_dir: PathBuf,
    pub main_name: String,
    /// 各库对象目录(拼 `--library-path`)。
    pub lib_dirs: Vec<PathBuf>,
    /// loader 对象目录(若有)。
    pub interpreter_dir: Option<PathBuf>,
    /// 运行时目录整体入库的条目(PoC-5 源3/4:.py 标准库 + lib-dynload 扩展,保留布局/软链)。
    pub runtime_objs: Vec<IngestedEntry>,
}

/// 读真实 mode 把一个普通文件内容寻址入库(PoC-6:权限位纳入哈希并恢复)。
fn put_file(store: &Store, name: &str, path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let content = std::fs::read(path)?;
    // 注意:`store.put` 用 `fs::write` 把 content 实体化入库(永远是实体文件,非 symlink)。
    // 故 meta.is_symlink 必须为 false——否则 put 用 is_symlink=true 算 hash,
    // 而 get 重算时对象是实体文件(is_symlink=false),hash 必失配(verify 完整性会抓到)。
    // `built.interpreter`/库路径常是解包目录里的 symlink,`fs::read` 已解引用读到目标内容;
    // 权限位也必须取**解引用后目标文件**的 mode(P1-19):symlink 自身总是 0o777,
    // 若按 symlink_metadata 取就丢 setuid(PoC-6 铁律:setuid 必须保留)。
    // 用 fs::metadata(path)(跟随 symlink)而非 symlink_metadata(不跟随)。
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o7777)
            .unwrap_or(0o755)
    };
    #[cfg(not(unix))]
    let mode = 0o755;
    let meta = FileMeta { mode, is_symlink: false };
    Ok(store.put(name, &content, meta)?)
}

fn obj_id(dir: &Path) -> String {
    dir.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 打开该 layout 的 store。
pub fn open_store(layout: &Layout) -> Result<Store, Box<dyn std::error::Error>> {
    Ok(Store::open(layout.store_dir())?)
}

/// 打开该 layout 的世代管理器。
pub fn open_generations(
    layout: &Layout,
) -> Result<GenerationManager, Box<dyn std::error::Error>> {
    Ok(GenerationManager::open(layout.generations_dir())?)
}

/// 记录某世代由哪个 lock 构建:写 `generations/gen-<id>/source-lock.txt`(内容为 lock 名)。
///
/// 让 `list`/`remove` 能问"**当前 active 世代**用的是哪个 lock",而不是瞎猜"最近改的 lock"。
/// 后者在 rollback/switch 后会撒谎(active 是旧世代,但最新 lock 是新装的那个)。
/// 与 GC 用的 `lock.txt`(存 object_id)分开命名,互不干扰。纯文本、跨平台。
pub fn record_generation_lock(
    layout: &Layout,
    gen_id: u64,
    lock_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let gens = open_generations(layout)?;
    let gen_dir = gens.generation_dir(gen_id);
    std::fs::create_dir_all(&gen_dir)?;
    std::fs::write(gen_dir.join("source-lock.txt"), lock_name)?;
    Ok(())
}

/// 解析**当前 active 世代**对应的 lock 名。
///
/// 1. 读 active 世代 id;无 active 世代 → `Ok(None)`。
/// 2. 读 `gen-<id>/source-lock.txt`(本轮起 install 会写)。
/// 3. 兼容旧世代(无该文件):回退到 `locks/` 里最近修改的 lock 名(老行为),
///    但只在拿不到 active 指针时才彻底无解。
pub fn active_lock_name(
    layout: &Layout,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let gens = open_generations(layout)?;
    let Some(active_id) = gens.active_generation()? else {
        return Ok(None);
    };
    let ptr = gens.generation_dir(active_id).join("source-lock.txt");
    if let Ok(name) = std::fs::read_to_string(&ptr) {
        let name = name.trim();
        if !name.is_empty() {
            return Ok(Some(name.to_string()));
        }
    }
    // 旧世代无指针:回退到 mtime 最新的 lock(尽力而为,带告警由调用方决定)。
    Ok(latest_lock_name(layout))
}

/// 回退用:`locks/` 里最近修改的 lock 名(不含 `.lock` 后缀)。无则 None。
pub fn latest_lock_name(layout: &Layout) -> Option<String> {
    let lock_dir = layout.locks_dir();
    if !lock_dir.is_dir() {
        return None;
    }
    let mut locks: Vec<_> = std::fs::read_dir(&lock_dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "lock").unwrap_or(false))
        .collect();
    locks.sort_by_key(|e| {
        std::cmp::Reverse(
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        )
    });
    locks
        .first()
        .and_then(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
}

/// 从 store 对象重建包内运行视图(块3:证明 store 是可运行的真相源)。
///
/// 按每个 [`IngestedEntry`] 的 `rel_path` 在 `dest` 重建包内布局,
/// 每个文件以 **symlink** 指回 store 对象里的真实文件(`<hash>-<name>/<name>`)。
/// 这样运行时(PYTHONHOME / MAGICK_PATH)指向 `dest` 即可,内容全部来自 store——
/// 不依赖解包目录,验证 store 内容寻址 + 布局重建可支撑真实运行。
///
/// # PoC-5 铁律
/// 视图用 symlink 指回 store,不复制(复杂包 137 软链/大标准库,复制会爆量)。
/// store 内的 symlink 对象其 target 即原始链接目标,重建时同样建 symlink(布局保真)。
///
/// unix 专有(symlink);非 unix 返回 [`std::io::Error`](Unsupported)。
#[cfg(unix)]
pub fn materialize_view(
    entries: &[IngestedEntry],
    dest: impl AsRef<Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dest = dest.as_ref().to_path_buf();
    // 确定性:按 rel_path 排序重建。
    let mut sorted: Vec<&IngestedEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    for e in sorted {
        let link = dest.join(&e.rel_path);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // store 对象里的真实文件名 = rel_path 的文件名(put/put_symlink 以 name 存)。
        let fname = e
            .rel_path
            .file_name()
            .ok_or("ingested entry 无文件名")?;
        let target = e.store_dir.join(fname);
        if link.exists() || std::fs::symlink_metadata(&link).is_ok() {
            let _ = std::fs::remove_file(&link);
        }
        std::os::unix::fs::symlink(&target, &link)?;
    }
    Ok(dest)
}

#[cfg(not(unix))]
pub fn materialize_view(
    _entries: &[IngestedEntry],
    _dest: impl AsRef<Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Err("materialize_view 需要 unix(symlink 视图)。请在 Linux/WSL 运行".into())
}

/// repair 方案C 运行时视图隔离(ai/02 §3.2):为多个**冲突共存**的软件各建一个私有依赖视图。
///
/// 这是"保留两份"的落地核心——每个 app 一个独立视图目录(`base_dest/<app>`),
/// 视图里它的依赖 symlink 各指向 **它该用的那一版** store 对象。于是两个 app 的同名依赖
/// (如 `libfoo.so.1`)在各自视图里指向不同 hash 的对象 → 同机共存、互不可见(语义契约见 ai/02 §3.2)。
///
/// 配合 [`run_isolated`]:用 app 私有视图目录作 `--library-path`,ld 按 soname 命中的就是该 app 那版库。
///
/// `views` 是 `(app 逻辑名, 该 app 的依赖条目集)` 列表;每个 app 的视图按其 entry 的 rel_path 重建。
/// 返回各 app 视图目录(与 `views` 同序)。任一 app 的逻辑名含路径分隔会被拒(防逃逸)。
///
/// unix 专有(symlink 视图)。
#[cfg(unix)]
pub fn materialize_isolated_views(
    views: &[(String, Vec<IngestedEntry>)],
    base_dest: impl AsRef<Path>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let base = base_dest.as_ref();
    let mut out = Vec::with_capacity(views.len());
    for (app, entries) in views {
        // app 名作子目录:拒绝路径分隔/`..`,防视图逃逸到 base 之外。
        if app.is_empty() || app.contains('/') || app.contains('\\') || app.contains("..") {
            return Err(format!("非法 app 视图名(含路径分隔或 ..): {app:?}").into());
        }
        let dest = base.join(app);
        // 每个 app 独立视图:复用 materialize_view 的"按 rel_path symlink 回 store"逻辑。
        let dir = materialize_view(entries, &dest)?;
        out.push(dir);
    }
    Ok(out)
}

#[cfg(not(unix))]
pub fn materialize_isolated_views(
    _views: &[(String, Vec<IngestedEntry>)],
    _base_dest: impl AsRef<Path>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    Err("materialize_isolated_views 需要 unix(symlink 视图)。请在 Linux/WSL 运行".into())
}

/// repair 方案C 世代级集成(旁路):把"保留两份"的私有依赖视图挂进一个已造好的世代。
///
/// 不改世代共享布局(`packages/`)——而在世代下新增旁路子树 `gen-NNN/private-views/<app>/`,
/// 每个冲突 app 一个私有视图(其冲突依赖各指向各自版本的 store 对象)。并写
/// `gen-NNN/keep-two.txt` 记录哪些 app 有私有视图(供运行期/审计:该 app 用私有视图作 `--library-path`)。
///
/// 这样现有世代/bootroot/GC 不受影响(它们只看 `packages/`),而"保留两份"的产物随世代走、可回滚。
/// `views` 是 `(app 逻辑名, 该 app 的私有依赖条目集)`。返回各 app 私有视图目录。unix 专有。
#[cfg(unix)]
pub fn attach_keep_two_views(
    layout: &Layout,
    gen_id: u64,
    views: &[(String, Vec<IngestedEntry>)],
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let gens = open_generations(layout)?;
    let gen_dir = gens.generation_dir(gen_id);
    if !gen_dir.exists() {
        return Err(format!("世代 gen-{gen_id:03} 不存在,无法挂私有视图").into());
    }
    let base = gen_dir.join("private-views");
    let dirs = materialize_isolated_views(views, &base)?;
    // 记录哪些 app 有私有视图(每行一个 app 名),供运行期/审计。
    let manifest: String = views
        .iter()
        .map(|(app, _)| app.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(gen_dir.join("keep-two.txt"), manifest)?;
    // 私有视图引用的 store 对象 id 写进 private-objects.txt → 纳入 GC 可达性,防误回收。
    let mut obj_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (_app, entries) in views {
        for e in entries {
            obj_ids.insert(e.object_id.clone());
        }
    }
    std::fs::write(
        gen_dir.join("private-objects.txt"),
        obj_ids.into_iter().collect::<Vec<_>>().join("\n"),
    )?;
    Ok(dirs)
}

#[cfg(not(unix))]
pub fn attach_keep_two_views(
    _layout: &Layout,
    _gen_id: u64,
    _views: &[(String, Vec<IngestedEntry>)],
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    Err("attach_keep_two_views 需要 unix(symlink 视图)。请在 Linux/WSL 运行".into())
}

/// repair 方案C 运行期:取某 app 在某世代的私有依赖视图目录(`gen-NNN/private-views/<app>`)。
///
/// 返回 `Some(dir)` 当且仅当该私有视图存在(即该 app 在此世代被"保留两份")。
/// 该目录可直接作 `run_isolated` 的 `--library-path`:ld 按 soname 命中的就是该 app 那版库。
/// 跨平台(纯路径检查)。
pub fn private_view_dir(layout: &Layout, gen_id: u64, app: &str) -> Option<PathBuf> {
    if app.is_empty() || app.contains('/') || app.contains('\\') || app.contains("..") {
        return None;
    }
    let gens = open_generations(layout).ok()?;
    let dir = gens.generation_dir(gen_id).join("private-views").join(app);
    if dir.is_dir() {
        Some(dir)
    } else {
        None
    }
}

/// 旁路记录某世代选用的模板及版本(验收7,模板模型 §)。
///
/// 写 `gen-NNN/templates.txt`,每行 `name@version`(按名排序,确定性)。**不动 make_generation 签名**
/// (跟随第46轮 keep-two.txt 先例:世代共享布局/lock/GC 只看 packages/,旁路文件不干扰)。
/// 纯审计:重放/可复现仍只来自 lock,本文件供"这个世代用了哪些模板"的追溯。跨平台(纯文本)。
pub fn record_generation_templates(
    layout: &Layout,
    gen_id: u64,
    templates: &[(String, String)],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let gens = open_generations(layout)?;
    let gen_dir = gens.generation_dir(gen_id);
    std::fs::create_dir_all(&gen_dir)?;
    let mut lines: Vec<String> = templates.iter().map(|(n, v)| format!("{n}@{v}")).collect();
    lines.sort();
    let path = gen_dir.join("templates.txt");
    std::fs::write(&path, lines.join("\n"))?;
    Ok(path)
}

/// 读回 `gen-NNN/templates.txt` 为 `(name, version)` 列表(无文件则空)。
pub fn read_generation_templates(layout: &Layout, gen_id: u64) -> Vec<(String, String)> {
    let Ok(gens) = open_generations(layout) else { return Vec::new() };
    let path = gens.generation_dir(gen_id).join("templates.txt");
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_once('@').map(|(n, v)| (n.to_string(), v.to_string())))
        .collect()
}

/// repair 方案C 运行期:用某 app 的私有视图作 `--library-path` 运行其二进制(ai/02 §3.2 落地闭环)。
///
/// 这是"保留两份各跑各的"的运行入口:`bin` 用 `private-views/<app>/` 里那版库,而非世代共享布局——
/// 于是同机两个冲突 app 各自 `run_app_isolated` 时,各加载到各自版本的依赖,互不可见。
///
/// `loader` 是动态链接器路径,`bin` 是要运行的主二进制,`args` 透传给它。
/// 该 app 无私有视图(未被保留两份)时返回 Err(调用方应回退普通运行路径)。unix 专有。
#[cfg(unix)]
pub fn run_app_isolated(
    layout: &Layout,
    gen_id: u64,
    app: &str,
    loader: &Path,
    bin: &Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let view = private_view_dir(layout, gen_id, app)
        .ok_or_else(|| format!("app {app:?} 在 gen-{gen_id:03} 无私有视图(未保留两份)"))?;
    Ok(run_isolated(loader, bin, &[view], args)?)
}

#[cfg(not(unix))]
pub fn run_app_isolated(
    _layout: &Layout,
    _gen_id: u64,
    _app: &str,
    _loader: &Path,
    _bin: &Path,
    _args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Err("run_app_isolated 需要 unix(显式 loader + --library-path)。请在 Linux/WSL 运行".into())
}

/// 确定性求解一组顶层包的依赖闭包,产出并写 lock(里程碑5:接真实 Debian 索引)。
///
/// 兑现 ADR-0003:AI 产意图(这里是顶层包名),**确定性求解器算闭包并产 lock**,
/// 可复现只来自 lock。读 `$AEVUM_ROOT/index/Packages`(prep-index.sh 解压的真实索引),
/// `aevum_solver::resolve` → `build_lock` → 写 `$AEVUM_ROOT/locks/<name>.lock`(纯文本)。
///
/// 跨平台(纯文本处理,无 unix 语义)。
pub fn resolve(
    layout: &Layout,
    template_pkgs: &[String],
    lock_name: &str,
) -> Result<aevum_solver::Lock, Box<dyn std::error::Error>> {
    let constraints: Vec<aevum_solver::Constraint> = template_pkgs
        .iter()
        .map(aevum_solver::Constraint::unconstrained)
        .collect();
    // 显式包名:无 AI 介入。
    resolve_constraints(layout, &constraints, lock_name, None)
}

/// 走 AI 增强层求解(里程碑6):意图 → IntentResolver 翻译成约束 → 确定性求解。
///
/// `resolver` 翻译失败(模型不可用/无匹配)返回 Err,调用方可降级到 [`resolve`](显式包名)。
/// AI 是否参与及理由写进 lock 的 `ai_assist`,但**可复现只来自确定性闭包**(ADR-0005)。
pub fn resolve_intent(
    layout: &Layout,
    resolver: &dyn aevum_intent::IntentResolver,
    intent: &aevum_intent::Intent,
    lock_name: &str,
) -> Result<(aevum_solver::Lock, aevum_intent::AiAssist), Box<dyn std::error::Error>> {
    let outcome = resolver.resolve_intent(intent)?;
    let lock = resolve_constraints(layout, &outcome.constraints, lock_name, Some(&outcome.assist))?;
    Ok((lock, outcome.assist))
}

/// 求解约束集 → 写 lock(可选携带 ai_assist 审计记录)。
///
/// 公开供 CLI 分两步用:先 [`IntentResolver::resolve_intent`] 翻译并让用户确认约束,
/// 再调本函数求解写 lock(人在回路,ADR-0003 边界3)。
///
/// 不自动 repair(冲突仅检测+建议);要自动应用方案A 放宽见 [`resolve_constraints_opt`]。
pub fn resolve_constraints(
    layout: &Layout,
    constraints: &[aevum_solver::Constraint],
    lock_name: &str,
    assist: Option<&aevum_intent::AiAssist>,
) -> Result<aevum_solver::Lock, Box<dyn std::error::Error>> {
    resolve_constraints_opt(layout, constraints, lock_name, assist, false, None, None)
}

/// 求解约束集 → 写 lock,可选自动应用 repair 方案A(放宽约束求单一共存版本)。
///
/// `repair=true` 时用 [`aevum_solver::resolve_with_repair`]:对有单一共存版本的冲突包
/// 钉到该版本重求解,把"建议"变成"可用 lock";无单一共存版本的冲突仍留诊断(需 B/C)。
/// 应用的放宽写进 lock 的 `# applied-repair:` 段(审计)。
pub fn resolve_constraints_opt(
    layout: &Layout,
    constraints: &[aevum_solver::Constraint],
    lock_name: &str,
    assist: Option<&aevum_intent::AiAssist>,
    repair: bool,
    inputs: Option<&str>,
    templates: Option<&str>,
) -> Result<aevum_solver::Lock, Box<dyn std::error::Error>> {
    let index_path = layout.index_file();
    let text = std::fs::read_to_string(&index_path).map_err(|e| {
        format!("读索引失败 {}: {e}(先 `bash scripts/prep-index.sh`)", index_path.display())
    })?;
    let index = aevum_solver::Index::from_packages_str(&text);

    let overrides = std::collections::HashMap::new();
    // repair=true:循环放宽求解(方案A 自动应用);否则单次求解(仅检测+建议)。
    let (resolution, applied) = if repair {
        let r = aevum_solver::resolve_with_repair(&index, constraints, &overrides);
        (r.resolution, r.applied)
    } else {
        (aevum_solver::resolve(&index, constraints, &overrides), Vec::new())
    };
    let lock = aevum_solver::build_lock(resolution);

    // 写 lock(纯文本,不引 serde_json)。
    std::fs::create_dir_all(layout.locks_dir())?;
    let lock_path = layout.locks_dir().join(format!("{lock_name}.lock"));
    let mut out = String::new();
    out.push_str(&format!("closure_id: {}\n", lock.closure_id));
    out.push_str(&format!("package_count: {}\n", lock.package_count));
    out.push_str(&format!("unresolved: {}\n", lock.diagnostics.unresolved.len()));
    // 已应用的 repair 放宽(方案A 自动应用,审计)。
    for a in &applied {
        out.push_str(&format!("# applied-repair: {} 钉到 {}\n", a.package, a.pinned_version));
    }
    // 版本冲突(ai/02 repair 触发依据):写进 lock 诊断段,留痕可审计。
    out.push_str(&format!("conflicts: {}\n", lock.diagnostics.conflicts.len()));
    for c in &lock.diagnostics.conflicts {
        out.push_str(&format!(
            "# conflict: {} 已选 {},但 {} 要求 ({} {})\n",
            c.package, c.chosen_version, c.source, c.required_op, c.required_ver
        ));
    }
    // repair 方案A 建议(放宽约束求单一共存版本)。
    for s in &lock.diagnostics.repair_suggestions {
        match &s.satisfying_version {
            Some(v) => out.push_str(&format!("# repair-A: {} → 可放宽到 {}\n", s.package, v)),
            None => out.push_str(&format!("# repair-A: {} → 无单一共存版本(需 B/C)\n", s.package)),
        }
    }
    // repair 方案B 建议(升级父包求兼容)。
    for b in &lock.diagnostics.repair_suggestions_b {
        out.push_str(&format!(
            "# repair-B: 升级 {} 到 {} → {} 取 {}\n",
            b.parent, b.upgrade_parent_to, b.dependency, b.dependency_version
        ));
    }
    // repair 方案C(保留两份)建议。
    for c in &lock.diagnostics.keep_two_suggestions {
        out.push_str(&format!(
            "# repair-C: {} 保留两份 {} 与 {}(需确认)\n",
            c.package, c.version_a, c.version_b
        ));
    }
    // repair 方案D(隔离失败,需用户取舍)。
    for u in &lock.diagnostics.unrepairable {
        out.push_str(&format!("# repair-D: {} 无法共存,需用户取舍(约束 {:?})\n", u.package, u.constraints));
    }
    // ai_assist 行(ADR-0005:记录 AI 参与,但重放不依赖它)。
    match assist {
        Some(a) => out.push_str(&format!(
            "ai_assist: involved={} model={} reason={}\n",
            a.ai_involved, a.model_id, a.reason
        )),
        None => out.push_str("ai_assist: involved=false model=none\n"),
    }
    // ts_inputs 行(ADR-0004:TS 前端的显式输入记入 lock 供审计/可复现追溯)。
    // 纯审计:不进 closure_id(closure_id 只是 resolved 包集摘要),重放暂不消费(见 CHANGELOG 边界)。
    // 单行化(替换 \n\r\t 为空格):lock 按行解析,值含换行/制表会破坏头部解析。
    match inputs {
        Some(s) => {
            let one_line = s.replace(['\n', '\r', '\t'], " ");
            out.push_str(&format!("ts_inputs: {one_line}\n"));
        }
        None => out.push_str("ts_inputs: none\n"),
    }
    // templates 行(模板系统:本 lock 选用的模板及版本,name@version 逗号分隔)。
    // 同 ts_inputs 性质:纯审计,不进 closure_id;供 maintain 接入时写世代 templates.txt(验收7控制面)。
    match templates {
        Some(s) if !s.trim().is_empty() => {
            let one_line = s.replace(['\n', '\r', '\t'], " ");
            out.push_str(&format!("templates: {one_line}\n"));
        }
        _ => out.push_str("templates: none\n"),
    }
    out.push_str("---\n");
    for p in &lock.locked {
        // 行格式:name@version#fingerprint\tfilename(filename 供 install 下载)。
        out.push_str(&format!(
            "{}@{}#{}\t{}\n",
            p.name, p.version, p.fingerprint, p.filename
        ));
    }
    std::fs::write(&lock_path, out)?;
    Ok(lock)
}

// ───────────────────────── install:下载→解包→入库→世代 ─────────────────────────

use sha2::{Digest, Sha256};

/// 默认 Debian 镜像(CDN);国内慢可换 mirrors.ustc.edu.cn/debian 等。
pub const DEFAULT_MIRROR: &str = "http://deb.debian.org/debian";

/// 国内推荐镜像。
pub const MIRROR_USTC: &str = "http://mirrors.ustc.edu.cn/debian";

/// 从 `$AEVUM_ROOT/config.toml` 的 `[source] mirror` 读用户配置的镜像(P1-24)。
///
/// `aevum ai` 此前硬编码 USTC 镜像,中国境外用户得到慢/被阻下载且无从覆盖。
/// 现在:config.toml 有 `[source] mirror` 则用它;无则回退 `DEFAULT_MIRROR`(CDN,全球可达)。
pub fn configured_mirror(layout: &Layout) -> String {
    let config_path = layout.root.join("config.toml");
    if let Ok(text) = std::fs::read_to_string(&config_path) {
        // 极简 TOML 提取(不引 toml crate):找 `[source]` 段下的 `mirror = "..."`。
        let mut in_source = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_source = trimmed == "[source]";
                continue;
            }
            if in_source {
                if let Some(val) = trimmed.strip_prefix("mirror") {
                    let val = val.trim_start().strip_prefix('=').unwrap_or("").trim();
                    let val = val.trim_matches('"').trim_matches('\'');
                    if !val.is_empty() {
                        return val.to_string();
                    }
                }
            }
        }
    }
    DEFAULT_MIRROR.to_string()
}

/// 找当前最大世代 id + 1(用于 install 自动分配)。
pub fn next_generation_id(layout: &Layout) -> u64 {
    let gens_dir = layout.generations_dir();
    let mut max_id = 0u64;
    if let Ok(entries) = std::fs::read_dir(&gens_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(num_str) = name.strip_prefix("gen-") {
                    if let Ok(id) = num_str.parse::<u64>() {
                        max_id = max_id.max(id);
                    }
                }
            }
        }
    }
    max_id + 1
}

/// 从 fingerprint(`sha256:hex`)取出期望 hash;非 sha256 形态返回 None。
#[cfg(unix)]
fn expected_sha256(fingerprint: &str) -> Option<&str> {
    fingerprint.strip_prefix("sha256:")
}

/// 下载一个 .deb 并做 SHA256 内容寻址校验(供应链命脉:hash 不符即拒)。
///
/// `curl -sL <mirror>/<filename> -o <dest>`;下载后算 SHA256 比对 `expected`(若有)。
/// 幂等:目标已存在且校验通过则跳过下载。失配返回 Err(不将就)。
pub fn download_deb(
    mirror: &str,
    filename: &str,
    expected: Option<&str>,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // 已存在且校验通过 → 跳过。
    if dest.exists() {
        if let Some(exp) = expected {
            if file_sha256(dest)? == exp {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("{}/{}", mirror.trim_end_matches('/'), filename);
    let status = std::process::Command::new("curl")
        .arg("-sL")
        .arg("--connect-timeout")
        .arg("30")
        .arg("--max-time")
        .arg("120")
        // P1-21:瞬时网络抖动重试。数百包闭包里单次失败本会让 propose 从头重来。
        .arg("--retry")
        .arg("3")
        .arg("--retry-delay")
        .arg("2")
        .arg("--retry-connrefused")
        .arg("--fail")
        .arg(&url)
        .arg("-o")
        .arg(dest)
        .status()
        .map_err(|e| format!("curl 执行失败: {e}"))?;
    if !status.success() {
        return Err(format!("下载失败({}): {url}", status).into());
    }
    // 内容寻址校验。
    if let Some(exp) = expected {
        let got = file_sha256(dest)?;
        if got != exp {
            let _ = std::fs::remove_file(dest);
            return Err(format!(
                "SHA256 校验失败 {filename}: 期望 {exp}, 实得 {got}(供应链污染?)"
            )
            .into());
        }
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(hex::encode(h.finalize()))
}

/// 解包 .deb 到目录(取其中 data.tar.* 的内容)。unix 专有(用系统 ar/tar)。
///
/// .deb = ar 归档,内含 control.tar.* + data.tar.*;文件内容在 data.tar。
/// `ar x` 取 data.tar.* → `tar xf`(tar 自动识别 xz/zst/gz 压缩)解到 dest。
#[cfg(unix)]
pub fn unpack_deb(deb_path: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dest)?;
    let deb_abs = std::fs::canonicalize(deb_path)?;
    // ar t 列出成员,找 data.tar.*
    let listing = std::process::Command::new("ar")
        .arg("t")
        .arg(&deb_abs)
        .output()
        .map_err(|e| format!("ar 执行失败: {e}"))?;
    if !listing.status.success() {
        return Err(format!("ar t 失败: {}", String::from_utf8_lossy(&listing.stderr)).into());
    }
    let data_member = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .find(|l| l.trim().starts_with("data.tar"))
        .map(|l| l.trim().to_string())
        .ok_or("deb 内无 data.tar.*")?;

    // 在 dest 里 ar x 取出 data.tar.*(ar 解到当前目录,故先 cd dest)。
    let st = std::process::Command::new("ar")
        .arg("x")
        .arg(&deb_abs)
        .arg(&data_member)
        .current_dir(dest)
        .status()
        .map_err(|e| format!("ar x 失败: {e}"))?;
    if !st.success() {
        return Err("ar x 提取 data.tar 失败".into());
    }
    // tar 解 data.tar.*(-a 自动识别压缩),解完删 tar。
    let data_path = dest.join(&data_member);
    let st = std::process::Command::new("tar")
        .arg("xf")
        .arg(&data_path)
        .arg("-C")
        .arg(dest)
        .status()
        .map_err(|e| format!("tar 执行失败: {e}"))?;
    if !st.success() {
        return Err("tar 解 data.tar 失败".into());
    }
    let _ = std::fs::remove_file(&data_path);
    Ok(())
}

#[cfg(not(unix))]
pub fn unpack_deb(_deb_path: &Path, _dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Err("unpack_deb 需要 unix(系统 ar/tar)。请在 Linux/WSL 运行".into())
}

/// install 报告。
#[derive(Debug)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub generation: u64,
    pub store_objects: usize,
}

/// 把 lock 里**选定**的包真装进 store + 造世代(里程碑7:接通 resolve→install)。
///
/// `only` 限定真下载安装哪些包(默认行为:不装 455 包全集,避免过重 + 半成品问题)。
/// **propose**:下载 .deb(SHA256 校验)→ 解包 → ingest_dir 入库 → 补运行闭包 → make_generation。
/// **造候选世代但不激活**(对应世代状态机 propose:不触碰 active)。
///
/// 这是 install 与 maintain 主循环共用的"造世代"核心。install 在此之上 set_active;
/// maintain 在此之上走 verify 门禁再激活。unix 专有(解包/世代 symlink)。
#[cfg(unix)]
pub fn propose_generation(
    layout: &Layout,
    lock: &aevum_solver::Lock,
    mirror: &str,
    only: &[String],
    gen_id: u64,
) -> Result<InstallReport, Box<dyn std::error::Error>> {
    let targets: Vec<&aevum_solver::LockedPackage> = lock
        .locked
        .iter()
        .filter(|p| only.is_empty() || only.iter().any(|n| n == &p.name))
        .collect();
    if targets.is_empty() {
        return Err("lock 中无匹配 --only 的包".into());
    }

    let store = open_store(layout)?;
    let mut refs = Vec::new();
    let mut installed = Vec::new();

    for p in &targets {
        if p.filename.is_empty() {
            return Err(format!("{} 无 filename(无法下载)", p.name).into());
        }
        let deb = layout.root.join("cache").join(format!("{}.deb", p.name));
        download_deb(mirror, &p.filename, expected_sha256(&p.fingerprint), &deb)?;
        let unpacked = layout.unpacked_dir(&p.name);
        let _ = std::fs::remove_dir_all(&unpacked);
        unpack_deb(&deb, &unpacked)?;
        // 整包内容寻址入库(复用里程碑3 ingest_dir,保留布局/软链/权限)。
        let entries = store.ingest_dir(&unpacked)?;
        for e in &entries {
            refs.push(PackageRef {
                name: e
                    .rel_path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.object_id.clone()),
                object_id: e.object_id.clone(),
                store_dir: e.store_dir.clone(),
                // 关键:保留包内布局(usr/bin/hello 等),世代据此建层级 symlink,
                // 供 bootroot 忠实重建。此前只存 file_name 丢了布局(本轮修复)。
                rel_path: Some(e.rel_path.clone()),
            });
        }
        installed.push(p.name.clone());
    }

    // —— 收口:补运行闭包(libc + loader)入世代,使世代真正自包含可引导 ——
    // 此前 install 只装包文件,bootroot 的 libc 要旁路 export-rootfs 补。现在直接补进世代:
    // 对每个装的包扫 ELF 补闭包(build_with 内部扫全包),把解出的库/loader put 进 store +
    // 加进 refs(rel_path=usr/lib/<soname>、usr/lib64/<loader>),世代即自带运行所需。
    let mut closure_seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for name in &installed {
        // 找包内主二进制(usr/bin 下任一文件);纯库包则跳过补闭包。
        let unpacked = layout.unpacked_dir(name);
        let bin_dir = unpacked.join("usr/bin");
        let main_rel = std::fs::read_dir(&bin_dir).ok().and_then(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .find(|p| p.is_file())
                .and_then(|p| p.strip_prefix(&unpacked).ok().map(|r| r.to_path_buf()))
        });
        let main_rel = match main_rel {
            Some(m) => m,
            None => continue, // 无可执行,纯库/数据包,跳过
        };
        let built = match build_with(layout, name, &main_rel, &[]) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // 解出的库 → store + 世代(rel_path usr/lib/<soname>)
        for (soname, path) in &built.libs {
            if !closure_seen.insert(soname.clone()) {
                continue;
            }
            if let Ok(dir) = put_file(&store, soname, path) {
                refs.push(PackageRef {
                    name: soname.clone(),
                    object_id: obj_id(&dir),
                    store_dir: dir,
                    rel_path: Some(PathBuf::from("usr/lib").join(soname)),
                });
            }
        }
        // loader → store + 世代(rel_path usr/lib64/<name>)
        if let Some(interp) = &built.interpreter {
            let ln = interp.file_name().and_then(|s| s.to_str()).unwrap_or("ld.so").to_string();
            if closure_seen.insert(format!("loader:{ln}")) {
                if let Ok(dir) = put_file(&store, &ln, interp) {
                    refs.push(PackageRef {
                        name: ln.clone(),
                        object_id: obj_id(&dir),
                        store_dir: dir,
                        rel_path: Some(PathBuf::from("usr/lib64").join(&ln)),
                    });
                }
            }
        }
    }

    let gens = open_generations(layout)?;
    gens.make_generation(gen_id, &refs)?;
    // propose 不激活:active 指针不动(世代状态机 propose 语义)。

    Ok(InstallReport {
        installed,
        generation: gen_id,
        store_objects: refs.len(),
    })
}

/// install:propose 造世代 + 立即 set_active(用户首装的便捷路径,不走 verify 门禁)。
/// 行为与历史一致;要带门禁的激活见 `maintain` / `activate_verified`。unix 专有。
#[cfg(unix)]
pub fn install(
    layout: &Layout,
    lock: &aevum_solver::Lock,
    mirror: &str,
    only: &[String],
    gen_id: u64,
) -> Result<InstallReport, Box<dyn std::error::Error>> {
    let report = propose_generation(layout, lock, mirror, only, gen_id)?;
    let gens = open_generations(layout)?;
    gens.set_active(gen_id)?;
    Ok(report)
}

/// **门禁安装**(ADR-0005):propose 造候选世代 → **verify 门禁** → 通过才 set_active。
///
/// 与 [`install`] 的区别:install 是人类显式敲包名的便捷路径,直接激活;
/// 本函数走 verify 闸门,用于 **AI 参与选包**的场景——AI 产出的意图必须由独立的
/// verify machine 复核(完整性/闭合/版本回退),AI 不能自我放行(ADR-0003 边界、ADR-0005)。
///
/// 返回 `(InstallReport, ActivateOutcome)`:候选已造好(report),是否真激活看 outcome。
/// 硬性失败或需确认未放行时 `outcome.activated=false`,候选世代留盘但 active 不动。
/// `confirm=true` 仅放行版本回退类安全判据,永不放行完整性/闭合硬失败。unix 专有。
#[cfg(unix)]
pub fn install_gated(
    layout: &Layout,
    lock: &aevum_solver::Lock,
    lock_name: &str,
    mirror: &str,
    only: &[String],
    gen_id: u64,
    confirm: bool,
) -> Result<(InstallReport, ActivateOutcome), Box<dyn std::error::Error>> {
    // 1. propose:造候选世代,但**不**激活(active 指针不动)。
    let report = propose_generation(layout, lock, mirror, only, gen_id)?;
    // 2. 当前 active 世代的 lock(供门禁做版本回退比较;首装为 None)。
    let active = active_lock_name(layout)?;
    // 3. verify 门禁 → 通过才 set_active(+写 verified 审计标记)。
    let outcome = activate_verified(
        layout,
        lock_name,
        gen_id,
        active.as_deref(),
        None, // foundation manifest:AI 便捷装包场景不强制 foundation 判据
        confirm,
    )?;
    Ok((report, outcome))
}

#[cfg(not(unix))]
pub fn install(
    _layout: &Layout,
    _lock: &aevum_solver::Lock,
    _mirror: &str,
    _only: &[String],
    _gen_id: u64,
) -> Result<InstallReport, Box<dyn std::error::Error>> {
    Err("install 需要 unix(解包/世代 symlink)。请在 Linux/WSL 运行".into())
}

// ───────────────────────── export-rootfs:自包含运行目录 ─────────────────────────

/// 导出 rootfs 的结果:目录 + 在其中运行主程序的命令(loader 注入,PoC-2)。
#[derive(Debug)]
pub struct RootfsExport {
    pub dir: PathBuf,
    /// 容器内运行主程序的 argv(相对 rootfs 根):loader --library-path lib bin/<name>。
    pub run_argv: Vec<String>,
    pub main_name: String,
}

/// 把一个包的运行闭包导出成**自包含 rootfs 目录**(里程碑8:全裸容器验证)。
///
/// 复制闭包的**实体文件**(不是 symlink 回 store,因 Docker COPY 会断链):
/// - `bin/<name>` 主二进制
/// - `lib/<soname>` 各依赖库
/// - `lib/<loader>` 动态链接器
///
/// 产 `run_argv`:`lib/<loader> --library-path lib bin/<name>`——显式 loader + library-path,
/// 不依赖写死的 interp 路径(PoC-2 铁律),故可在无 /lib64、无系统库的 scratch 容器跑。
///
/// unix 专有(权限位)。
#[cfg(unix)]
pub fn export_rootfs(
    built: &BuiltClosure,
    dest: &Path,
) -> Result<RootfsExport, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::remove_dir_all(dest);
    let bin_dir = dest.join("bin");
    let lib_dir = dest.join("lib");
    std::fs::create_dir_all(&bin_dir)?;
    std::fs::create_dir_all(&lib_dir)?;

    // 复制并保留可执行权限的 helper。
    fn copy_exec(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
        // 跟随软链取真实内容(宿主库常是 libfoo.so→实体)。
        let real = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        std::fs::copy(&real, dst)?;
        let mut perms = std::fs::metadata(dst)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dst, perms)?;
        Ok(())
    }

    // 主二进制 → bin/<name>
    let (main_name, main_path) = &built.main;
    copy_exec(main_path, &bin_dir.join(main_name))?;

    // 各库 → lib/<soname>(ld 按 soname 找,文件名须等于 soname)
    for (soname, path) in &built.libs {
        copy_exec(path, &lib_dir.join(soname))?;
    }

    // loader → lib/<basename>
    let loader_name = built
        .interpreter
        .as_ref()
        .ok_or("无 loader(interp),无法在全裸环境运行")?
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("loader 名解析失败")?
        .to_string();
    copy_exec(built.interpreter.as_ref().unwrap(), &lib_dir.join(&loader_name))?;

    // 运行命令(相对 rootfs 根):loader --library-path lib bin/<name>
    let run_argv = vec![
        format!("lib/{loader_name}"),
        "--library-path".to_string(),
        "lib".to_string(),
        format!("bin/{main_name}"),
    ];

    Ok(RootfsExport {
        dir: dest.to_path_buf(),
        run_argv,
        main_name: main_name.clone(),
    })
}

#[cfg(not(unix))]
pub fn export_rootfs(
    _built: &BuiltClosure,
    _dest: &Path,
) -> Result<RootfsExport, Box<dyn std::error::Error>> {
    Err("export_rootfs 需要 unix(权限位/loader)。请在 Linux/WSL 运行".into())
}

// ───────────────────────── compose-generation + export-bootroot ─────────────────────────

/// 把若干已存世代引用的对象合并组成一个**新世代**(引擎驱动,非脚本拼)。
///
/// 用途:hello 装在 gen-A、busybox 装在 gen-B,合并成一个含两者的可引导世代。
/// 读各源世代的 `generation_refs`(rel_path→store 对象),按 **rel_path 布局路径**去重
/// (同路径取后者),保留布局后 make_generation。
#[cfg(unix)]
pub fn compose_generation(
    layout: &Layout,
    src_gens: &[u64],
    new_id: u64,
) -> Result<usize, Box<dyn std::error::Error>> {
    let gens = open_generations(layout)?;
    // 按 rel_path(布局路径)去重,保留布局——不再按裸 name(会丢布局且撞名)。
    let mut by_path: std::collections::BTreeMap<PathBuf, PackageRef> =
        std::collections::BTreeMap::new();
    for &gid in src_gens {
        for (rel, store_dir) in gens.generation_refs(gid)? {
            let object_id = store_dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = rel
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| object_id.clone());
            by_path.insert(
                rel.clone(),
                PackageRef { name, object_id, store_dir, rel_path: Some(rel) },
            );
        }
    }
    let refs: Vec<PackageRef> = by_path.into_values().collect();
    gens.make_generation(new_id, &refs)?;
    Ok(refs.len())
}

/// 从一个**真实世代**导出可作系统根的 bootroot(ADR-0006 阶段2,引擎驱动)。
///
/// 内容与布局**全部来自世代**(`generation_refs` 的 rel_path + store 对象),Aevum 引擎产出,
/// 非脚本手拼、非绕 unpacked 猜包名:
/// - 每个 store 对象(单文件)按其 **rel_path** 放进 bootroot 对应位置(忠实保留布局)
/// - `/lib64/ld-linux-x86-64.so.2` 软链(让写死 interp 的二进制直接跑)
/// - `/AEVUM_GENERATION_ROOT`(根标志,含 gen_id 可审计)
///
/// rel_path 在 install 建世代时已保留(本轮修复:此前只存 file_name 丢了布局)。
#[cfg(unix)]
pub fn export_bootroot(
    layout: &Layout,
    gen_id: u64,
    dest: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;
    let gens = open_generations(layout)?;
    let refs = gens.generation_refs(gen_id)?;

    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest)?;
    for d in ["proc", "sys", "dev", "tmp", "lib64"] {
        std::fs::create_dir_all(dest.join(d))?;
    }

    let mut copied = 0usize;
    let mut loader_name: Option<String> = None;
    for (rel, store_dir) in &refs {
        // store 对象是单文件:<hash>-<name>/<name>。取其中的真实条目(可能是软链)。
        let fname = match rel.file_name() {
            Some(f) => f,
            None => continue,
        };
        let src = store_dir.join(fname);
        // ⚠ PoC-5 铁律:符号链接保留不解引用。库的 soname 链(libX.so.A → libX.so.A.B.C)
        // 是 loader 按 NEEDED 命中的关键;此前这里用 is_file()+canonicalize 把软链跳过/解引用,
        // 导致 multiarch 目录丢 soname 链、s6 等多库依赖包引导时 loader 找不到库(第27轮 4a 暴露)。
        let src_meta = match std::fs::symlink_metadata(&src) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let dst = dest.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if src_meta.file_type().is_symlink() {
            // 软链对象:在 bootroot 重建同样的软链(保留 soname→实体 关系,不解引用)。
            let target = std::fs::read_link(&src)?;
            let _ = std::fs::remove_file(&dst);
            std::os::unix::fs::symlink(&target, &dst)?;
            copied += 1;
            continue;
        }
        if !src_meta.is_file() {
            continue;
        }
        // 实体文件:复制内容 + 恢复权限位(PoC-6:权限是语义)。
        std::fs::copy(&src, &dst)?;
        let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755));
        copied += 1;
        // 记下 loader 的实际位置(供建 /lib64 软链)。
        let n = fname.to_string_lossy();
        if n.starts_with("ld-linux") {
            loader_name = Some(rel.to_string_lossy().into_owned());
        }
    }

    // /lib64/ld-linux 软链 → 世代里的 loader(让写死 interp 的二进制直接跑)。
    if let Some(l) = loader_name {
        let link = dest.join("lib64/ld-linux-x86-64.so.2");
        let _ = std::fs::remove_file(&link);
        // 从 /lib64 指向世代内 loader 的相对路径。
        std::os::unix::fs::symlink(format!("/{l}"), &link)?;
    }

    // 根标志(含 gen_id,可审计:证明此根来自哪个 Aevum 世代)。
    std::fs::write(
        dest.join("AEVUM_GENERATION_ROOT"),
        format!(
            "Aevum generation root: gen-{gen_id}\nobjects: {}\n",
            refs.len()
        ),
    )?;

    Ok(copied)
}

/// 导出可运行系统 rootfs:在 [`export_bootroot`] 基础上补齐 nspawn/chroot 所需的最小骨架。
///
/// 补齐项(bootroot 不含,但 systemd-nspawn / chroot 要求):
/// - `/etc/passwd`(root 用户)、`/etc/group`(root 组)、`/etc/hostname`
/// - `/bin/sh` 软链(指向世代内的 busybox/busybox-static;无则尝试 bash)
/// - `/root` home 目录
///
/// 这是路线3"Aevum 管理的用户态真能运行"的交付物:
/// `systemd-nspawn -D <dest>` 或 `chroot <dest> /bin/sh` 即可进入 Aevum 世代。
#[cfg(unix)]
pub fn export_system(
    layout: &Layout,
    gen_id: u64,
    dest: &Path,
) -> Result<ExportSystemReport, Box<dyn std::error::Error>> {
    // 1. 调 export_bootroot 铺世代文件树 + 基础目录。
    let file_count = export_bootroot(layout, gen_id, dest)?;

    // 2. 补 /etc 骨架。
    let etc = dest.join("etc");
    std::fs::create_dir_all(&etc)?;
    std::fs::write(etc.join("passwd"), "root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n")?;
    std::fs::write(etc.join("group"), "root:x:0:\nnogroup:x:65534:\n")?;
    std::fs::write(etc.join("hostname"), "aevum\n")?;
    std::fs::write(etc.join("resolv.conf"), "nameserver 8.8.8.8\n")?;

    // 3. /root home 目录。
    std::fs::create_dir_all(dest.join("root"))?;

    // 4. /bin/sh:找世代内的 busybox-static / busybox / bash,建 /bin/sh 软链。
    let bin_dir = dest.join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let shell_found = find_and_link_shell(dest, &bin_dir);

    Ok(ExportSystemReport { dest: dest.to_path_buf(), file_count, shell_found })
}

/// 导出报告。
#[derive(Debug)]
pub struct ExportSystemReport {
    pub dest: PathBuf,
    pub file_count: usize,
    pub shell_found: bool,
}

/// 在 rootfs 内找 busybox-static / busybox / bash,建 /bin/sh 软链;成功返回 true。
#[cfg(unix)]
fn find_and_link_shell(rootfs: &Path, bin_dir: &Path) -> bool {
    // 搜索候选:按优先级(静态 busybox 最佳,无外部依赖)。
    // 包含根目录候选(部分世代 rel_path 为扁平文件名,铺在根)。
    let candidates = [
        "usr/bin/busybox-static",
        "usr/bin/busybox",
        "bin/busybox-static",
        "bin/busybox",
        "busybox-static",
        "busybox",
        "usr/bin/bash",
        "bin/bash",
        "usr/bin/sh",
        "bin/sh",
    ];
    for rel in candidates {
        let full = rootfs.join(rel);
        if full.exists() {
            let sh = bin_dir.join("sh");
            let _ = std::fs::remove_file(&sh);
            // 用绝对路径(rootfs 内):从 /bin/sh → /<rel>
            let target = format!("/{rel}");
            let _ = std::os::unix::fs::symlink(&target, &sh);
            return true;
        }
    }
    false
}

#[cfg(not(unix))]
pub fn export_system(
    _layout: &Layout,
    _gen_id: u64,
    _dest: &Path,
) -> Result<ExportSystemReport, Box<dyn std::error::Error>> {
    Err("export_system 需要 unix。请在 Linux/WSL 运行".into())
}

/// 刷新 `$AEVUM_ROOT/profile/bin/`:扫描 active 世代的可执行文件,建 symlink 指向 store 内实体。
///
/// 路线1 核心:用户把 `$AEVUM_ROOT/profile/bin` 加进 PATH(一次性),之后每次 switch/activate
/// 都调本函数刷新 → shell 里立即能跑新世代的程序。跟 Nix 的 `~/.nix-profile/bin` 同一个模式:
/// 稳定路径 + 内容随世代切换而变。
///
/// 实现:遍历 active 世代的 `generation_refs`(rel_path + store_dir),对 rel_path 含
/// `usr/bin/` 或 `bin/` 的条目,在 profile/bin/ 建 `<name>` → `<store_dir>/<name>` 的 symlink。
/// 每次刷新前清空 profile/bin/(原子更新:先建新目录再 rename)。
#[cfg(unix)]
pub fn refresh_profile(layout: &Layout) -> Result<usize, Box<dyn std::error::Error>> {
    let gens = open_generations(layout)?;
    // 找 active 世代 id:read_link 取 symlink 目标,从中提取 gen-NNN。
    // 注意:set_active 写的可能是相对路径(相对 cwd,如 ./.aevum/generations/gen-100),
    // 或绝对路径;我们只需从路径的最后一段提取 gen id。
    let active_link = layout.generations_dir().join("active");
    if !active_link.symlink_metadata().is_ok() {
        return Err("无 active 世代(先 aevum switch/activate)".into());
    }
    let target = std::fs::read_link(&active_link)?;
    // 从路径中找 "gen-NNN" 段(可能在末尾,也可能路径里有)。
    let gen_id: u64 = target
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter_map(|s| s.strip_prefix("gen-"))
        .filter_map(|s| s.parse::<u64>().ok())
        .next()
        .ok_or_else(|| format!("active 指向非世代目录: {}", target.display()))?;

    let refs = gens.generation_refs(gen_id)?;

    // 建新的 profile/bin(先用临时目录,再 rename 实现原子更新)。
    let profile_bin = layout.profile_bin_dir();
    let tmp_bin = layout.root.join("profile").join(".bin.new");
    let _ = std::fs::remove_dir_all(&tmp_bin);
    std::fs::create_dir_all(&tmp_bin)?;

    let mut count = 0usize;
    for (rel, store_dir) in &refs {
        // 只处理 bin 路径下的条目(usr/bin/<name> 或 bin/<name>)。
        let rel_str = rel.to_string_lossy();
        let is_bin = rel_str.starts_with("usr/bin/")
            || rel_str.starts_with("bin/")
            || rel_str.starts_with("usr/sbin/")
            || rel_str.starts_with("sbin/");
        if !is_bin {
            continue;
        }
        let Some(name) = rel.file_name().and_then(|n| n.to_str()) else { continue };
        // store 对象是目录 `<hash>-<name>/<name>`:symlink 指向里面的同名文件。
        // 必须用绝对路径(store_dir 可能是相对路径,从 profile/bin/ 解析不到)。
        let actual_file = store_dir.join(name);
        let actual_file = match std::fs::canonicalize(&actual_file) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let link = tmp_bin.join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&actual_file, &link)?;
        count += 1;
    }

    // 原子替换:删旧 → rename 新。(非真原子,但世代切换期间够用。)
    let _ = std::fs::remove_dir_all(&profile_bin);
    std::fs::rename(&tmp_bin, &profile_bin)?;

    // 同时建 profile/lib/:所有 .so 文件的 symlink farm(让动态链接程序通过 LD_LIBRARY_PATH 直接跑)。
    let profile_lib = layout.root.join("profile").join("lib");
    let tmp_lib = layout.root.join("profile").join(".lib.new");
    let _ = std::fs::remove_dir_all(&tmp_lib);
    std::fs::create_dir_all(&tmp_lib)?;

    let mut lib_count = 0usize;
    for (rel, store_dir) in &refs {
        let rel_str = rel.to_string_lossy();
        // 只处理 lib 路径下的 .so 文件
        let is_lib = rel_str.contains("/lib/") || rel_str.starts_with("lib/") || rel_str.starts_with("usr/lib/");
        if !is_lib {
            continue;
        }
        let Some(name) = rel.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.contains(".so") {
            continue;
        }
        let actual_file = store_dir.join(name);
        let actual_file = match std::fs::canonicalize(&actual_file) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let link = tmp_lib.join(name);
        if !link.exists() {
            // 不覆盖已有(先到的优先,避免版本冲突)
            let _ = std::os::unix::fs::symlink(&actual_file, &link);
            lib_count += 1;
            // 建 soname symlink:libfoo.so.X.Y.Z → 也建 libfoo.so.X 和 libfoo.so
            // 让动态链接器能通过 soname 找到库。
            create_soname_links(&tmp_lib, name);
        }
    }

    let _ = std::fs::remove_dir_all(&profile_lib);
    if lib_count > 0 {
        std::fs::rename(&tmp_lib, &profile_lib)?;
    } else {
        let _ = std::fs::remove_dir_all(&tmp_lib);
    }

    // 写 profile/env.sh:source 后设 PATH + LD_LIBRARY_PATH
    let env_sh = layout.root.join("profile").join("env.sh");
    let profile_bin_abs = std::fs::canonicalize(&layout.profile_bin_dir()).unwrap_or(layout.profile_bin_dir());
    let profile_lib_abs = layout.root.join("profile").join("lib");
    std::fs::write(&env_sh, format!(
        "# Aevum profile 环境(source 此文件)\nexport PATH=\"{}:$PATH\"\nexport LD_LIBRARY_PATH=\"{}:${{LD_LIBRARY_PATH:-}}\"\n",
        profile_bin_abs.display(),
        profile_lib_abs.display(),
    ))?;

    Ok(count)
}

#[cfg(not(unix))]
pub fn refresh_profile(_layout: &Layout) -> Result<usize, Box<dyn std::error::Error>> {
    Err("refresh_profile 需要 unix。请在 Linux/WSL 运行".into())
}

/// 进程级排他锁(P1-5):防两个 aevum 变更命令并发改同一 root。
///
/// 全仓此前无任何 advisory lock。两个并发 install:`next_generation_id` 都取 max+1
/// 算出**同一** id,向同一 `gen-NNN/packages` 交错写 symlink;还共享 per-package
/// unpacked 目录互相 `remove_dir_all`。set_active 的 rename 原子,但其前的一切都不是。
///
/// 实现:对 `$AEVUM_ROOT/.lock` 文件 `flock(LOCK_EX)`。RAII:guard 在 drop 时关闭 fd,
/// 内核自动释放锁(进程崩溃/被 kill 也释放,不会留死锁)。变更命令开头取锁、持有到结束。
/// unix 专有;非 unix 为 no-op(变更命令本就只在 Linux/WSL 真跑)。
#[cfg(unix)]
pub struct FsLock {
    _file: std::fs::File,
}

#[cfg(unix)]
impl FsLock {
    /// 取 `$AEVUM_ROOT/.lock` 的排他锁(阻塞直到拿到)。root 不存在则先建。
    pub fn acquire(layout: &Layout) -> Result<Self, Box<dyn std::error::Error>> {
        use std::os::unix::io::AsRawFd;
        std::fs::create_dir_all(&layout.root)?;
        let path = layout.root.join(".lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        // flock LOCK_EX:阻塞获取排他锁。EINTR 时重试。
        loop {
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc == 0 {
                break;
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("获取 {} 排他锁失败: {err}", path.display()).into());
        }
        Ok(FsLock { _file: file })
    }
}
// drop 时 File 关闭 → 内核释放 flock(无需显式 unlock)。

#[cfg(not(unix))]
pub struct FsLock;

#[cfg(not(unix))]
impl FsLock {
    pub fn acquire(_layout: &Layout) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(FsLock) // 非 unix:no-op(变更命令本就需 unix)
    }
}

/// 初始化一个 Aevum root:建好目录骨架 + 写**引导版** `profile/env.sh`(P1-4)。
///
/// 解决"开箱即崩":`env.sh` 原本只在 refresh_profile(首次 install/switch 后)才写,
/// 但 README quick-start 让用户 `aevum update` 后立刻 `source env.sh` —— 此时文件不存在。
/// `aevum init` 先把 `profile/{bin,lib}` 和一个空的 env.sh 建出来,使首装前 source 就不报错
/// (PATH 先指向空 bin,装完包 refresh_profile 会重写同一文件填入真实路径)。
///
/// 幂等:已存在的目录/文件不破坏(env.sh 总是重写为引导版,内容稳定)。
/// 跨平台:纯建目录 + 写文本(env.sh 是 sh 脚本,Linux/WSL 用;非 unix 也无害地写出)。
pub fn init_layout(layout: &Layout) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&layout.root)?;
    let profile = layout.root.join("profile");
    let profile_bin = profile.join("bin");
    let profile_lib = profile.join("lib");
    std::fs::create_dir_all(&profile_bin)?;
    std::fs::create_dir_all(&profile_lib)?;
    std::fs::create_dir_all(layout.locks_dir())?;
    std::fs::create_dir_all(layout.generations_dir())?;

    // 引导版 env.sh:用绝对路径(canonicalize 失败则退回原路径,目录此刻已存在)。
    let env_sh = profile.join("env.sh");
    let bin_abs = std::fs::canonicalize(&profile_bin).unwrap_or(profile_bin);
    let lib_abs = std::fs::canonicalize(&profile_lib).unwrap_or(profile_lib);
    std::fs::write(
        &env_sh,
        format!(
            "# Aevum profile 环境(source 此文件)。由 `aevum init` 建,装包后 refresh_profile 重写。\n\
             export PATH=\"{}:$PATH\"\n\
             export LD_LIBRARY_PATH=\"{}:${{LD_LIBRARY_PATH:-}}\"\n",
            bin_abs.display(),
            lib_abs.display(),
        ),
    )?;
    Ok(())
}

/// 为 `libfoo.so.X.Y.Z` 建 soname symlink:`libfoo.so.X` → `libfoo.so.X.Y.Z`(在同目录)。
/// 动态链接器按 soname 查找库,但 store 里只有完整版本名的文件。
#[cfg(unix)]
fn create_soname_links(dir: &Path, full_name: &str) {
    // 模式:libfoo.so.X.Y.Z → 建 libfoo.so.X 和 libfoo.so
    // 找 ".so." 的位置
    let Some(so_pos) = full_name.find(".so.") else { return };
    let base = &full_name[..so_pos + 3]; // "libfoo.so"
    let after_so = &full_name[so_pos + 4..]; // "X.Y.Z" 或 "X"

    // libfoo.so.X(soname,最重要)
    if let Some(dot_pos) = after_so.find('.') {
        let soname = format!("{}.{}", base, &after_so[..dot_pos]);
        let link = dir.join(&soname);
        if !link.exists() {
            let _ = std::os::unix::fs::symlink(full_name, &link);
        }
    }

    // libfoo.so(linker name,可选)
    let linker_link = dir.join(base);
    if !linker_link.exists() {
        let _ = std::os::unix::fs::symlink(full_name, &linker_link);
    }
}


#[cfg(not(unix))]
pub fn export_bootroot(_l: &Layout, _g: u64, _d: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    Err("export_bootroot 需要 unix。请在 Linux/WSL 运行".into())
}

#[cfg(not(unix))]
pub fn compose_generation(_l: &Layout, _s: &[u64], _n: u64) -> Result<usize, Box<dyn std::error::Error>> {
    Err("compose_generation 需要 unix。请在 Linux/WSL 运行".into())
}

// ───────────────────────── verify:AI maintainer 安全闸门(C 主线)─────────────────────────

/// 把 [`resolve_constraints`] 写出的文本 lock 读回 [`aevum_solver::Lock`]。
///
/// 格式(见 `resolve_constraints`):头部 `key: value` 行 → `---` → 每行 `name@version#fingerprint\tfilename`。
/// 头部的 `closure_id` 被保留;`package_count` 按实际包数重算(不信任文件头,防手改不一致)。
/// 跨平台(纯文本)。
pub fn parse_lock_file(path: &Path) -> Result<aevum_solver::Lock, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读 lock 失败 {}: {e}", path.display()))?;
    let mut closure_id = String::new();
    let mut in_body = false;
    let mut locked = Vec::new();
    for line in text.lines() {
        if !in_body {
            if line.trim() == "---" {
                in_body = true;
            } else if let Some(v) = line.strip_prefix("closure_id:") {
                closure_id = v.trim().to_string();
            }
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // name@version#fingerprint\tfilename
        let (pkg, filename) = match line.split_once('\t') {
            Some((p, f)) => (p, f.to_string()),
            None => (line, String::new()),
        };
        // 从右拆 #fingerprint,再从中拆 @version(版本号本身不含 @,name 不含 @)。
        let (name_ver, fingerprint) = pkg
            .split_once('#')
            .map(|(nv, fp)| (nv, fp.to_string()))
            .unwrap_or((pkg, String::new()));
        let (name, version) = name_ver
            .split_once('@')
            .ok_or_else(|| format!("lock 行格式非法(缺 @version): {line}"))?;
        locked.push(aevum_solver::LockedPackage {
            name: name.to_string(),
            version: version.to_string(),
            fingerprint,
            filename,
        });
    }
    Ok(aevum_solver::Lock {
        closure_id,
        package_count: locked.len(),
        locked,
        diagnostics: aevum_solver::Diagnostics::default(),
    })
}

/// 读回 lock 头部的 `templates:` 行 → `(name, version)` 列表(`none` 或缺失 → 空)。
///
/// 供 maintain 接入时把"这个 lock 用了哪些模板"写进世代 templates.txt(验收7控制面)。
/// 头部行格式:`templates: name@ver, name2@ver2`(resolve_constraints_opt 写)。
pub fn read_lock_templates(path: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    for line in text.lines() {
        if line.trim() == "---" {
            break; // 只扫头部
        }
        if let Some(v) = line.strip_prefix("templates:") {
            let v = v.trim();
            if v.is_empty() || v == "none" {
                return Vec::new();
            }
            return v
                .split(',')
                .map(|e| e.trim())
                .filter(|e| !e.is_empty())
                .filter_map(|e| e.split_once('@').map(|(n, ver)| (n.to_string(), ver.to_string())))
                .collect();
        }
    }
    Vec::new()
}

/// 读回 lock 头部的 `ts_inputs:` 行(`none`/缺失 → None)。供 audit 漂移检测用记录的输入重放。
pub fn read_lock_ts_inputs(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if line.trim() == "---" {
            break; // 只扫头部
        }
        if let Some(v) = line.strip_prefix("ts_inputs:") {
            let v = v.trim();
            if v.is_empty() || v == "none" {
                return None;
            }
            return Some(v.to_string());
        }
    }
    None
}

/// TS 配置 → 约束集 + 模板记录(供 `resolve --config` 与 `audit-config` 共用,避免重复)。
///
/// 链路:沙箱求值 `eval_to_outcome` → 模板展开(override/exclude 一并作用)→ 收集模板版本
/// → 模板约束与直接 use/override 约束合并(直接 use 按名覆盖模板)。**不写 lock**——
/// 返回约束集与模板记录串,由调用方决定求解/比对。
///
/// 返回 `(constraints, templates_record)`:templates_record 为 `name@ver, ...`(无模板则 None)。
pub fn ts_config_to_constraints(
    layout: &Layout,
    ts_source: &str,
    inputs: Option<&str>,
) -> Result<(Vec<aevum_solver::Constraint>, Option<String>), Box<dyn std::error::Error>> {
    let outcome = aevum_config_ts::eval_to_outcome(ts_source, inputs)
        .map_err(|e| format!("TS 配置求值失败: {e}"))?;

    // 模板展开:override/exclude 作用于模板能力。
    let mut opts = aevum_template::ExpandOptions::default();
    for (n, v) in &outcome.overrides {
        opts.overrides.insert(n.clone(), format!("={v}"));
    }
    for e in &outcome.excludes {
        opts.excludes.insert(e.clone());
    }
    let tmpl_constraints = if outcome.templates.is_empty() {
        Vec::new()
    } else {
        aevum_template::expand(&layout.templates_dir(), &outcome.templates, &opts)
            .map_err(|e| format!("模板展开失败: {e}"))?
    };
    // 收集展开涉及的模板及版本(含 extends 链)。
    let templates_record = if outcome.templates.is_empty() {
        None
    } else {
        let pairs = aevum_template::collect_templates(&layout.templates_dir(), &outcome.templates)
            .map_err(|e| format!("收集模板版本失败: {e}"))?;
        Some(pairs.iter().map(|(n, v)| format!("{n}@{v}")).collect::<Vec<_>>().join(", "))
    };

    // 合并:模板约束 + 直接 use/override(后者按名覆盖模板)。
    let direct = outcome.into_constraints();
    let mut by_name: std::collections::BTreeMap<String, aevum_solver::Constraint> =
        std::collections::BTreeMap::new();
    for c in tmpl_constraints {
        by_name.insert(c.name.clone(), c);
    }
    for c in direct {
        by_name.insert(c.name.clone(), c);
    }
    Ok((by_name.into_values().collect(), templates_record))
}

/// 配置漂移检测报告:重跑源 TS 配置后,与历史 lock 比对 closure_id 的结果。
#[derive(Debug, Clone)]
pub struct AuditReport {
    /// 是否漂移(closure_id 不一致)。
    pub drifted: bool,
    /// lock 记录的 closure_id(期望)。
    pub expected_closure_id: String,
    /// 重跑求解得到的 closure_id(实际)。
    pub actual_closure_id: String,
    /// lock 的包数。
    pub expected_pkg_count: usize,
    /// 重跑的包数。
    pub actual_pkg_count: usize,
    /// 本次重放所用的 inputs(来自 lock 记录或 --inputs 覆盖)。
    pub used_inputs: Option<String>,
}

impl std::fmt::Display for AuditReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.drifted {
            writeln!(f, "⚠ 配置已漂移:重跑源配置产出的 closure_id 与 lock 不一致")?;
            writeln!(f, "  lock 期望: {} ({} 包)", self.expected_closure_id, self.expected_pkg_count)?;
            writeln!(f, "  重跑实际: {} ({} 包)", self.actual_closure_id, self.actual_pkg_count)?;
            write!(f, "  (漂移可能源于:源 .ts/模板被改、或包索引快照变化)")
        } else {
            write!(f, "✓ 未漂移:源配置重跑产出相同 closure_id={} ({} 包)", self.actual_closure_id, self.actual_pkg_count)
        }
    }
}

/// 配置漂移检测:用 lock 记录的 inputs 重跑源 TS 配置 → 重新求解 → 比对 closure_id(D1~D5)。
///
/// 可复现只来自 lock;本函数是**旁路审计**——验证某历史 lock 是否仍能由其源 TS 配置 + 记录的
/// inputs 重新产出。一致=源未漂移;不一致=源 .ts/模板被改或索引变化。
///
/// `inputs_override`:`Some` 时覆盖 lock 记录的 ts_inputs(用于"换输入会怎样"对比);
/// `None` 时用 lock 记录值(审计语义:用当时的输入重放)。重跑写临时 lock `audit-<against>`。
pub fn audit_config(
    layout: &Layout,
    ts_source: &str,
    against_lock_name: &str,
    inputs_override: Option<&str>,
) -> Result<AuditReport, Box<dyn std::error::Error>> {
    let lock_path = layout.locks_dir().join(format!("{against_lock_name}.lock"));
    if !lock_path.exists() {
        return Err(format!("lock 不存在: {}", lock_path.display()).into());
    }
    let expected = parse_lock_file(&lock_path)?;
    let recorded_inputs = read_lock_ts_inputs(&lock_path);

    // inputs 优先用 override,否则用 lock 记录值。
    let inputs = inputs_override.map(|s| s.to_string()).or(recorded_inputs);

    // 重跑求解(写临时 lock 名,不覆盖被审计的 lock)。
    let (constraints, _templates_record) =
        ts_config_to_constraints(layout, ts_source, inputs.as_deref())?;
    let audit_name = format!("audit-{against_lock_name}");
    let actual = resolve_constraints_opt(layout, &constraints, &audit_name, None, false, inputs.as_deref(), None)?;

    Ok(AuditReport {
        drifted: actual.closure_id != expected.closure_id,
        expected_closure_id: expected.closure_id,
        actual_closure_id: actual.closure_id,
        expected_pkg_count: expected.package_count,
        actual_pkg_count: actual.package_count,
        used_inputs: inputs,
    })
}

/// 编排一次 verify:读 candidate lock + 世代 object_ids + index(+可选 active lock / foundation manifest)→
/// 调 [`aevum_maintainer::verify`] 产报告。这是 C 主线 propose→**verify**→activate 的中段闸门。
///
/// - `candidate_lock_name`:`locks/<name>.lock`,提供版本语义(判据2/4)。
/// - `candidate_gen`:已构造的候选世代 id,其 `lock.txt` 提供 store object_ids(判据1 完整性)。
/// - `active_lock_name`:当前 active 的 lock 名,用于版本回退比较;`None` 跳过(首装)。
/// - `foundation_manifest_path`:foundation manifest TOML 路径;`Some` 时启用判据3(required 在场+版本精确)
///   并把 manifest 包名并入 foundation 提供集(消除闭合性误报);`None` 时跳过判据3(行为同旧版)。
///
/// 跨平台(verify 本身纯数据;store 完整性重算在 unix 才实际比对哈希,见 store::get)。
pub fn verify_generation(
    layout: &Layout,
    candidate_lock_name: &str,
    candidate_gen: u64,
    active_lock_name: Option<&str>,
    foundation_manifest_path: Option<&Path>,
) -> Result<aevum_maintainer::VerifyReport, Box<dyn std::error::Error>> {
    // index(查 Depends 做闭合性)。
    let index_path = layout.index_file();
    let index_text = std::fs::read_to_string(&index_path).map_err(|e| {
        format!("读索引失败 {}: {e}(先 `bash scripts/prep-index.sh`)", index_path.display())
    })?;
    let index = aevum_solver::Index::from_packages_str(&index_text);

    // candidate lock(版本语义)。
    let cand_path = layout.locks_dir().join(format!("{candidate_lock_name}.lock"));
    let candidate_lock = parse_lock_file(&cand_path)?;

    // active lock(可选,版本回退比较)。
    let active_lock = match active_lock_name {
        Some(name) => Some(parse_lock_file(&layout.locks_dir().join(format!("{name}.lock")))?),
        None => None,
    };

    // foundation manifest(可选,判据3)。
    let foundation = match foundation_manifest_path {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("读 foundation manifest 失败 {}: {e}", path.display()))?;
            Some(aevum_maintainer::FoundationManifest::parse(&text)?)
        }
        None => None,
    };

    // 候选世代 store 对象(完整性)。
    let gens = open_generations(layout)?;
    let object_ids = gens.generation_object_ids(candidate_gen)?;

    let store = open_store(layout)?;

    Ok(aevum_maintainer::verify(
        &candidate_lock,
        active_lock.as_ref(),
        &index,
        &store,
        &object_ids,
        &[], // foundation_provided:显式集留空,manifest 在场时由 verify 自动并入其包名
        foundation.as_ref(),
    ))
}

/// 一次门禁激活的结果(供 CLI 据此打印与定退出码)。
pub struct ActivateOutcome {
    /// verify 报告(无论是否激活都附,供打印分判据)。
    pub report: aevum_maintainer::VerifyReport,
    /// 是否真的切了 active 指针。
    pub activated: bool,
    /// 未激活时的原因(硬性失败 / 需确认未给 --confirm);激活时为 None。
    pub blocked_reason: Option<ActivateBlocked>,
}

/// 门禁拒绝激活的原因。
#[derive(Debug, PartialEq, Eq)]
pub enum ActivateBlocked {
    /// 硬性校验未通过(完整性/闭合/层)。
    HardFail,
    /// 校验通过但触发安全判据(版本回退),且未给 `--confirm`。
    NeedsConfirm,
}

/// **verify 门禁激活**:C 主线把 verify 作为 activate 的前置门禁,闭合 ADR-0003 安全模型。
///
/// 流程:`verify_generation` → 按报告分流 →
/// - 硬性失败(`!passed`):拒绝,`active` 不动([`ActivateBlocked::HardFail`])。
/// - 需人工确认(`needs_user_confirm`)且未 `confirm`:拒绝([`ActivateBlocked::NeedsConfirm`])。
/// - 通过(或已 `confirm` 覆盖安全判据):`set_active` 原子切换 + 写 `verified` 审计标记。
///
/// 这是与裸 `set_active`(install/switch/rollback 用,机械切换)并列的**安全激活路径**:
/// 它**永不**绕过 verify。rollback 不走这里——回滚目标本就是历史 verified 世代,且须满足"秒回"红线。
///
/// `confirm=true` 仅能放行**版本回退**类安全判据(人类已知情拍板,ADR-0003 边界3);
/// **永远不能**放行硬性失败(完整性/闭合)——损坏/不闭合的世代无论如何不可激活。
///
/// unix 专有(`set_active` 是 symlink+rename)。
#[cfg(unix)]
pub fn activate_verified(
    layout: &Layout,
    candidate_lock_name: &str,
    candidate_gen: u64,
    active_lock_name: Option<&str>,
    foundation_manifest_path: Option<&Path>,
    confirm: bool,
) -> Result<ActivateOutcome, Box<dyn std::error::Error>> {
    let report = verify_generation(
        layout,
        candidate_lock_name,
        candidate_gen,
        active_lock_name,
        foundation_manifest_path,
    )?;

    // 硬性失败:无论 confirm 与否都拒绝(损坏/不闭合的世代不可激活)。
    if !report.passed {
        return Ok(ActivateOutcome { report, activated: false, blocked_reason: Some(ActivateBlocked::HardFail) });
    }
    // 需人工确认且未给 --confirm:拒绝(防 AI 自我放行,人类未拍板)。
    if report.needs_user_confirm && !confirm {
        return Ok(ActivateOutcome { report, activated: false, blocked_reason: Some(ActivateBlocked::NeedsConfirm) });
    }

    // 通过门禁:原子切换 + 写 verified 审计标记。
    let gens = open_generations(layout)?;
    gens.set_active(candidate_gen)?;
    write_verified_marker(layout, candidate_gen, &report, confirm)?;

    Ok(ActivateOutcome { report, activated: true, blocked_reason: None })
}

/// 写世代的 `verified` 审计标记(证明此激活经过 verify 门禁,可审计)。
/// 放在世代目录下,随世代走;内容记录判据结果与是否经人工确认。
#[cfg(unix)]
fn write_verified_marker(
    layout: &Layout,
    gen_id: u64,
    report: &aevum_maintainer::VerifyReport,
    confirmed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let marker = layout.generations_dir().join(format!("gen-{gen_id:03}")).join("verified");
    let body = format!(
        "verified by aevum verify gate\n\
         passed={}\n\
         needs_user_confirm={}\n\
         user_confirmed={}\n\
         integrity_failures={}\n\
         unclosed_deps={}\n\
         version_rollbacks={}\n",
        report.passed,
        report.needs_user_confirm,
        confirmed,
        report.integrity_failures.len(),
        report.unclosed_deps.len(),
        report.version_rollbacks.len(),
    );
    std::fs::write(&marker, body)?;
    Ok(())
}

// ───────────────────────── maintain:端到端主循环(C 主线总成)─────────────────────────

/// maintain 主循环的阶段性结果(供 CLI 打印各步)。
#[cfg(unix)]
pub struct MaintainOutcome {
    /// 求解得到的 closure 包数。
    pub resolved_packages: usize,
    /// propose 造出的候选世代 id。
    pub candidate_gen: u64,
    /// propose 入世代的 store 对象数。
    pub store_objects: usize,
    /// 门禁激活结果(含 verify 报告与是否激活)。
    pub activation: ActivateOutcome,
}

/// **maintain**:显式包名走完整主循环 —— 求解 → propose 候选世代 → verify 门禁 → 激活。
///
/// 对应 `docs/ai/01-maintainer-loop.md` 的主循环图(意图解析→求解闭包→propose→verify→激活)。
/// 这是 C 主线的端到端总成。意图(自然语言)入口见 CLI `aevum maintain --intent`,
/// 它在翻译+确认后写 lock,再调 [`maintain_from_lock`] 复用本函数的后半段。
///
/// 流程:
/// 1. `resolve` 显式包名 → 确定性闭包 → 写 `locks/<lock_name>.lock`(ADR-0003:AI 不选 hash)。
/// 2. `maintain_from_lock`:propose 候选世代(不激活)→ verify 门禁 → 激活。
///
/// `active_lock_name`/`foundation_manifest_path`/`confirm` 透传给门禁(见 [`activate_verified`])。
/// 门禁拒绝时返回的 `activation.activated=false` + `blocked_reason`,**不**视为 Err(让 CLI 据此定退出码)。
/// unix 专有。
#[cfg(unix)]
pub fn maintain(
    layout: &Layout,
    packages: &[String],
    mirror: &str,
    lock_name: &str,
    candidate_gen: u64,
    active_lock_name: Option<&str>,
    foundation_manifest_path: Option<&Path>,
    repair: bool,
    confirm: bool,
) -> Result<MaintainOutcome, Box<dyn std::error::Error>> {
    // 1. 求解闭包 → 写 lock(确定性,无 AI 选 hash)。repair=true 时自动应用方案A 放宽。
    let constraints: Vec<aevum_solver::Constraint> =
        packages.iter().map(|p| aevum_solver::Constraint::unconstrained(p.as_str())).collect();
    resolve_constraints_opt(layout, &constraints, lock_name, None, repair, None, None)?;

    // 2~4. propose → verify 门禁 → 激活(与 intent 路径共用后半段)。
    maintain_from_lock(
        layout,
        lock_name,
        mirror,
        candidate_gen,
        active_lock_name,
        foundation_manifest_path,
        confirm,
    )
}

/// maintain 主循环的**后半段**:从已写好的 `locks/<lock_name>.lock` 起 —— propose 候选世代 → verify 门禁 → 激活。
///
/// 抽出供两个入口共用:显式包名([`maintain`])与自然语言意图(CLI `--intent`:翻译+确认后写 lock 再调此)。
/// 这样"AI 翻译意图"只发生在 lock 之前(ADR-0003:可复现只来自 lock,AI 不进 propose/verify)。
/// unix 专有。
#[cfg(unix)]
pub fn maintain_from_lock(
    layout: &Layout,
    lock_name: &str,
    mirror: &str,
    candidate_gen: u64,
    active_lock_name: Option<&str>,
    foundation_manifest_path: Option<&Path>,
    confirm: bool,
) -> Result<MaintainOutcome, Box<dyn std::error::Error>> {
    // 读回 lock 取包数(propose 内部也会用它下载/造世代)。
    let lock = parse_lock_file(&layout.locks_dir().join(format!("{lock_name}.lock")))?;
    let resolved_packages = lock.locked.len();

    // propose:造候选世代,不触碰 active。
    let report = propose_generation(layout, &lock, mirror, &[], candidate_gen)?;

    // 把 lock 记录的模板(及版本)写进世代 templates.txt(验收7控制面:世代构建即记录所用模板)。
    // 旁路文件,不影响 propose/verify/激活;无模板记录则写空。
    let lock_path = layout.locks_dir().join(format!("{lock_name}.lock"));
    let templates = read_lock_templates(&lock_path);
    let _ = record_generation_templates(layout, candidate_gen, &templates);

    // verify 门禁激活。
    let activation = activate_verified(
        layout,
        lock_name,
        candidate_gen,
        active_lock_name,
        foundation_manifest_path,
        confirm,
    )?;

    Ok(MaintainOutcome {
        resolved_packages,
        candidate_gen,
        store_objects: report.store_objects,
        activation,
    })
}
