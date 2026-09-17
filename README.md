# Zoxide Native Shell Plugin

将 [zoxide](https://github.com/ajeetdsouza/zoxide) 的 `add` / `query` 热路径编译为 Shell 原生动态库，实现进程内目录跳转，消除每条命令启动新进程的开销。数据库遵循原版 zoxide 的并发模型：每个命令打开并关闭 `db.zo`，因此其他 shell / 进程的更新不会被常驻内存覆盖。

## 工作原理

```
传统模式（每次 add / query 都启动新进程）:
  precmd hook → zoxide add -- $PWD → 进程启动 → 打开+解析 db.zo → 写回 → 退出
  z foo      → zoxide query foo  → 进程启动 → 打开+解析 db.zo → 输出 → 退出

Native 模式（进程内执行）:
  chpwd hook → ZOXIDE_ADD_PATH=$PWD zoxide_add       → zsh builtin → FFI → zoxide cmd::Add
  z foo      → ZOXIDE_QUERY_KEYWORDS=(foo) zoxide_query → zsh builtin → FFI → ZOXIDE_RESULT
```

FFI 不再持有常驻 `Database`，而是直接构造上游 `cmd::Add` / `cmd::Query` /
`cmd::Remove` 并执行其 `Run` 实现。每次命令都像原版二进制一样打开、保存、
关闭 `db.zo`，复用上游已验证的并发行为。

## 支持平台

| Shell                | 平台                  | 加载方式                     |
| -------------------- | --------------------- | ---------------------------- |
| zsh                  | Linux, macOS          | `zmodload zoxide_native`     |
| pwsh (PowerShell 7+) | Windows, Linux, macOS | `Import-Module zoxide-native` |

## 编译

### 前置条件

- **CMake 3.21+**
- Rust 1.88+ (`cargo`)
- GCC 或 Clang (Linux/macOS，用于编译 zsh shim)
- .NET SDK 8.0+ (仅 pwsh 模块)

### 一键构建

仓库根目录已经提供了 `zoxide -> ../zoxide` 和 `zsh-5.9.2 -> ../zsh-5.9.2` 两个符号链接。CMake 默认直接使用它们：

```bash
cmake -B build -S .
cmake --build build --config Release
```

如果本地没有这两个 checkout，CMake 会从 GitHub 下载 `in-process` 分支的 zoxide、从 SourceForge 下载并配置 zsh 5.9.2。若本地 zoxide checkout 只读，CMake 会自动复制到构建目录中供 cargo 使用。

### 使用本地源码

```bash
cmake -B build -S . \
  -DZOXIDE_SOURCE=/path/to/zoxide_source \
  -DZSH_SOURCE=/path/to/configured_zsh_source
cmake --build build --config Release
```

### 仅构建特定模块

```bash
# 仅 zsh 模块
cmake -B build ... -DBUILD_PWSH_MODULE=OFF

# 仅 pwsh 模块
cmake -B build ... -DBUILD_ZSH_MODULE=OFF
```

### 安装

```bash
# zsh: 安装到 ~/.local/lib/zsh/zoxide-native；pwsh: 安装到 ~/.local/share/pwsh/modules/zoxide-native
cmake --install build --config Release --prefix ~/.local
```

### CMake 选项

| 选项                 | 默认值    | 说明                                                    |
| -------------------- | --------- | ------------------------------------------------------- |
| `ZOXIDE_SOURCE`      | `AUTO`    | zoxide 源码：`AUTO` 使用符号链接/GitHub，或本地路径      |
| `ZSH_SOURCE`         | `AUTO`    | zsh 源码：`AUTO` 使用符号链接/下载 zsh 5.9.2，或本地路径 |
| `ZSH_VERSION`        | `5.9.2`   | 下载 zsh 的版本（ZSH_SOURCE=AUTO 时）                   |
| `BUILD_ZSH_MODULE`   | `ON`      | 构建 zsh 可加载模块                                     |
| `BUILD_PWSH_MODULE`  | `ON`      | 构建 PowerShell 二进制模块                              |
| `BUILD_TESTS`        | `ON`      | 构建测试目标                                            |

生成器选择：

```bash
# Linux/macOS
-G "Ninja"

# Windows (Visual Studio)
-G "Visual Studio 17 2022"
```

## 使用

### zsh

#### 方式一：插件管理器

先编译并安装：

```bash
cmake -B build -S .
cmake --build build --config Release
cmake --install build --config Release --prefix ~/.local
```

然后在插件管理器里加载本仓库（`zsh_src/zoxide-native.plugin.zsh` 是插件入口）：

| 管理器    | 配置示例                                                                                             |
| --------- | ---------------------------------------------------------------------------------------------------- |
| oh-my-zsh | `git clone <repo> ~/.oh-my-zsh/custom/plugins/zoxide-native`，然后 `plugins=(... zoxide-native)` |
| zinit     | `zinit light <user>/shell-integrated-zoxide`                                                         |
| antigen   | `antigen bundle <user>/shell-integrated-zoxide`                                                      |
| zplug     | `zplug "<user>/shell-integrated-zoxide", use:"zoxide-native.plugin.zsh"`                            |

#### 方式二：手动加载

```zsh
# 使用未安装的构建产物
# Ninja Multi-Config / Visual Studio: zsh_src/build/Release（或 Debug）
# 单配置 Ninja / Makefiles:              zsh_src/build
export ZOXIDE_NATIVE_DIR=/path/to/shell-integrated-zoxide/zsh_src/build/Release
source /path/to/shell-integrated-zoxide/zsh_src/zoxide-native.plugin.zsh
```

插件会：

- 加载 `zoxide_native` 模块（zsh 在所有平台都使用 `.so` 后缀）；`boot_` 初始化进程内全局 session（只保存计数器，数据库按命令打开/关闭），`cleanup_` 在卸载时关闭它
- 注册 `chpwd` hook：工作目录变化时调用进程内 `zoxide_add`（原版默认的 pwd hook）
- 定义 `z` / `zi`：调用进程内 `zoxide_query`，结果写入 `$ZOXIDE_RESULT`
- 安装与原版 `zoxide init zsh` 一致的补全
- 查询失败按原版二进制行为写 stderr：no-match 输出 `zoxide: no match found`，fzf Ctrl-C 静默；`__zoxide_z` / `__zoxide_zi` 返回 1 且不切换目录

可用内置命令都是零参数 builtin，输入/输出通过 zsh 变量传递：

| Builtin          | 读取变量                                            | 写入变量          |
| ---------------- | --------------------------------------------------- | ----------------- |
| `zoxide_add`     | `ZOXIDE_ADD_PATH`（标量或数组）、`ZOXIDE_ADD_SCORE`（可选） | —                 |
| `zoxide_query`   | `ZOXIDE_QUERY_KEYWORDS`（数组）、`ZOXIDE_QUERY_EXCLUDE`、`ZOXIDE_QUERY_BASE_DIR`、`ZOXIDE_QUERY_ALL`、`ZOXIDE_QUERY_INTERACTIVE`、`ZOXIDE_QUERY_LIST`、`ZOXIDE_QUERY_SCORE` | `ZOXIDE_RESULT` |
| `zoxide_remove`  | `ZOXIDE_REMOVE_PATHS`（标量或数组）                 | —                 |
| `zoxide_version` | —                                                   | `ZOXIDE_VERSION`  |
| `zoxide_stats`   | —                                                   | `ZOXIDE_STATS_*`  |

```zsh
ZOXIDE_ADD_PATH="$PWD" zoxide_add

typeset -ga ZOXIDE_QUERY_KEYWORDS
ZOXIDE_QUERY_KEYWORDS=(proj)
ZOXIDE_QUERY_EXCLUDE="$PWD"
zoxide_query && print -r -- "$ZOXIDE_RESULT"
```

### pwsh

```powershell
# 开发构建产物
Import-Module ./pwsh_src/ZoxideNative/bin/Release/net8.0/zoxide-native.psd1

# 或安装后
Import-Module ~/.local/share/pwsh/modules/zoxide-native/zoxide-native.psd1
```

模块会：

- 以 `zoxide-native` 模块名加载（清单 `zoxide-native.psd1` + 脚本模块 `zoxide-native.psm1`）
- 加载托管封装 `ZoxideNative.dll`（P/Invoke 调用 `zoxide_ffi`）
- 导入时创建进程内全局 session（`Initialize`，内部 `zo_init`），移除模块时销毁（`Shutdown`，内部 `zo_shutdown`）；每个 `add` / `query` / `remove` 都独立打开/关闭数据库，与原版二进制行为一致
- 把原版 pwd/prompt hook 改为只在工作目录变化时调用进程内 `zoxide add`
- 定义 `z` / `zi` 别名，查询直接调用静态 `[ZoxideNative.Session]::Query(...)`
- 查询失败不抛异常：no-match 输出 `zoxide-native: no match found`，fzf Ctrl-C 静默退出
- 导出 `Get-ZoxideNativeVersion` / `Get-ZoxideNativeStats` / `Set-ZoxideEnv` / `Remove-ZoxideEnv`

Linux/macOS 上 `$env:_ZO_*` 只更新 .NET 环境块，Rust 的 `getenv()` 看不到。请使用：

```powershell
Set-ZoxideEnv _ZO_MAXAGE 10000
Set-ZoxideEnv _ZO_RESOLVE_SYMLINKS 1
```

`PATH` 也一样：`zi` 通过原生 `getenv("PATH")` 查找 `fzf`，导入模块之后再修改
`$env:PATH` 不会同步到原生环境。请在 profile 中先完成 PATH 设置、最后再导入
zoxide-native（和原版 `zoxide init powershell` 一样要求最后加载）；确实需要
在导入后修改 PATH 时，使用 `Set-ZoxideEnv PATH ...` / `Remove-ZoxideEnv PATH`。

## 测试

```bash
cmake --build build --config Release --target test-rust        # zoxide-ffi Rust 单元测试
cmake --build build --config Release --target test-upstream    # 上游 zoxide session/db 单元测试
cmake --build build --config Release --target test-zsh         # zsh 集成测试
cmake --build build --config Release --target test-zsh-unload  # zsh 卸载/重载回归测试
cmake --build build --config Release --target test-pwsh        # pwsh 集成测试
cmake --build build --config Release --target check            # CTest（FFI 系统测试）
```

`tests/` 中的测试分层（安装了 fzf 时，zsh/pwsh 集成测试会额外覆盖 `--interactive` 查询）：

| 测试                        | 类型       | 说明                                        |
| --------------------------- | ---------- | ------------------------------------------- |
| `test-rust` / `test-upstream` | 单元测试 | `rust_src` 全局 session/FFI 测试 + 上游 zoxide session/db/权限测试 |
| `ffi_smoke.c`               | 系统测试   | `dlopen` 真实 `libzoxide_ffi`，验证 ABI 与全局 session 生命周期 |
| `test_zoxide_zsh.sh`        | 集成测试   | zmodload、变量协议、跨进程可见性、非 ASCII EXCLUDE/BASE_DIR、插件函数 |
| `test_unload_zoxide_zsh.sh` | 集成测试   | 失败加载可重试 + 反复 `zmodload -u` / `zmodload` 并校验计数器归零 |
| `test_pwsh.ps1`             | 集成测试   | 模块导入/移除重载、hook、z/zi、fzf、失败查询不抛异常 |

zoxide 上游的 `session.rs` 测试由 `test-upstream` 目标执行。
