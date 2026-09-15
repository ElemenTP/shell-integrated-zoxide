# Architecture: Zoxide as Native Shell Plugins

## 问题背景

zoxide 的 shell 集成在每个热路径上都启动一个独立进程：

1. **`zoxide add`**（chpwd / prompt hook）：每次目录变化 fork + exec，打开并反序列化 `db.zo`，写入后退出。
2. **`zoxide query`**（`z` / `zi` / 补全）：同样 fork + exec，并再次打开同一个数据库。

进程启动开销在小跳转上非常明显。

## 解决方案

把 zoxide 的 `add` / `query` / `remove` 编译为 C FFI 动态库，由 shell 直接加载：

- **零进程创建**：zsh 通过内置命令、pwsh 通过 P/Invoke 调用进程内实现
- **原版并发模型**：Session 不常驻 `Database`，每个命令都像 `zoxide` 二进制一样打开/保存/关闭 `db.zo`，避免旧内存覆盖其他 shell 的更新
- **fork 安全设计**：zsh 插件直接调用零参数 builtin，结果通过 `ZOXIDE_RESULT` 参数传递；不在 `$()` 中运行 FFI 调用

## 整体架构

```
┌──────────────────────────────────────────────────────────────┐
│  rust_src/  (zoxide-ffi crate, cdylib)                      │
│  ┌────────────────────────────────────────────────────────┐  │
│  │  ffi.rs — extern "C" zo_session_* API                  │  │
│  └───────────────┬────────────────────────────────────────┘  │
│                  │ path dependency, features = ["in-process"] │
│  ┌───────────────▼────────────────────────────────────────┐  │
│  │  zoxide → zoxide in-process fork                        │  │
│  │  src/session.rs — Session / QueryOptions / SessionStats │  │
│  │  config.rs / db/{mod,dir,stream}.rs / util.rs          │  │
│  └────────────────────────────────────────────────────────┘  │
└────────────────┬─────────────────────────┬───────────────────┘
                 │ cdylib(.so/.dylib/.dll) │
                 ▼                          ▼
┌─────────────────────────────┐  ┌──────────────────────────────┐
│  zsh_src/                   │  │  pwsh_src/                   │
│  module.c (C shim)          │  │  ZoxideNative/ (.NET 8)      │
│  → zoxide_native zmodule    │  │  NativeMethods.cs (P/Invoke) │
│  零参数 builtin:            │  │  Session.cs (QueryResult)    │
│    zoxide_add               │  │  Environment.cs (双环境块)   │
│    zoxide_query             │  │  加载方式: Import-Module      │
│    zoxide_remove            │  │    zoxide-native             │
│    zoxide_version           │  │  平台: Windows/Linux/macOS   │
│    zoxide_stats             │  └──────────────────────────────┘
│  加载方式: zmodload         │
│  平台: Linux / macOS        │
└─────────────────────────────┘
```

## 核心设计决策

### 1. 上游 in-process feature

zoxide 的 `in-process` fork 增加了 `lib` target 和 `session` feature：

```rust
// zoxide/src/lib.rs
pub mod config;
pub mod db;
pub mod error;
pub mod util;
#[cfg(feature = "in-process")]
pub mod cmd;
#[cfg(feature = "in-process")]
pub mod import;
#[cfg(feature = "in-process")]
pub mod session;
#[cfg(feature = "in-process")]
pub mod shell;
```

`rust_src/Cargo.toml` 通过普通 path dependency 消费它：

```toml
zoxide = { path = "../zoxide", default-features = false, features = ["in-process"] }
```

`zoxide/src/session.rs` 只暴露 `Session::new()`，数据目录完全由 zoxide 自身的
`_ZO_DATA_DIR` / 平台默认目录规则决定，保证与二进制行为一致。

### 2. Session：无状态封装，执行上游 cmd::*

`zoxide/src/session.rs` 定义了：

| 类型             | 作用                                             |
| ---------------- | ------------------------------------------------ |
| `Session`        | 无状态薄封装 + `adds/queries/removes` 计数器     |
| `QueryOptions`   | `keywords/exclude/base_dir/all/interactive/list/score` |
| `SessionStats`   | 会话内计数器                                    |

`Session` 不持有 `Database`。`add` / `remove` 构造上游 `cmd::Add` /
`cmd::Remove` 并执行 `Run`；`query` 构造 `cmd::Query` 并调用
`run_with_writer(&mut Vec<u8>)` 捕获输出。每个命令打开、保存、关闭一次
`db.zo`，因此另一个 shell 写库后，下一个命令一定看到最新数据，不会用常驻
内存覆盖磁盘。

`Session::query` 捕获上游 stdout 后统一去掉尾随 `\n` / `\r`，模拟
`$(zoxide query ...)` 命令替换语义。

### 3. 单线程，无 fork guard

zoxide 没有 tokio/rayon 或其他线程池，`Database` 只是普通的自引用数据结构。
因此 FFI 层没有 starship/atuin 那样的 `creator_pid` fork guard。

fork 安全由 shell 侧契约保证：

- zsh 插件直接调用零参数 `zoxide_query`，读取 `$ZOXIDE_RESULT`；不在 fork
  出来的子 shell 中执行 FFI
- pwsh 的函数直接调用托管方法，没有子进程
- 补全场景同样直接调用 builtin，再读 `$ZOXIDE_RESULT`

### 4. FFI API 设计

**错误即返回值**：所有可能失败的导出函数直接返回错误信息字符串，
`NULL` 表示成功；非 NULL 指针是 Rust 分配的 UTF-8 消息，调用方必须用
`zo_free` 释放。没有任何错误槽、没有全局状态。

```c
// 生命周期：handle 是载荷，所以走 out 参数，返回值是错误
char *zo_session_create(zo_session_t **session);   // NULL = 成功
char *zo_session_destroy(zo_session_t *session);   // NULL = 成功；错误仅供诊断

// 数据库操作：NULL = 成功，否则返回错误消息（须 zo_free）
char *zo_session_add(zo_session_t *, const char *path, double score);
char *zo_session_remove(zo_session_t *, const char *path);
char *zo_session_query(zo_session_t *, const zo_query_options_t *, char **out);

// 元数据 / 统计
char       *zo_session_stats(zo_session_t *, zo_stats_t *out);
const char *zo_version(void);                      // 静态字符串，不可释放
void        zo_free(char *ptr);                    // 释放结果与错误消息
```

内存约定：`zo_session_query` 成功时把结果写入 `*out`，失败时把 `*out` 置
为 NULL（调用方不能复用上一次的 out 指针），两种情况都通过返回值报告是否
出错。`zo_version` 返回静态字符串，不可释放。出错返回的消息同样由 Rust
分配，必须 `zo_free`；`zo_session_destroy` 的错误只在析构 panic 时出现，
无法补救但便于 debug，调用方打印/释放后照常收尾。

### 4.1 为什么是返回值而不是错误槽

早期版本把错误写进 `Mutex<Option<CString>>`：先是单个进程级全局槽，后来
改成"每个 session 一个槽 + 全局兜底"。两者都需要说明清除/覆盖语义，而且
"调用方读到的错误属于哪一次调用"依赖调用与读取之间没有别的调用插进来。
改为返回值后这些概念全部消失：

- **无共享状态**：错误直接从失败的调用返回给它的调用方。两个 session、
  两个线程并发失败时，各自拿到的就是自己的消息，不存在互相覆盖的可能，
  也不需要加锁（`SessionHandle` 里也不再需要 `Mutex`）。
- **无生命周期陷阱**：没有 `thread_local!`（其 TLS 析构器在宿主线程退出
  时会指向已被 `dlclose()` 卸载的 DSO——glibc 会跳过，macOS/Windows 不会，
  可能 SIGSEGV），因为根本不需要长期存在的错误存储。
- **无陈旧状态**：成功就是 NULL，失败就是消息，不存在"上一次的错误还留
  在槽里"的问题。

**空消息 ≠ 成功**：zoxide 用 `SilentExit`（fzf Ctrl-C）表示"失败但无话可
说"，此时返回的是指向空字符串的非 NULL 指针。调用方必须先判断指针，再判
断内容：

```c
char *err = zo_session_query(session, &opts, &out);
if (err) {
    if (*err)                      /* 空消息：静默失败，不打印 */
        fprintf(stderr, "zoxide: %s\n", err);
    zo_free(err);
    return 1;
}
/* 成功：out 是结果字符串，用完 zo_free(out) */
```

被 `catch_unwind` 捕获的 panic 也走同一条通道：panic 消息被格式化成错误
字符串返回，不会跨 FFI 边界展开（`zo_session_destroy` 的析构 panic 也由
`ffi_guard_error!` 捕获并作为诊断信息返回）。

### 4.2 各调用方如何取错误

| 调用方 | 代码 | 说明 |
| ------ | ---- | ---- |
| zsh `module.c` | `char *err = zo_session_add(...); if (err) { zwarnnam(...); zo_free(err); }` | 由 `report_ffi_error()` / `report_query_error()` 统一打印并释放 |
| zsh `boot_` | `char *err = zo_session_create(&g_session);` | 创建失败时 `g_session` 保持 NULL，builtin 拒绝运行 |
| zsh `cleanup_` | `char *err = zo_session_destroy(g_session);` | 析构失败只在 panic 时出现，打印后释放即可 |
| pwsh `Session.cs` | `IntPtr error = NativeMethods.SessionAdd(...);` → `TakeError(error)` | Add/Remove/Stats 抛异常，Query 写入 `QueryResult.Error`，Dispose 释放析构诊断 |
| C 系统测试 | `err = session_add(session, NULL, 1.0); CHECK(err != NULL)` | 见 `tests/ffi_smoke.c` |

### 5. Shell 集成契约

**zsh**：

- 模块文件名在所有平台都是 `zoxide_native.so`（zsh 的 `DL_EXT` 宏在 macOS
  上也使用 `.so`），FFI 库名才按平台区分
- 安装目录为 `lib/zsh/zoxide-native`，模块与 `libzoxide_ffi.*` 同目录
- 只有 `zoxide_add` / `zoxide_query` / `zoxide_remove` / `zoxide_version` /
  `zoxide_stats` 五个零参数 builtin，没有 dispatch builtin 或参数解析器
- 输入/输出全部通过 zsh 变量传递：
  `ZOXIDE_ADD_PATH`、`ZOXIDE_ADD_SCORE`、`ZOXIDE_QUERY_*`、
  `ZOXIDE_REMOVE_PATHS`、`ZOXIDE_RESULT`、`ZOXIDE_VERSION`、`ZOXIDE_STATS_*`
- `zoxide_query` 成功时 `setsparam("ZOXIDE_RESULT", metafy(...))`，失败时
  unset 结果并返回 1；失败 stderr 按原版二进制行为输出（no-match 有消息，
  fzf Ctrl-C 静默），插件用 `else return 1` 把失败状态向上传播
- 标量读取使用 `getsparam_u()` + `ztrdup()`，调用方 `zsfree()` 释放；
  `getsparam_u()` 的静态缓冲区会被下一次调用覆盖
- 字符串写入 zsh 参数前必须 `metafy(..., META_DUP)`；读取标量使用
  `getsparam_u()`，读取数组先 `zarrdup()` 再逐项 `unmetafy()`，避免 UTF-8
  高字节被 zsh 的 metafication 破坏
- `boot_` 创建 session，`cleanup_` 销毁 session；卸载/重载安全

**pwsh**：

- `Session` 是 `IDisposable` 托管封装，模块卸载时销毁；每次 Add/Query/Remove
  打开并关闭数据库，不持有常驻 db
- Linux/macOS 上 .NET 环境块与 libc 环境块分离，`ZoxideEnvironment` 同时写
  `setenv()`；`Set-ZoxideEnv` / `Remove-ZoxideEnv` 暴露给用户
- `_ZO_DATA_DIR` 在创建 session 前从 .NET 环境同步到原生环境；其余 `_ZO_*`
  在导入时同步到原生环境块
- `PATH` 也必须在导入前设置好，或通过 `Set-ZoxideEnv PATH ...` 同步；
  `zi` 的 fzf 查找使用原生 `getenv("PATH")`
- `Session.Query` 返回 `QueryResult` 而不是抛异常；失败时由 psm1 按原版
  二进制行为写 stderr（Ctrl-C 静默）

## 目录结构

```
shell-integrated-zoxide/
├── rust_src/                    # FFI crate (cdylib)
│   ├── Cargo.toml
│   └── src/lib.rs, ffi.rs
├── zoxide/                      # 符号链接 → zoxide in-process fork
│   └── src/session.rs           # Session / QueryOptions / SessionStats
├── zsh_src/                     # zsh 模块
│   ├── ffi.h                    # C ABI 头文件
│   ├── module.c                 # 零参数 zsh builtin shim
│   └── zoxide-native.plugin.zsh # zsh 插件入口
├── pwsh_src/                    # PowerShell 模块
│   ├── ZoxideNative/
│   │   ├── NativeMethods.cs     # P/Invoke + 原生库解析器
│   │   ├── Session.cs           # 托管封装 + QueryResult
│   │   └── Environment.cs       # 双环境块 helper
│   ├── zoxide-native.psm1      # 模块主体（hook/z/zi）
│   └── zoxide-native.psd1       # 模块清单
├── tests/
│   ├── ffi_smoke.c              # dlopen FFI 系统测试
│   ├── test_zoxide_zsh.sh       # zsh 集成测试
│   ├── test_unload_zoxide_zsh.sh# zsh 卸载/重载测试
│   └── test_pwsh.ps1            # pwsh 集成测试
├── CMakeLists.txt
└── docs/
    ├── architecture.md
    └── implementation-notes.md
```

## 构建系统

CMake 负责：

1. 解析/创建 `zoxide` 与 `zsh-5.9.2` 符号链接；只读 zoxide checkout 自动复制到构建目录
2. 用真实 OUTPUT 规则构建 Rust cdylib（Ninja 可以正确增量判断）
3. 编译 zsh 模块并设置 RPATH（Linux `$ORIGIN` / macOS `@loader_path`）
4. 构建 .NET 8 二进制模块（`zoxide-native.psd1` + `zoxide-native.psm1`）并把原生库复制到输出目录
5. 注册 CTest 与 `test-rust` / `test-upstream` / `test-zsh` / `test-pwsh` 自定义目标
