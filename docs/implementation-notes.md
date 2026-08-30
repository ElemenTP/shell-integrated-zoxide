# Implementation Notes

本文档记录将 zoxide 改造为 shell 进程内插件过程中的设计决策与踩坑记录。

## 1. 上游源码复用

### 1.1 in-process feature

zoxide 的 `in-process` fork 在 `Cargo.toml` 中增加了 `in-process` feature，
`src/lib.rs` 暴露 config/db/error/util，并在 feature 开启时额外暴露
cmd/import/shell/session。`rust_src` 用普通 path dependency 消费：

```toml
zoxide = { path = "../zoxide", default-features = false, features = ["in-process"] }
```

`Session` 不再实现自己的 add/query/remove 逻辑，而是直接构造上游
`cmd::Add` / `cmd::Query` / `cmd::Remove` 并执行 `Run`。数据目录仍完全由
zoxide 自己的 `config::data_dir()`（`_ZO_DATA_DIR` 或平台默认目录）决定，
保证与原版二进制一致。

### 1.2 只读 checkout 的处理

沙箱中 `zoxide -> ../zoxide` 可能是只读 checkout，而 zoxide 的 `build.rs`
会向 `contrib/completions` 写补全文件。CMake 先用
`cmake -E touch .cmake-write-test` 探测可写性；不可写时把源码复制到
`${CMAKE_BINARY_DIR}/zoxide-src`，并让 repo-root 的 `zoxide` 符号链接指向该
副本。正常开发环境仍直接使用原 checkout。

## 2. Rust FFI 层

### 2.1 不常驻 Database：直接执行上游 cmd::*

第一版 `Session` 在模块生命周期内持有 `Database`。这带来严重的数据丢失：
另一个 shell / `zoxide` 进程写库后，当前 session 内存中的旧数据会把对方的新
条目覆盖掉。zoxide 的 db 格式没有并发锁或合并协议，不能像 SQLite 那样安全
常驻。

因此 `Session` 退化为无状态的薄封装：

```rust
Cmd::Add(Add { paths: vec![path], score: Some(score) }).run()?;
Cmd::Remove(Remove { paths: vec![path.to_owned()] }).run()?;
```

每个命令内部由上游 `Database::open()` 打开、保存并关闭 `db.zo`，与
`zoxide` 二进制的并发行为完全一致。`Session` 只保留 `adds/queries/removes`
计数器；`entry_count()` 按需临时打开一次数据库读取。

`Session::query` 同样构造 `cmd::Query`，但 `Run` 直接写进程 stdout，FFI
需要返回字符串。上游 `Query` 增加了
`run_with_writer<W: Write>(&self, writer: &mut W)`：`Run` 传
`io::stdout().lock()`，session 传 `Vec<u8>`，查询算法本身仍是上游同一份
`query_with_writer` 实现，后续上游变更可直接同步。

### 2.2 查询返回约定

上游 CLI 的 stdout 行为不完全一致：`query` 单项和 `--list` 用 `writeln!`
（带换行），`--interactive` 用 `print!`（不带换行）。FFI 返回的是“结果
字符串”而不是 stdout，`Session::query` 用 `Vec<u8>` 捕获
`run_with_writer` 的输出后统一 `trim_end_matches(['\n', '\r'])`，因此：

- 单项 / interactive：路径或 `score\tpath`，**无尾随换行**
- `--list`：多行用 `\n` 连接，**无最后换行**

这样 shell 模块拿到 `ZOXIDE_RESULT` 后可以直接 `cd`，行为等价于原版
`$(zoxide query ...)` 命令替换。

### 2.3 panic 隔离与错误记录

- 所有导出函数包裹 `catch_unwind`
- `LAST_ERROR` 是全局 `Mutex<Option<CString>>`，不使用 `thread_local!`
  （避免 macOS/Windows 在宿主线程退出时调用已卸载 DSO 的 TLS destructor）
- 只有 `zo_session_query` / `zo_last_error` 的返回值需要 `zo_free`

### 2.4 为什么没有 fork guard

zoxide 没有 tokio/rayon，没有全局线程池，`Database` 只是普通内存结构。
zsh 插件通过零参数 builtin 写参数，不在 `$(...)` 中调用 FFI。单线程 + 无
fork 调用 = 不需要 PID 检查。

## 3. zsh 模块

### 3.1 零参数 builtin + zsh 变量

最初使用 `zoxide add -- PATH` 这类 dispatch builtin 和纯 C 参数解析器，但
zsh shim 因此引入了与 zsh 本身无关的复杂度。改造后模块只保留 5 个零参数
builtin，参数和返回值都走 zsh 变量：

| Builtin          | 输入变量                                            | 输出变量          |
| ---------------- | --------------------------------------------------- | ----------------- |
| `zoxide_add`     | `ZOXIDE_ADD_PATH`（标量或数组）、`ZOXIDE_ADD_SCORE` | —                 |
| `zoxide_query`   | `ZOXIDE_QUERY_KEYWORDS`(数组)、`ZOXIDE_QUERY_EXCLUDE`、`ZOXIDE_QUERY_BASE_DIR`、`ZOXIDE_QUERY_ALL`、`ZOXIDE_QUERY_INTERACTIVE`、`ZOXIDE_QUERY_LIST`、`ZOXIDE_QUERY_SCORE` | `ZOXIDE_RESULT` |
| `zoxide_remove`  | `ZOXIDE_REMOVE_PATHS`（标量或数组）                 | —                 |
| `zoxide_version` | —                                                   | `ZOXIDE_VERSION`  |
| `zoxide_stats`   | —                                                   | `ZOXIDE_STATS_*`  |

这与 starship 模块中 `starship_prompt` 读取 `STARSHIP_*` 参数并写回
`STARSHIP_PROMPT` 的模式一致。

### 3.2 结果走参数而不是 stdout

`zoxide_query` 在当前 shell 中执行，成功时 `setsparam("ZOXIDE_RESULT", ...)`，
失败时 `unsetparam("ZOXIDE_RESULT")` 并返回 1。失败信息不经过 `zwarnnam`
（那会输出 zsh 的 `zsh:zoxide_native:行号: ...` 格式），而是按原版二进制格式
写到 stderr：错误消息非空时输出 `zoxide: <message>`，错误消息为空
（fzf Ctrl-C 的 `SilentExit`）时什么都不输出。插件直接：

```zsh
if __zoxide_query "$@"; then
  __zoxide_cd "${ZOXIDE_RESULT}"
else
  return 1
fi
```

`else` 分支的 `return 1` 也很重要：zsh 的 `if` 结构在条件为假且没有
`else` 时会返回 0，否则 `__zoxide_z` / `__zoxide_zi` 会把查询失败吞掉。

没有命令替换，因此 FFI 不在 fork 出来的子 shell 中运行。

### 3.3 zsh 参数读写与 metafy/unmetafy

zsh 内部存储的字符串是 metafied 形式，所有 `>0x80` 字节都用 Meta 字符转义。

**写入**：Rust 返回的路径是原始 UTF-8，直接 `setsparam` 会经过 zsh 的
metafication 往返，多字节 UTF-8（emoji 等）会被破坏。写入前必须 metafy：

```c
setsparam(name, metafy((char *)val, strlen(val), META_DUP));
```

**读取标量**：`getsparam()` 返回参数表中的 metafied 字符串，不能直接传给
Rust。`getsparam_u()` 返回的 unmetafied 副本在 zsh 的**静态缓冲区**里，
下一次 `getsparam_u()` 会覆盖它。连续读取两个含非 ASCII 的标量
（如 `ZOXIDE_QUERY_EXCLUDE` + `ZOXIDE_QUERY_BASE_DIR`）必须立即 `ztrdup()`：

```c
static char *get_str_param(const char *name) {
  char *val = getsparam_u((char *)name);
  if (!val || !*val)
    return NULL;
  return ztrdup(val);   /* caller: zsfree() */
}
```

`tests/test_zoxide_zsh.sh` 的 `🚀-projects` 用例让 exclude 和 base_dir 同时
包含 emoji，覆盖该别名问题。

**读取数组**：`getaparam()` 返回的数组项也是 metafied 且指向参数表。
不能原地 `unmetafy()`，否则会破坏 shell 参数。需要先 `zarrdup()` 深拷贝，
再逐项 `unmetafy()`，FFI 调用结束后 `freearray()`：

```c
char **copy = zarrdup(arr);
for (size_t i = 0; i < n; i++)
    unmetafy(copy[i], NULL);
/* ... call FFI ... */
freearray(copy);
```

`tests/test_zoxide_zsh.sh` 使用 `alpha-🚀` 目录和关键词覆盖该路径。

### 3.4 卸载安全

`cleanup_` 只做 `zo_session_destroy` + `setfeatureenables(..., NULL)`。
没有线程池需要 shutdown。`tests/test_unload_zoxide_zsh.sh` 连续三轮
`zmodload -u` / `zmodload` 验证会话能重新创建。

## 4. PowerShell 模块

### 4.1 LibraryImport 的 string marshalling

`SessionCreate` / `SessionAdd` / `SessionRemove` 直接声明 `string?` 参数，
`[LibraryImport(..., StringMarshalling = StringMarshalling.Utf8)]` 会把空引用
封送为 NULL，不需要手工分配字符串。查询的 keyword 数组仍需要手工分配
`char *[]`，由 `NativeInput` 统一持有并在 finally 中释放。

### 4.2 PowerShell 不能 splat 调用 .NET 方法

PowerShell 的 splatting 只适用于 cmdlet / 函数调用，不适用于 .NET 方法。
psm1 中通过 `Invoke-ZoxideQuery` 函数用位置参数调用 C# `Session.Query`，
调用侧仍可用命名参数。

### 4.3 查询失败不能抛异常

最初 `Session.Query` 在 `rc != 0` 时抛 `InvalidOperationException`，导致
`zi` 无匹配或 fzf Ctrl-C 时 PowerShell 打印一大段异常。原版二进制行为是：

- no match：stderr 输出错误信息，退出码 1
- fzf Ctrl-C：静默退出

因此 `Session.Query` 现在返回 `QueryResult { Success, Output, Error }`。
psm1 在 `Success == false` 时：

```powershell
if (-not [string]::IsNullOrEmpty($query.Error)) {
    [Console]::Error.WriteLine("zoxide-native: $($query.Error)")
}
return
```

本项目 pwsh 模块名就叫 zoxide-native，前缀保留为 `zoxide-native:`；zsh
模块则与原版二进制一致输出 `zoxide:`。

错误为空（Ctrl-C / SilentExit）时什么都不输出。`test_pwsh.ps1` 使用一个
退出码 130 的假 `fzf` 覆盖该路径。

### 4.4 双环境块问题

Linux/macOS 上 `$env:_ZO_MAXAGE = 100` 只更新 .NET 环境块，Rust 的
`std::env::var` 通过 `getenv()` 读取不到。`ZoxideEnvironment` 同时调用
libc `setenv` / `unsetenv`。`_ZO_DATA_DIR` 在创建 session 前同步到原生环境。

`PATH` 同样受此限制：`zi` 的 Rust 侧通过 `Command::new("fzf")` 读取原生
`getenv("PATH")`。约定 pwsh profile 在导入本模块前完成 PATH 设置，并把本
模块放在最后加载（和原版 `zoxide init powershell` 的要求一致）；导入后确需
修改 PATH 时必须使用 `Set-ZoxideEnv PATH ...` / `Remove-ZoxideEnv PATH`。

### 4.5 模块命名与按名称导入

`pwsh_src` 中的清单和脚本模块必须同名：`zoxide-native.psd1` +
`zoxide-native.psm1`。`RootModule` 指向 `zoxide-native.psm1`，C# 程序集
`ZoxideNative.dll` 只通过 `NestedModules` 加载。

## 5. 构建系统

### 5.1 Cargo home 隔离

沙箱的 `/opt/rust/cargo` 对普通用户只读。CMake 把 `CARGO_HOME` 指向
`${CMAKE_BINARY_DIR}/cargo-home`，自定义命令通过
`cmake -E env CARGO_HOME=... cargo build` 执行。

### 5.2 真实 OUTPUT 规则

`libzoxide_ffi.so` 由 `add_custom_command(OUTPUT ...)` 生成，依赖
`rust_src/src`、`Cargo.toml`、`Cargo.lock` 以及 zoxide 上游源码。普通
`add_custom_target` 没有 up-to-date 判断，会导致每次构建都重跑 cargo。

### 5.3 zsh 模块 RPATH

开发构建产物在 `zsh_src/build/<config>`（Multi-Config 生成器）或
`zsh_src/build`（单配置生成器）；安装后模块与 FFI 库都在
`lib/zsh/zoxide-native`。因此：

- `BUILD_RPATH` 指向 `rust_src/target/<config>`
- Linux：`INSTALL_RPATH` 为 `$ORIGIN`
- macOS：`INSTALL_RPATH` 为 `@loader_path`（macOS 没有 `$ORIGIN`）
- Linux 链接选项 `-Wl,--allow-shlib-undefined -Wl,-z,lazy`（zsh 宿主提供内部符号）
- macOS 链接选项 `-Wl,-undefined,dynamic_lookup`

### 5.4 macOS dylib 修复

zsh 通过 `DL_EXT` 宏把所有平台的 loadable module 后缀都定为 `.so`，macOS
也不例外。因此 CMake 显式设置 `SUFFIX ".so"`（即使产物是 Mach-O dylib），
插件也只搜索 `zoxide_native.so`。

Rust cdylib 在 macOS 上的默认 `LC_ID_DYLIB` 是 cargo target 的绝对路径，
链接进 `zoxide_native.so` 后会泄漏构建机路径。CMake 在 `cargo build` 后执行：

```bash
install_name_tool -id @rpath/libzoxide_ffi.dylib libzoxide_ffi.dylib
```

模块侧配合 `INSTALL_RPATH "@loader_path"`，安装目录可随意搬迁。

strip 在 macOS 使用 `-u`（只移除调试符号，保留导出符号），Linux 使用默认
`strip`。

## 6. 测试策略

| 层         | 位置                                      | 说明                                   |
| ---------- | ----------------------------------------- | -------------------------------------- |
| 单元测试   | `rust_src/src/ffi.rs`                     | FFI null 安全、输出槽清零、roundtrip、stats、错误  |
| 单元测试   | zoxide `src/session.rs` + `db`            | 跨 session 可见性、命令复用、0600 权限；`test-upstream` 目标执行 |
| 系统测试   | `tests/ffi_smoke.c`                       | dlopen 真实 .so，验证 ABI 与内存所有权 |
| 集成测试   | `tests/test_zoxide_zsh.sh`                | zmodload、变量协议、跨进程更新、非 ASCII 标量、plugin 跳转、fzf |
| 集成测试   | `tests/test_unload_zoxide_zsh.sh`         | 失败加载可重试 + 卸载/重载循环        |
| 集成测试   | `tests/test_pwsh.ps1`                     | 模块导入、hook、z/zi、fzf、查询失败不抛异常 |
