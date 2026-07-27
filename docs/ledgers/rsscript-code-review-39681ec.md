# RSScript 代码与文档全面 Review

- **Repository:** `Haofei/rsscript`
- **Reviewed commit:** `39681ec869ea2d412e797903e4b4e1886fdf12a3`
- **Review date:** 2026-07-26
- **Review perspective:** 生产环境、长期维护、安全边界、可测试性与演进成本
- **Review method:** 固定到指定提交，对 workspace、核心 crate、运行时、VM/JIT、原生 ABI、REIR、LSP、包工具、原生适配器、CI/CD 与主要设计文档进行静态审阅，并交叉核对文档声明、Cargo 配置、工作流和关键执行路径。

> **重要限制**：本次会话环境无法解析 `github.com`，因此不能在本地 clone 后独立执行 `cargo test`、Clippy、Miri、fuzz 或基准测试。报告中的行为结论来自指定提交的源码与 CI 配置静态分析；“现有测试通过”仅代表仓库工作流意图，不代表本次审阅独立复现。涉及并发时序、跨平台文件系统行为和 JIT 机器码正确性的结论，应通过本文列出的回归测试进一步验证。

---

## Summary

### 整体评分：**6 / 10**

- **作为研究原型与语言工程项目：7.5 / 10**
- **按生产级、多租户或不可信输入执行环境衡量：约 4 / 10**

评分不是因为代码“写得差”。相反，项目已经具备不少成熟工程特征：明确的架构文档、safe/unsafe 边界、严格反序列化、输入预算、差分测试、self-host 一致性、JIT hardening 和发布 provenance。扣分主要来自：**安全/策略检查尚未形成唯一且不可绕过的执行入口；资源预算在不同执行后端不一致；若干异步与文件系统边界存在可触发的拒绝服务或越界写入风险；多个核心模块体量过大，已开始抬高审计与变更成本。**

### 优点

1. **产品定位诚实。** README 明确说明 0.1.x 属于原型、不是 sandbox、能力绑定仍可能依赖作者声明、当前不应视为生产级强制安全门。这避免了把“语义证据”误宣传为隔离或形式化证明。
2. **整体分层方向合理。** 语法/HIR/检查、Review 协议、Rust lowering、包工具、运行时 ABI、REIR、LSP、JIT 与 native adapter 基本有清楚边界。
3. **unsafe 隔离意识强。** 主体 safe crate 与 `native-abi`、`vm-jit`、Metal 等显式边界分开；JIT 代码也有输入验证、寄存器/指令/分析规模上限和专门 hardening 工作流。
4. **包输入侧防御较扎实。** 包根 canonicalization、相对路径规范化、拒绝源码树符号链接、目录深度/文件数/字节数预算、子进程超时与输出上限都值得保留。
5. **REIR 决策核心倾向 fail-closed。** 生产策略会阻断 missing、unknown、excess、unverified 等状态，策略结构也拒绝未知字段。
6. **测试与发布治理优于一般原型。** 主 CI 覆盖 rustfmt、全 workspace Clippy、nextest、生成后端、自举一致性、README/示例校验和工作树新鲜度；发布包含校验和与 provenance；JIT 另有 ASan、Miri、fuzz 定时任务。
7. **命令执行多数使用 argv，而非 shell 字符串拼接。** 这显著降低了常见 shell injection 风险。
8. **数据库参数化接口基本正确。** SQLite/SQLx adapter 有参数绑定和结果集预算，未发现普通参数化路径中的直接 SQL injection。

### 主要风险

1. **策略不是统一执行边界。** 至少 VM/package native 路径存在“先构建/加载原生代码，再完成包级检查或策略执行”的顺序，`unsafe_policy` 因而可能退化为报告信息，而非真正门禁。
2. **写入侧没有统一的 capability-style 文件系统封装。** 读取路径很谨慎，但 `vendor/`、锁文件、manifest、REIR 输出等直接使用 `std::fs`，存在父目录符号链接/重解析点越界、非原子截断和并发覆盖风险。
3. **资源管理不一致。** 包扫描有预算，但 VM 默认无限 steps/memory/stdout/host calls；AOT/HTTP/LSP/异步 timer/外部 channel 等路径仍可无界消耗线程或内存。
4. **异步 I/O 所有权模型错误。** TCP 与 WebSocket 把全双工对象放在单一 mutex 中，并在阻塞式 `read/recv().await` 期间持锁，导致同连接并发写入被阻塞。
5. **核心模块过大。** `vm-jit/src/lib.rs`、`reg_vm/mod.rs`、REIR RSScript adapter、LSP `main.rs`、runtime `domain.rs` 等已经超过“一个 reviewer 可以稳定建立完整心智模型”的规模。
6. **手写解析/扫描器承担了过重的安全含义。** Terraform/HCL 采集与 native Rust “unsafe/build/network/thread”扫描是启发式实现，适合作为证据或提示，不适合作为生产级允许/拒绝的唯一依据。

### 必须修改的问题

以下建议作为 **P0，进入任何“生产门禁/不可信代码执行”版本之前完成**：

1. 统一 `load → resolve → review/check → policy enforce → build/load/execute` 执行管线，禁止任何后端绕过。
2. 修复 native dependency `cargo_features` 在 VM shim 构建与缓存键中丢失的问题。
3. 建立 `SafeFs`/`ArtifactStore`：受限根目录、拒绝符号链接/重解析点、原子替换、同步落盘、统一大小预算。
4. 将 package dependency 表示改为规范化 DAG，避免菱形依赖指数展开和重复 Review。
5. 为 VM/AOT/HTTP/进程/定时器/channel 建立默认启用的资源策略；“无限”必须显式 opt-in。
6. TCP/WebSocket 使用独立 read/write half；不得在等待输入期间阻塞发送端。
7. Terraform 收集器禁止跟随链接、增加 visited/depth/file/byte 预算，并以结构化 Terraform JSON 或成熟 HCL parser 取代手写扫描器。
8. LSP 增加 debounce、主动取消与增量 snapshot，避免快速编辑触发分析风暴。
9. 修复 `panic = "abort"` 与插件/adapter `catch_unwind`、Rayon overflow panic 之间的语义冲突。
10. 为上述边界添加 PR 必跑的攻击性回归测试，而不是只依赖周度 hardening。

### 建议优化的问题

- 拆分超大文件，按领域和不变量组织模块，而不只是机械按函数分类。
- 用显式依赖注入替代核心路径中的全局 runtime、环境变量、当前目录和直接 `std::fs`/`Command`。
- HTTP client、DB pool、JIT pipeline/device 等昂贵对象应复用，并使用超时、配额和可观测性策略。
- 辅助 workflow 与 Docker 工具链全部固定到不可变版本/提交；加入 `cargo audit` 或 `cargo deny`。
- 文档版本和 Cargo test target 列表从机器可读清单生成，避免手工漂移。
- 将 raw SQL、arbitrary MSL、native build 等高风险 API 从普通 API 命名空间中显式隔离，例如 `unsafe_execute_raw`、`compile_untrusted_disabled_by_default`。

### 长期建议

1. **建立三个清晰平面：** `Compiler Core`、`Policy/Evidence Core`、`Effectful Adapters`。核心只处理不可变数据；文件、进程、网络、时钟、数据库和 native loader 都通过 port 注入。
2. **把“资源预算”提升为一等类型。** 同一 `ResourcePolicy` 应贯穿包扫描、编译、VM/JIT、native build、网络、子进程、LSP 与 REIR。
3. **形成可证明的唯一执行入口。** 所有 CLI、LSP、CI Action 和 library API 最终都调用同一个 preparation service；策略结果产生不可伪造的 `Reviewed<T>`/`Authorized<T>` 类型后才能进入执行层。
4. **用规范化 IR 代替递归对象树。** 包依赖、调用图、证据关系都应使用 node ID + adjacency/index，避免复制、二次扫描和路径数量膨胀。
5. **按威胁模型定义支持等级。** 建议明确 `trusted-local`、`CI-untrusted-source`、`multi-tenant` 三档；当前架构更接近第一档，不能只靠开关宣称覆盖后两档。

---

## Severity 说明

- **Critical**：可直接导致跨信任域任意代码执行、稳定的敏感数据泄漏、不可接受的数据破坏，且无需额外前提。
- **High**：可绕过关键策略、导致路径越界/稳定 DoS/严重错误执行，或在项目目标威胁模型中具有高影响。
- **Medium**：需要特定条件、影响局部，或主要造成可靠性、可维护性、性能与诊断风险。
- **Low**：局部质量、文档、可观测性或边缘兼容性问题。

**本次静态审阅未确认 Critical。** 这不代表系统已经安全；多个 High 在项目被作为多租户安全门或执行不可信 native code 时，实际业务等级可能上调为 Critical。

---

# 按文件逐个 Review

## 1. Workspace、文档与交付配置

### `Cargo.toml`

#### [ARCH-01] 全 workspace 的 `panic = "abort"` 与工具型进程的可靠性目标冲突

**Severity: High**

**问题**

release profile 全局设置 `panic = "abort"`。这会让 CLI、LSP、包工具、runtime、adapter 和 native ABI 在任何未预期 panic 时直接终止进程：

- `catch_unwind` 在 release 构建中无法形成真正隔离；
- 临时文件、子进程、锁和输出可能无法走正常清理；
- LSP 一处插件/分析 panic 会结束整个语言服务器；
- adapter 中本来应转换为脚本错误的整数溢出等情况会变成进程级崩溃。

`packages/rayon/native/rust/src/lib.rs` 中的 checked arithmetic 失败会主动 `panic!`，与该 profile 组合后尤其危险。

**为什么需要修改**

`panic=abort` 可以缩小二进制并简化某些 FFI/JIT 边界，但不应无差别施加给长生命周期工具进程和用户输入驱动的服务。

**修改收益**

- 单次用户输入错误不会终止整个 LSP/CLI 服务；
- FFI panic 隔离语义与代码实现一致；
- 更容易收集崩溃上下文并进行失败恢复。

**推荐方案**

- 为 CLI/LSP/REIR/tooling 使用 `panic = "unwind"`；
- 仅对经过审计的独立 native/JIT worker 二进制使用 `abort`；
- 更理想的是将不可信执行放入独立 worker process，以进程隔离而非 unwind 作为最后边界；
- adapter 不得通过 panic 表示可预期输入错误，应返回 ABI error/status。

**测试**

- release profile 下触发 Rayon overflow、runtime borrow conflict、插件 panic，确认调用者得到结构化错误且主进程仍存活。

---

#### [ARCH-02] workspace 边界清楚，但 release/test profile 是“全局策略”，缺少按产品差异化配置

**Severity: Medium**

**问题**

CLI、LSP、编译器、REIR、runtime、JIT 和 native adapter 的故障模型不同，却共享同一组 profile 选择。当前 `test` profile 关闭 debug info，也会降低 CI 中 sanitizer/崩溃定位质量。

**推荐方案**

- 建立 `release-tooling`、`release-worker`、`release-jit` 等 profile；
- 对 hardening job 保留足够 debug symbols；
- 将 LTO/codegen-units 的体积优化与可靠性策略解耦。

---

### `README.md`、`docs/README.md`、`docs/architecture.md`、语言/执行/包管理/REIR 规范

#### [DOC-01] 文档治理方向优秀，但权威版本与实现清单存在漂移

**Severity: Medium**

**问题**

文档明确给出语言 v0.7、执行 v0.1、包管理设计 v0.6、REIR v0.2 的权威层级，这是优点。但架构文档声称只有四个 Cargo-facing test target，实际主 crate 还存在隔离的 `jit_cost_model` 与 `jit_env` target。多个规范版本并行演进，也增加了“某项约束到底由哪一版定义”的认知成本。

**为什么需要修改**

架构文档若不能由构建清单验证，随着 target、feature 和 backend 增长会持续失真。

**推荐方案**

- 从 `Cargo.toml`、schema version 和测试 manifest 自动生成文档矩阵；
- CI 增加 docs contract test：测试 target、feature、支持矩阵、schema 版本与文档一致；
- 每份规范增加 `implemented since`、`implementation status` 与 breaking-change policy。

---

#### [DOC-02] 测试通过固定相对路径读取文档，形成脆弱的布局耦合

**Severity: Low**

**问题**

文档新鲜度/契约测试依赖仓库固定目录。重构目录时容易出现非语义性失败，也使 crate 难以独立发布或复用。

**推荐方案**

构建阶段生成文档 manifest，测试读取显式 artifact；或通过 `CARGO_MANIFEST_DIR` 与声明式路径映射而非散落字符串访问。

---

### `.github/workflows/ci.yml`、`release.yml`、`selfhost.yml`、JIT workflows

#### [CI-01] 主 CI 很强，但辅助工作流仍使用可变 Action/toolchain 引用

**Severity: Medium**

**问题**

主 CI 和 release 对关键 Action/toolchain 有较好的 pinning；selfhost、JIT hardening/perf 等辅助流程仍出现 `checkout@v5`、`stable` 或其他可变引用。Dockerfile 还下载 “latest” nextest。

**为什么需要修改**

hardening pipeline 本身属于供应链信任根。可变引用会让同一提交在不同日期执行不同代码，也削弱故障复现和 provenance。

**推荐方案**

- 所有 workflow Action 固定完整 commit SHA；
- Rust toolchain 固定版本与 components；
- nextest、cargo-fuzz 等工具固定版本并验证 checksum；
- 使用 Renovate/Dependabot 提交显式升级 PR。

---

#### [CI-02] 缺少 PR 必跑的供应链审计，以及关键安全回归的统一门禁

**Severity: Medium**

**问题**

主测试 manifest 覆盖格式、Clippy、nextest、自举、示例和生成物，但未见 `cargo audit`/`cargo deny`。Miri、ASan、fuzz 主要在定时 JIT workflow 中，不足以阻止普通 PR 引入文件系统、runtime 或 adapter 回归。

**推荐方案**

- PR 增加锁文件漏洞/许可证/重复依赖检查；
- 为本文 P0 风险建立普通 integration test，不依赖 sanitizer 才能发现；
- 周度 fuzz 保留，同时设定 corpus regression 在 PR 中快速执行。

---

#### [CI-03] Action 的 SARIF 生成失败被 `|| true` 吞掉，降低可观测性

**Severity: Low**

**问题**

安全报告转换失败不阻断主任务是合理的可用性取舍，但当前行为可能让用户只看到“无 SARIF”而不知道转换器失败。

**推荐方案**

把转换失败记录为明确 warning、step summary 和 artifact；提供 `strict_sarif` 输入供受监管环境选择 fail-closed。

---

### `Dockerfile`

#### [SUPPLY-01] 基础镜像与 nextest 下载不可完全复现

**Severity: Medium**

**问题**

`rust:1-bookworm` 是可移动 tag；nextest 通过 latest 下载路径获取。即使源码 commit 固定，构建结果和执行工具仍可能变化。

**推荐方案**

- 基础镜像使用 digest；
- nextest 固定版本与 SHA-256；
- 使用 `curl --fail --location --proto '=https' --tlsv1.2`，避免 pipe-to-tar；
- 生成 SBOM，并在 release provenance 中记录工具链 digest。

---

## 2. 包模型与文件系统边界

### `crates/rsscript/src/package/source_set.rs`

**正面结论**

该文件是项目中较成熟的边界实现：严格 TOML 反序列化、canonical root、拒绝 absolute/`..` 路径、canonical 后 `strip_prefix`、源码遍历拒绝 symlink。建议把这里的约束抽成所有写入与 adapter 都复用的基础能力。

#### [FS-01] canonicalize 与后续 open/read 之间仍有 TOCTOU 窗口

**Severity: Medium**

**问题**

“先 canonicalize/metadata，再用路径打开”在攻击者可并发修改目录的场景中不是稳定 capability。项目当前更像本地 CLI，风险有限；若未来作为服务处理不可信 checkout，此窗口会扩大。

**推荐方案**

- Unix 使用目录 fd + `openat2`/`O_NOFOLLOW`/`RESOLVE_BENEATH`；
- Windows 使用 handle-relative 打开并拒绝 reparse points；
- 或采用 `cap-std` 一类 capability-oriented filesystem API；
- 对不可支持的平台明确降低安全等级，而不是静默退化。

---

### `crates/rsscript/src/package.rs`

**正面结论**

目录深度、文件数、总字节数、子进程超时和每流输出上限均为正确方向；Unix process group 也优于只 kill leader。

#### [PROC-01] 通过 PATH 调用外部 `kill`，子进程树终止不够可靠

**Severity: Medium**

**问题**

终止 Unix process group 时依赖环境中的 `kill` 可执行文件。PATH 被污染、工具缺失或行为不兼容时，后代进程可能继续运行；fallback 的 `child.kill()` 只保证 leader。

**推荐方案**

使用 `nix::sys::signal::killpg` 或小范围 `libc::kill`，并把 unsafe 封装在单一审计模块。Windows 使用 Job Object，确保 close/terminate 覆盖整个 process tree。

---

#### [PROC-02] 读取与复制采用“检查后再访问”，并发文件树下可能超预算或换入不同对象

**Severity: Medium**

**问题**

文件大小预算与实际读取/复制之间有时间窗口。恶意进程可以在检查后替换文件或增大文件，造成预算失真。

**推荐方案**

在已打开 handle 上读取 metadata，并以实际累计字节数作为最终预算；复制时每个 chunk 更新全局计数，超过上限立即失败。

---

### `crates/rsscript/src/package/vendor.rs`

#### [SEC-01] `vendor/` 父路径可为指向包外的符号链接，导致越界删除/写入

**Severity: High**

**问题**

`vendor_package_dir` 直接执行：

1. `create_dir_all(package_dir/vendor)`；
2. 对 `vendor/<derived-name>` 调用 `remove_dir_all`；
3. 复制依赖；
4. `fs::write(vendor/rss-vendor.json)`。

若攻击者预先把包内 `vendor` 建成指向包外目录的 symlink/reparse point，`create_dir_all` 不会把它替换为受控目录，后续路径解析会进入包根之外。最终子目录不是 symlink 时，`remove_dir_all` 甚至可能删除外部目标中的同名真实目录；复制和元数据写入也会落到外部。

**为什么需要修改**

这是明确的写入边界错误。读取侧拒绝 symlink，并不能保护写入侧。

**修改收益**

防止恶意 checkout 借 CI runner 权限修改工作区之外的文件，也统一后续 lock/metadata/output 的安全模型。

**推荐方案**

- 在任何写入前对 `package_dir` 建立 directory capability；
- 逐级拒绝 symlink/reparse point，尤其是父目录；
- `vendor` 先写入同一父目录下的新临时目录，完成校验后原子 rename；
- 更新时使用锁，避免两个 vendor 进程互相删除；
- 不要仅依赖 `canonicalize().starts_with(root)`，因为检查后路径仍可变化。

**示例接口**

```rust
pub trait ArtifactStore {
    fn replace_tree(
        &self,
        root: &Path,
        relative: &Path,
        build: impl FnOnce(&Path) -> Result<(), ArtifactError>,
    ) -> Result<(), ArtifactError>;

    fn write_atomic(
        &self,
        root: &Path,
        relative: &Path,
        bytes: &[u8],
    ) -> Result<(), ArtifactError>;
}
```

实现应持有根目录 handle，在同一目录创建临时对象，拒绝链接组件，`sync_all` 后 rename，并同步父目录。

**必须增加的测试**

- `vendor -> outside` symlink；
- `vendor/<entry>` 被并发换成 symlink；
- Windows junction/reparse point；
- 中途写失败时旧 vendor 保持完整；
- 两个进程并发 vendor 的锁与最终一致性。

---

#### [REL-01] vendor 更新不是事务性的，失败会留下半成品

**Severity: Medium**

**问题**

当前逐项删除、复制，最后写 metadata。任一依赖复制失败会留下“部分依赖已更新、metadata 仍旧或不存在”的状态。

**推荐方案**

构建 `vendor.tmp.<nonce>`，生成并校验 metadata/checksum 后单次原子替换。对跨平台无法原子替换非空目录的情况，使用版本化目录 + 小型指针文件切换。

---

### `crates/rsscript/src/cli/package.rs`

#### [SEC-02] lockfile、manifest 等直接写入会跟随最终 symlink，且不是原子替换

**Severity: High**

**问题**

`rsspkg.lock`、`rsspkg.toml` 等路径通过 `fs::write` 直接写入。恶意仓库可以预置最终路径为 symlink，使 CI runner 覆盖包外文件；进程崩溃或磁盘满也可能把原文件截断为半份内容。

**推荐方案**

所有仓库内 artifact 统一走上一节的 `ArtifactStore`：

- 拒绝 final symlink/reparse point；
- 同目录 temp file；
- write + flush + `sync_all`；
- 原子 rename；
- 必要时同步 parent；
- 对 manifest 加 advisory lock 或 compare-and-swap revision。

---

#### [QUALITY-01] `pkg add` 通过 `toml::Value` 重新序列化，会破坏注释与格式

**Severity: Medium**

**问题**

语义上只增加依赖，却可能重排整个 manifest、丢失注释和用户格式，造成大 diff 与 merge conflict。

**推荐方案**

使用 `toml_edit::DocumentMut` 做保留格式的局部修改，并对重复依赖、表/inline table 风格建立 round-trip 测试。

---

#### [REL-02] package create/add 缺少事务边界

**Severity: Medium**

**问题**

多文件创建或修改中途失败时，会留下部分目录、部分 manifest 或旧/新状态混合。

**推荐方案**

引入 `PackageMutation`：在临时目录或 journal 中完成全部写入，校验后 commit；失败自动 rollback。

---

### `crates/rsscript/src/package/dependency.rs`

**正面结论**

canonical path 去重、cycle detection、feature union 是正确基础。真正的问题发生在下游再次把图展开为递归树。

### `crates/rsscript/src/package/graph.rs`

#### [PERF-01] DAG 被按“到达路径”递归物化，菱形依赖可指数膨胀

**Severity: High**

**问题**

依赖解析阶段已经按 canonical path 建图，但 `graph.rs` 后续递归创建嵌套结果，并对每个出现位置重新 load/review。分层菱形图中，同一节点会沿多条路径重复出现；树大小由节点数转为路径数，最坏可指数增长。provider/search 再扫描这棵重复树，会进一步放大 CPU 和内存消耗。

**为什么需要修改**

这是合法 manifest 即可触发的算法性 DoS，不需要巨型源码文件。现有文件/字节预算不能限制“逻辑路径数量”。

**修改收益**

- Review 每个规范化 package/feature-set 至多一次；
- 内存从 O(路径数) 回到 O(V+E)；
- provider、capability、diagnostic 可建立索引；
- cycle、版本冲突和增量更新更容易表达。

**推荐方案**

```rust
type NodeId = u32;

struct PackageGraph {
    root: NodeId,
    nodes: Vec<PackageNode>,
    outgoing: Vec<Vec<NodeId>>,
    by_identity: BTreeMap<PackageKey, NodeId>,
}

struct PackageNode {
    canonical_root: PathBuf,
    identity: PackageIdentity,
    effective_features: BTreeSet<String>,
    review: OnceLock<Arc<PackageReview>>,
}
```

输出树只应是引用视图，不复制节点内容。Review cache key 至少包含 canonical package root、内容摘要、effective features、编译器/规则版本。

**必须增加的测试**

构造 20～30 层菱形 DAG，断言节点数线性、每个节点 Review 一次、运行时间和峰值内存受预算约束。

---

### `crates/rsscript/src/package/lock.rs`

#### [PERF-02] archive/checksum 路径缺少统一文件数、深度与总字节预算

**Severity: Medium**

**问题**

包源集有严格预算，但 archive/checksum 对更广的包目录递归并对单文件 `fs::read`，可能在超大/超深非源码树上消耗大量内存或栈。

**推荐方案**

复用统一 `TreeBudget`；流式 hash，不把完整文件读入内存；迭代式遍历；拒绝链接；把忽略规则与发布清单显式化。

---

### `crates/rsscript/src/package/native.rs`

#### [SEC-03] native 安全扫描是启发式文本扫描，不能作为强制安全边界

**Severity: Medium**

**问题**

手写 Rust 扫描器试图识别 `unsafe`、build script、网络与线程等行为，但 raw string、宏展开、条件编译、过程宏、间接调用、transitive dependency 与语义别名都可能绕过文本模式。

**推荐方案**

- 文档与 API 明确命名为 `heuristic evidence`，不得产生“已证明安全”的授权 token；
- build 阶段结合 Cargo metadata、deny policy、依赖 allowlist、rustc/MIR 或 sandboxed worker；
- native crate 默认视为任意代码执行，策略只能决定“是否允许进入隔离 worker”，而不是证明其无副作用。

---

#### [SEC-04] 手工临时目录命名与清理存在竞争和链接风险

**Severity: Medium**

**问题**

基于 PID/时间戳形成路径，并在创建前清理同名目录，不具备不可预测性和原子占有语义。在共享 temp 目录或更高权限 runner 上可能被抢占。

**推荐方案**

使用 `tempfile::TempDir` 或 directory-handle relative 的 `mkdir` with exclusive semantics；不要删除未经本进程持有 capability 的预先存在路径。

---

#### [QUALITY-02] native manifest/生成源的 schema 与语法验证不够集中

**Severity: Medium**

**问题**

部分 native 配置结构未统一 `deny_unknown_fields`；`rust_path` 等值进入生成代码前依赖字符串约束，而不是解析为受限 AST/token。

**推荐方案**

- 所有外部 manifest DTO 统一 strict schema；
- 路径、crate name、feature name 分别用 newtype 验证；
- 生成 Rust 时使用 token builder/AST，而不是模板拼接；
- cache key 包含规范化配置的完整序列化。

---

### `crates/rsscript/src/package/publish.rs`

#### [DOC-03] publish 目前主要是 dry-run/校验语义，应避免给出完整发布系统错觉

**Severity: Low**

**问题**

代码做 archive hash、review 检查与报告，但 registry transport、签名、不可变版本、重放保护等尚不是完整发布链路。

**推荐方案**

CLI/help/文档明确标记 `publish --dry-run` 的支持边界；真正发布时引入签名 provenance、registry API version、idempotency key 和 server-side digest verification。

---

## 3. 执行准备、CLI 与后端一致性

### `crates/rsscript/src/cli/run_cmd.rs`、`crates/rsscript/src/package/native.rs`、native binding loader/shim

#### [SEC-05] native 代码可在包级 Review/策略门禁之前构建和加载

**Severity: High**

**问题**

至少 VM package 执行路径先准备/加载 native bindings，再进入完整包 lowering/check/review 语义。项目中没有一个强制所有入口调用的 `prepare authorized executable` 服务。这意味着：

- `unsafe_policy` 或 capability policy 可能只影响报告，不阻止 native build/load；
- CLI、测试、LSP、library API 可能逐渐形成不同顺序；
- 未来增加后端时很容易再次漏掉门禁。

**为什么需要修改**

安全策略只有在“未获得授权类型就无法调用执行 API”时才是边界；依赖调用者记住正确顺序不是边界。

**修改收益**

统一 CLI/CI/LSP/library 行为，消除策略旁路，并使测试只需验证一条执行准备管线。

**推荐方案**

```rust
pub struct ReviewedPackage { /* private fields */ }
pub struct AuthorizedPackage { /* private fields */ }
pub struct ExecutablePackage { /* private fields */ }

pub fn prepare_executable_package(
    request: PackageRunRequest,
    policy: &ExecutionPolicy,
    services: &ExecutionServices,
) -> Result<ExecutablePackage, PrepareError> {
    let loaded = services.loader.load(request.root)?;
    let resolved = services.resolver.resolve(loaded)?;
    let reviewed = services.reviewer.review(resolved)?;
    let authorized = policy.authorize(reviewed)?;
    services.backend_builder.build(authorized, request.backend)
}
```

`build_native`、`dlopen`、JIT/AOT execute API 接受 `AuthorizedPackage`，其构造函数不公开。

**必须增加的测试**

- unsafe native package 在 VM/AOT/测试/Action 所有入口都在 build 前失败；
- 失败时断言 build script 没有被执行；
- policy/report 版本变化会使旧 authorization/cache 失效。

---

#### [BUG-01] native dependency 的 `cargo_features` 在 VM shim 构建和 cache key 中被丢弃

**Severity: High**

**问题**

manifest 能声明 native dependency 的 Cargo features，但 VM shim 只把 crate name/path 写入依赖，未把 feature 集传入生成的 Cargo.toml；缓存身份也未完整包含 features。结果可能是：

- 构建出的 native 行为与 manifest/Review 看到的配置不同；
- 切换 features 后错误复用旧 artifact；
- required feature 未启用导致编译失败，或 default feature 意外启用。

**推荐方案**

```rust
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
struct NativeDependencyBuildSpec {
    canonical_path: PathBuf,
    package_name: String,
    crate_name: String,
    default_features: bool,
    cargo_features: BTreeSet<String>,
}
```

生成依赖时显式写入 `features = [...]` 与 `default-features = ...`；cache digest 使用完整规范化 spec、target triple、profile、rustc/Cargo 版本、lockfile digest 和 ABI version。

**必须增加的测试**

同一 native crate 提供互斥 feature 行为，连续运行 feature A/B，验证生成 Cargo.toml、输出与 cache artifact 都不同。

---

### `crates/rsscript/src/cli/run_cmd.rs`

#### [PERF-03] VM CLI 默认关闭绝大多数资源限制

**Severity: High**

**问题**

`VmLimits` 支持 steps、memory、stdout、host calls 等预算，但默认值除调用深度外多为 `None`；普通 `rss run --vm` 使用无限配置。循环、递归分配、巨量输出或高频 host call 可让进程长时间占用 CPU、OOM 或把输出缓冲撑爆。

**推荐方案**

- 提供 `VmLimits::safe_default()` 并作为 CLI/agent 默认；
- `--trusted-unlimited` 才允许关闭预算，且打印明显警告；
- limits 通过统一 `ResourcePolicy` 传递，不由每个后端自行解释；
- stdout API 真正流式且只保留 bounded diagnostic tail。

建议初始值可从 steps、heap bytes、stdout bytes、host calls、wall-clock 五个维度设置，具体数值通过真实 workload 基准校准，而不是硬编码为永久 ABI。

---

#### [PERF-04] AOT/程序执行使用 `Command::output()`，子进程输出可无界占用内存

**Severity: High**

**问题**

构建或运行的 stdout/stderr 被完整收集。恶意程序持续输出即可使父进程 OOM；超时若未统一应用，进程还可能永久运行。

**推荐方案**

复用包工具中已有的 capped stream reader/process-group supervision：

- 并发 drain stdout/stderr；
- 每流硬上限与总上限；
- 超限立即终止整个 process tree；
- 实时转发可选，但只保留固定大小 tail 供错误报告；
- wall-clock deadline 与 cancellation 必须贯穿 build/run。

---

#### [PERF-05] standalone source 使用无界 `read_to_string`

**Severity: Medium**

**问题**

包源集有限制，但直接运行单文件时可读取任意大文件并额外产生 UTF-8 String 分配。

**推荐方案**

统一 `read_utf8_bounded(path, max_source_bytes)`，并在诊断中报告 limit 与实际大小。

---

#### [BUG-02] 退出码窄化到 `u8` 可能改变非标准退出码语义

**Severity: Low**

**问题**

平台退出状态若被直接窄化，负值/大于 255 的语义可能被截断。Unix 最终通常只观察低 8 位，但代码应显式表达平台策略。

**推荐方案**

使用 `ExitCode`/平台适配函数，对 signal termination 单独映射并记录原因。

---

### `crates/rsscript/src/cli/test_cmd.rs`

#### [PERF-06] 测试子进程输出被完整读入 `String`

**Severity: Medium**

**问题**

测试编排有超时和并行逻辑，但 reader 线程 `read_to_string`，单个失控测试可消耗全部内存。

**推荐方案**

使用固定上限的 ring buffer；实时输出与保留 tail 分开；超限标记为明确的 `OutputLimitExceeded`。

---

#### [PERF-07] `--all` 顶层顺序执行，批处理发现失败后仍继续启动剩余任务

**Severity: Low**

**问题**

这不是正确性缺陷，但会延长失败反馈并浪费 CI 资源。

**推荐方案**

提供显式 `fail_fast` 与 `max_parallelism`；失败后不再调度新任务，但允许已启动任务做受控清理。

---

## 4. VM、JIT、Lowering 与编译器核心

### `crates/rsscript/src/reg_vm/mod.rs` 及子模块

#### [ARCH-03] 单文件仍承担过多职责，审计与优化边界不清晰

**Severity: Medium**

**问题**

`reg_vm/mod.rs` 数千行，横跨 bytecode/IR、验证、调用图、JIT eligibility、执行、host binding、limits、错误与输出。虽然已有子模块，核心不变量仍集中在一个巨型模块中。

**推荐方案**

按不变量拆分：

- `bytecode/model.rs`
- `bytecode/verify.rs`
- `analysis/call_graph.rs`
- `execution/interpreter.rs`
- `execution/limits.rs`
- `bindings/registry.rs`
- `jit/eligibility.rs`

公共构造函数只能产出 `VerifiedProgram`；执行器与 JIT 不接收未验证结构。

---

#### [PERF-08] JIT eligibility 构造可达性矩阵，时间/空间可达 O(V²)

**Severity: Medium**

**问题**

按函数数建立 reachability matrix 并重复遍历调用图，对含大量小函数的自动生成代码会产生明显二次内存和更高时间成本。

**推荐方案**

使用 Tarjan/Kosaraju SCC O(V+E)，在 condensation DAG 上计算需要的属性；若只判断递归与后端兼容性，无需完整 transitive-closure matrix。

**测试**

生成 10k～100k 函数的链、星形、SCC 与稠密图，记录 peak RSS 和分析时间，并设置回归阈值。

---

#### [QUALITY-03] “streaming output” 仍保留完整 captured stdout

**Severity: Medium**

**问题**

即使存在回调/流式接口，内部仍累计全部输出，会让调用者误以为 streaming 等同于 bounded memory。

**推荐方案**

拆成：

- `OutputSink`：实时消费；
- `DiagnosticTail`：固定大小 ring buffer；
- `OutputBudget`：累计字节并在超限时中断。

返回值不默认包含完整 stdout；需要完整捕获时由可信调用者显式选择并提供上限。

---

### `crates/vm-jit/src/lib.rs`

**正面结论**

JIT 对寄存器、参数、指令、跳转和分析规模有显式上限，unsafe 区域相对集中，并配有独立 fuzz/Miri/ASan 工作流。这些是正确的安全工程基础。

#### [ARCH-04] 13k+ 行单文件使机器码审计、平台差异与 fuzz 归因困难

**Severity: Medium**

**问题**

编码器、验证、寄存器分配/约定、平台 backend、可执行内存、trampoline、错误和测试集中在单文件。任何小改动都要求 reviewer 重新建立大范围上下文。

**推荐方案**

先按不变量而不是 ISA 指令机械拆分：

1. `validated_ir`
2. `abi`
3. `code_buffer`
4. `x86_64`/`aarch64`
5. `executable_memory`
6. `entry_stub`
7. `verification_tests`

每个 backend 导出“从 `ValidatedJitFunction` 到 sealed executable”的窄接口。为机器码 emitter 增加 golden/disassembly 与 differential tests。

---

### `crates/rsscript/src/rust_lower.rs` 及 `rust_lower/*`

#### [ARCH-05] lowering 已开始拆分，但仍与文件输出、runtime ABI 和后端测试高度耦合

**Severity: Medium**

**问题**

lowering 的纯转换逻辑与生成包布局/文件写入、runtime import 约定容易互相渗透，导致编译器核心难以无 I/O 单测。

**推荐方案**

- `LoweringContext` 输入不可变 HIR + target capabilities；
- 输出 `GeneratedUnit { files: Vec<GeneratedFile>, metadata }`，不直接写磁盘；
- 独立 emitter 负责安全、原子落盘；
- ABI symbol/version 通过 typed descriptor 注入；
- 保留现有 generated backend compile tests，并增加 snapshot normalization。

---

### `crates/rsscript/src/analyzer.rs`、`checks/*`、`syntax/*`、`hir/*`

**正面结论**

语法、HIR 与语义检查的概念边界总体清楚；规则模块化程度较高，已有属性测试/差分测试支撑。README 也诚实记录泛型替换路径存在已知 O(n³) 热点。

#### [PERF-09] 语义分析缺少统一 work budget，已知超线性路径可被自动生成源码放大

**Severity: Medium**

**问题**

已有文件大小限制不等于分析复杂度限制。深层泛型、嵌套类型、重复替换和大量诊断可能让小文件触发高 CPU/内存。

**推荐方案**

- 引入 `AnalysisBudget { nodes, substitutions, diagnostics, recursion, wall_clock_poll }`；
- substitution/memoization key 使用 interned type IDs；
- 对已知 O(n³) 路径建立基准与上限；
- 超预算返回结构化 incomplete diagnostic，而非 hang/panic。

---

### `crates/rsscript/src/lib.rs`

#### [ARCH-06] 大量 re-export 扩大公共 API 面，削弱模块封装

**Severity: Medium**

**问题**

库根暴露很多内部类型/函数，调用者可绕过推荐 orchestration，未来重构也更难维持 semver。

**推荐方案**

- 只公开 use-case facade：parse/check/review/prepare/execute；
- 内部 DTO 保持 `pub(crate)`；
- 对外返回稳定 report/schema 类型；
- 用 sealed traits 防止外部实现破坏不变量。

---

## 5. Runtime 与并发

### `crates/runtime/src/async_runtime.rs`

#### [PERF-10] native sleep 路径每次调用创建独立 OS 线程，取消后线程仍可存活到原定截止时间

**Severity: High**

**问题**

`timer_sleep_native_start*` 这一组 native timer helper 没有注册到共享 timer wheel，而是每次调用 `thread::spawn`。大量短/长 native timer 会线性消耗 OS 线程；取消 pending task 时，后台线程仍可能继续 sleep。可取消版本还会以固定间隔轮询。合作式 `TimerSleepPending` 本身不属于这一结论。

**为什么需要修改**

能够触达该 native timer helper 的不可信脚本只需创建大量 timer，就可能稳定耗尽线程、地址空间和 scheduler。

**推荐方案**

使用 Tokio timer 或单一 timer wheel：

```rust
async fn sleep_with_cancel(
    duration: Duration,
    cancel: CancellationToken,
) -> Result<(), RuntimeError> {
    tokio::select! {
        _ = tokio::time::sleep(duration) => Ok(()),
        _ = cancel.cancelled() => Err(RuntimeError::Cancelled),
    }
}
```

若不能把 API 改为 async，则向一个专用 timer service 注册 deadline，返回 cancellable registration；不得一 timer 一线程。

**必须增加的测试**

创建并取消 10k timer，断言线程数近似常数、内存受控、取消延迟有上界。

---

#### [REL-03] 完成任务的 abort handle 未及时注销

**Severity: Medium**

**问题**

Cancellation token 内部持有 task abort handles，任务完成后若不 unregister，会延长对象生命周期并使 cancel 扫描越来越慢。

**推荐方案**

注册返回 RAII guard，任务结束/取消时 `Drop` 自动注销；存储 weak handle 或 generation-tagged slot。

---

#### [PERF-11] wake key 使用 Vec 累计，重复唤醒可造成队列膨胀和重复扫描

**Severity: Medium**

**问题**

同一 task 在消费前可被多次加入 ready list；TaskGroup join 又可能在每次 wake 时扫描全部任务。

**推荐方案**

使用 `VecDeque<TaskId>` + per-task queued bit；join 维护 remaining counter/完成队列，而非全表扫描。

---

#### [ARCH-07] 全局固定 4 worker runtime 缺乏部署级配置

**Severity: Medium**

**问题**

桌面 CLI、CI runner 和服务器的 CPU/阻塞模型不同。固定 worker 数和全局 singleton 也让测试隔离困难。

**推荐方案**

注入 `RuntimeHandle`/`RuntimeConfig`；默认按可用并行度与上限计算；测试使用 current-thread runtime；阻塞任务通过独立受限 pool。

---

### `crates/runtime/src/socket.rs`

#### [CONC-01] 单一 mutex 包住 `TcpStream`，read 等待期间阻塞 concurrent write

**Severity: High**

**问题**

`read().await` 在持有 `Mutex<TcpStream>` guard 时等待网络输入。同一连接上的发送任务无法取得锁。典型请求/响应、握手或对端等待本端先发数据时可永久挂起。

**推荐方案**

连接建立后 `into_split()`：

```rust
struct RssTcpStream {
    reader: tokio::sync::Mutex<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>,
}
```

read/write 分别配置 deadline、byte budget 和 cancellation；close/shutdown 使用明确状态机。

**必须增加的测试**

- task A 先阻塞 read，task B 随后 write，断言 write 可完成；
- 双向大流量；
- read timeout/cancel；
- peer half-close。

---

### `crates/runtime/src/websocket.rs`

#### [CONC-02] 单锁包住 WebSocket sink+stream，`recv().await` 阻塞 send/ping/close

**Severity: High**

**问题**

与 TCP 相同，但 WebSocket 更严重：协议级 ping/pong、close handshake 和应用 send 都可能需要在 recv 等待时并发进行。

**推荐方案**

使用 `StreamExt::split()`，独立 sink/stream owner；限制 frame/message 大小与累计 fragmented message；加入 idle/read/write timeout。

---

#### [SEC-06] 错误信息包含完整 URL，可能泄漏 userinfo/query token

**Severity: Medium**

**问题**

连接错误或 debug 输出若记录原始 URL，`wss://user:pass@...` 或 query token 会进入日志/REIR artifact。

**推荐方案**

统一 `RedactedUrl`：只保留 scheme/host/port/path，移除 userinfo，query 默认删除或 allowlist。

---

### `crates/runtime/src/process.rs`

#### [PERF-12] `run_many` 可按调用者 jobs 值创建大量线程/子进程

**Severity: High**

**问题**

缺少全局 hard cap 和跨调用 semaphore。脚本可传入巨大 jobs，在同一进程或多次调用中造成 fork/thread storm。

**推荐方案**

- `ProcessSupervisor` 持有全局 semaphore；
- `jobs = min(requested, policy.max_processes, items.len())`；
- 每个子进程都有 deadline、output budget、process-tree handle；
- 调用取消时等待确认整个树终止。

---

#### [PROC-03] Windows 仅 kill child，无法保证终止后代进程

**Severity: Medium**

**推荐方案**

创建 Job Object，配置 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，把所有 child 关联到 job；测试 child 再 spawn grandchild 的取消行为。

---

#### [CONC-03] abort `spawn_blocking` 不会停止内部同步子进程操作

**Severity: Medium**

**问题**

取消外层 async task 只会丢弃结果，不能自动中止已经运行的 blocking closure/child process。

**推荐方案**

传入 cooperative cancellation handle，所有等待都由 supervisor 轮询 deadline/cancel 并 kill tree；不要把“abort task”当作“终止副作用”。

---

### `crates/runtime/src/domain.rs`

#### [ARCH-08] 单文件混合 HTTP、config、cache、DB、image、JSON/CSV、env 等多个领域

**Severity: Medium**

**问题**

职责、错误模型、资源预算和敏感数据策略互不相同，却共享同一模块。新增能力时容易复制 I/O 逻辑和 debug 逻辑。

**推荐方案**

拆为 `http_client`、`config`、`cache`、`database`、`image`、`structured_data`、`environment`；公共层只保留 capability registration 与 typed errors。

---

#### [PERF-13] HTTP 每次请求新建 Client、默认可无超时、整包读取 response body

**Severity: High**

**问题**

- 每次 `Client::new()` 无法有效复用连接池/DNS/TLS；
- timeout 为 0 时可能永久等待；
- `response.bytes()` 将远端可控响应完整读入内存，后续转换还可能再复制；
- retry 会重复 payload 与资源消耗。

**推荐方案**

```rust
struct HttpService {
    client: reqwest::Client,
    policy: HttpPolicy,
}

struct HttpPolicy {
    request_timeout: Duration,
    connect_timeout: Duration,
    max_response_bytes: u64,
    max_request_bytes: u64,
    max_retries: u32,
}
```

使用 streaming body 累计计数，超过上限立即取消；retry 只用于幂等操作或提供 idempotency policy，并加入指数退避/jitter 和总 deadline。

---

#### [SEC-07] HTTP/debug summary 可能包含完整 body、URL 或敏感配置

**Severity: Medium**

**问题**

body、query token、DB URL、配置值进入 diagnostics/Review artifact 会造成二次泄漏。

**推荐方案**

- 默认只记录长度、hash、content-type、status；
- header/body 使用 allowlist；
- `SecretString`/redaction type 禁止 `Debug` 输出明文；
- artifact schema 标记敏感字段并支持自动 scrub。

---

### `crates/runtime/src/channel.rs`

#### [CONC-04] external receiver bridge 一接收端一 OS 线程，内部 VecDeque 无界

**Severity: High**

**问题**

外部同步 channel 通过后台线程持续收取并放入无界队列。消费者慢或不消费时，内存无限增长；sender 长期不关闭时线程也长期存在。

**推荐方案**

- 使用 bounded queue 与 backpressure/drop policy；
- bridge 由共享受限 executor 驱动，不是一 receiver 一线程；
- 提供显式 close/cancel/join；
- metrics 暴露 queue depth、drops 和 blocked duration。

---

#### [BUG-03] capacity 从有符号整数转换到 `usize` 的跨平台边界不够明确

**Severity: Low**

**问题**

在 32-bit target 上，大正数 cast 可能截断/失败语义不清。

**推荐方案**

`usize::try_from(capacity)`，并同时校验 policy max。

---

### `crates/runtime/src/resource_pool.rs`

#### [BUG-04] 持有 `RefCell` mutable borrow 时调用 factory，重入会 panic

**Severity: Medium**

**问题**

factory 若间接再次访问同一 pool，会触发 runtime borrow panic；release abort 下成为进程崩溃。

**推荐方案**

在调用用户 factory 前释放内部 borrow；用显式 state machine 标记 `Creating`；公开 API 返回 `Result<Lease, PoolError>`，不使用 panic 表达 exhaustion/reentrancy。

---

### `crates/runtime/src/managed.rs`

#### [REL-04] 面向用户输入的 borrow conflict 仍可走 panic API

**Severity: Medium**

**问题**

虽然有 `try_*` 版本，但普通 read/write 路径使用 `RefCell` panic。编译生成代码或 native adapter 一旦调用错误，在 release 中会 abort。

**推荐方案**

生成代码与公共 runtime API 只使用 fallible borrow；panic 版本限制为内部已证明不冲突的 helper，并加 debug assertion。

---

### `crates/runtime/src/fs.rs`

**正面结论**

有 bounded read、递归遍历预算、symlink 拒绝和 atomic write helper，是良好基础。

#### [FS-02] 原子写 helper 未成为唯一写入口，durability 语义也不完整

**Severity: Medium**

**问题**

同 crate 仍存在直接 write；atomic helper 若只 rename 而不 `sync_all` 文件和父目录，在断电一致性要求下仍可能丢失。

**推荐方案**

统一到 `ArtifactStore`；明确两种契约：`atomic-visible` 与 `durable`，后者同步文件和 parent directory。

---

## 6. Native ABI、Metal 与 adapter

### `crates/native-abi/src/lib.rs`

**正面结论**

`repr(C)`、magic/version/struct size、entry 数上限、插件 allocator/free 和动态库生命周期绑定都体现了良好的 ABI 防御意识。

#### [NABI-01] `catch_unwind` 与 release abort 不一致

**Severity: Medium**

**问题**

ABI 代码看似防止 Rust panic 穿过 FFI，但 workspace release abort 使该保证在生产构建中失效。

**推荐方案**

除 profile 调整外，native plugin 最稳妥的隔离是独立进程 + versioned IPC；进程内 ABI 只支持受信插件。

---

#### [NABI-02] 先 hash 路径再 `dlopen` 存在 TOCTOU，且 hash 整文件读入内存

**Severity: Medium**

**问题**

校验与加载不是同一文件 handle，攻击者可在两步之间替换库；大 library 也会造成额外内存峰值。

**推荐方案**

- 流式 hash；
- 复制到私有、不可变 content-addressed store 后校验并从该 store 加载；
- Linux 可考虑 fd-backed `/proc/self/fd` 方案，但需跨平台抽象；
- artifact key 包含 ABI/schema/target。

---

### `crates/metal-compute/src/lib.rs`

#### [NUM-01] tensor 维度乘法与 usize→u32/u64 转换缺少 checked conversion

**Severity: High**

**问题**

`m*k`、`k*n`、`m*n`、bytes 和 thread count 等计算若溢出，release 下会 wrap，可能绕过 buffer length 校验、产生错误 allocation/dispatch，最终造成 GPU fault、进程崩溃或错误结果。

**推荐方案**

```rust
fn checked_elements(a: usize, b: usize) -> Result<usize, MetalError> {
    a.checked_mul(b).ok_or(MetalError::DimensionOverflow)
}

let grid_width = u64::try_from(elements)
    .map_err(|_| MetalError::DispatchTooLarge)?;
```

统一校验：dimension、elements、byte size、buffer count、threadgroup size、device limits。任何 raw MSL 编译都必须有 source/output/thread/input 配额。

**必须增加的测试**

接近 `usize::MAX`、32-bit 边界、空维度、device max thread/buffer 边界；在 macOS 真机 CI 验证，不只做 Linux compile。

---

#### [PERF-14] device/queue/pipeline 每调用重复构建，缺少受限缓存

**Severity: Medium**

**推荐方案**

按 `(device_registry_id, source_hash, function_name, compile_options)` 缓存 pipeline；使用有容量/LRU 的 cache，避免攻击者用无限不同 shader 填满内存。

---

### `packages/cli/native/rust/src/lib.rs`

#### [BUG-05] 手写 positionals 解析会把布尔 flag 后的真实参数误当成 value 跳过

**Severity: Medium**

**问题**

解析器看到 `--flag` 就统一跳过下一个 token，没有 option schema 区分 boolean 与 value-taking option。

**推荐方案**

使用成熟 parser，或至少定义 `OptionSpec { takes_value }`；支持 `--`、`--key=value`、重复项和未知 flag 的确定语义。

---

### `packages/crypto/native/rust/src/lib.rs`

#### [CRYPTO-01] 自行实现 constant-time equality 没有必要

**Severity: Medium**

**问题**

短小实现也可能被编译器优化改变时序；密码学边界不应维护自制 primitive。

**推荐方案**

使用 `subtle::ConstantTimeEq` 或底层库提供的 `verify_slice`，并做长度差异、空值和不同优化等级测试。

---

### `packages/http-server/native/rust/src/lib.rs`

#### [PROD-01] 示例 HTTP server 是单线程阻塞实现，缺少 shutdown、配额和错误传播

**Severity: Medium**

**问题**

作为 demo 可以接受，但若 package 名称和文档让用户误以为可生产使用，会出现慢连接阻塞、body clone、无并发控制和发送错误被忽略。

**推荐方案**

明确标为 demo-only；生产 adapter 使用成熟 HTTP server，设置 header/body/connection/timeouts、graceful shutdown 和 request cancellation。

---

### `packages/rayon/native/rust/src/lib.rs`

#### [BUG-06] 整数溢出通过 panic 表达，在 release abort 下可由输入终止宿主进程

**Severity: High**

**问题**

checked arithmetic 失败调用 panic；测试 profile 的 `catch_unwind` 不能证明 release 行为安全。

**推荐方案**

返回 `Result`/ABI status，例如 `Overflow`；native ABI wrapper 永不 panic。增加 `cargo test --release` 与宿主存活测试。

---

### `packages/sqlite/native/rust/src/lib.rs`

**正面结论**

参数化 query/execute 和结果预算是正确实现；未发现参数绑定路径中的直接注入。

#### [DB-01] 每调用打开连接，缺少 busy timeout/事务 use-case

**Severity: Medium**

**问题**

高频调用会增加连接开销，SQLite lock contention 时行为不稳定；跨多语句操作无法原子化。

**推荐方案**

受限 connection pool 或 per-package connection capability；配置 busy timeout；提供闭包/handle 型 transaction API。raw SQL execute 重命名并单独标记为高风险 API，避免用户字符串插值。

---

### `packages/sqlx/native/rust/src/lib.rs`

**正面结论**

参数化查询、row/byte budgets 和 pool 上限是值得保留的防御。

#### [DB-02] 查询无统一 deadline/cancellation，连接池注册表无淘汰策略

**Severity: Medium**

**问题**

慢查询可长期阻塞同步调用；32 个不同 URL 可填满全局 registry，若调用者不显式 close，后续合法连接被拒绝。

**推荐方案**

- 每 query/transaction 使用总 deadline；
- registry 使用授权 identity + LRU/idle expiry；
- URL 作为 secret，不进入 Debug/error/cache key 明文；
- 测试从已有 Tokio runtime 调用 adapter 的行为，避免嵌套 `block_on` 集成冲突。

---

## 7. REIR 与证据决策

### `crates/reir/src/policy.rs`、`decision.rs`、`reconciliation.rs`

**正面结论**

生产默认策略阻断 missing/unknown/excess/unverified，策略输入拒绝未知字段，属于正确的 fail-closed 核心。建议保持“纯函数决策内核”，将所有 I/O adapter 与其分离。

#### [PERF-15] reconciliation 若反复线性匹配 required/granted，规模增大后可能退化

**Severity: Medium**

**问题**

证据条目通常不大，但自动生成基础设施可产生大量资源/动作；嵌套循环与字符串比较会放大。

**推荐方案**

预先按规范化 capability key 建 `BTreeMap`/`HashMap`，显式限制 evidence facts、resources、edges 和 diagnostics 数量。

---

### `crates/reir/src/adapter/terraform.rs`

#### [SEC-08] 递归目录发现跟随 symlink，且无 visited/depth/file/byte 预算

**Severity: High**

**问题**

`Path::is_dir`/普通递归会跟随目录 symlink。恶意仓库可创建：

- 指向父目录的循环，导致无限递归/栈溢出；
- 指向仓库外的链接，采集不属于目标项目的 Terraform 文件；
- 巨型目录树，造成 CPU/内存/文件描述符耗尽。

**推荐方案**

- 使用 `symlink_metadata`，默认拒绝所有链接；
- canonical root + handle-relative traversal；
- visited file identity/inode 集；
- `max_depth/max_files/max_total_bytes/max_single_file_bytes`；
- 迭代遍历而非递归调用栈。

**必须增加的测试**

self-loop、parent-loop、outside link、深度 10k、文件 100k、稀疏巨型文件、并发替换。

---

#### [SEC-09] 手写 HCL 字符串/括号扫描不能作为生产证据解析器

**Severity: High**

**问题**

HCL 支持字符串插值、heredoc、注释、动态 block、表达式、模块与 provider 语义。手写 brace/string scanner 很容易对合法复杂输入误解析，也可能被构造为漏报。当前 production policy 的 fail-closed 默认降低了“缺失证据直接放行”的概率，但若 adapter 将错误结果包装成看似完整或已验证的 evidence，解析缺口仍可能形成策略旁路。

**推荐方案**

优先使用 `terraform show -json`/plan JSON 作为结构化输入，并验证 Terraform 版本/schema；源码预览使用成熟 HCL parser，但结果只标记为 `unverified_source_evidence`，不能提升为已验证授权。

---

#### [OBS-01] 嵌入状态/策略解析失败部分路径只忽略，诊断不足

**Severity: Medium**

**问题**

即使最终 grant 保持 fail-closed，静默忽略会让用户无法区分“确实没有证据”与“证据坏了”。

**推荐方案**

产生 machine-readable `EvidenceParseFailure`，在 production policy 下默认阻断；允许显式策略降级，但必须出现在报告摘要。

---

### `crates/reir/src/main.rs`

#### [ARCH-09] CLI orchestration、I/O、格式化和业务逻辑集中在近 2k 行单文件

**Severity: Medium**

**推荐方案**

拆分 `commands/collect.rs`、`commands/report.rs`、`commands/gate.rs`、`io.rs`、`output.rs`；command handler 只编排 use case，不直接解析所有格式与写文件。

---

#### [SEC-10] evidence/policy 输入无统一大小上限，输出直接写路径

**Severity: High**

**问题**

无界 `read_to_string` 可被巨型 JSON/TOML 造成 OOM；输出 `fs::write` 会跟随 symlink 并非原子。CI 中处理不可信 checkout 时尤其危险。

**推荐方案**

复用 `SafeFs` 和 `ResourcePolicy`；解析使用 bounded reader/serde stream；输出限定到 workspace capability 或显式受信目录；原子写入。

---

### `crates/reir/src/adapter/rsscript.rs`

#### [ARCH-10] 6k+ 行 adapter 混合解析、映射、归一化与诊断

**Severity: Medium**

**问题**

adapter 是信任语义转换边界，体量过大使“输入事实如何变成 capability”难以审计。

**推荐方案**

按 artifact schema 拆分 parser，再用纯映射层输出统一 `Fact`; 每个 mapping rule 有唯一 rule ID、spec link 和 golden fixture。禁止 adapter 直接做 policy decision。

---

## 8. LSP

### `crates/lsp/src/main.rs`

#### [PERF-16] 每次编辑复制全部打开文档并启动完整分析，旧任务只在完成后丢弃

**Severity: High**

**问题**

快速输入会形成分析风暴：多个 stale job 同时复制文档文本、读取 workspace、运行完整编译/诊断，最终只有最后一个结果被发布。revision guard 保护了正确性，但没有节省已浪费的 CPU/内存。

**为什么需要修改**

LSP 是长生命周期交互服务，典型负载正是高频小改动。大 workspace 中该模式会造成输入延迟、内存峰值和电量消耗。

**推荐方案**

- 每个 package/workspace 维护 generation + cancellation token；新编辑主动 cancel 旧分析；
- 100～250ms debounce，保存/显式请求可立即执行；
- 文档内容用 `Arc<str>`/immutable snapshot，只复制变更文档；
- 缓存解析树、symbol index、dependency graph 和磁盘文件 fingerprint；
- diagnostics pipeline 检查 cancellation/budget checkpoints。

**必须增加的测试**

对 100 个文档连续发送 1k 次 change，断言只有有限并发分析、旧 generation 迅速停止、最终诊断版本正确、peak RSS 有上界。

---

#### [CONC-05] 多个请求在 document mutex 下执行同步重工作业

**Severity: Medium**

**问题**

锁粒度过大时 completion/hover/symbol/diagnostics 会互相阻塞；同步文件读取还可能占用 async executor。

**推荐方案**

锁内只取得 `Arc<Snapshot>` 和 revision，随后释放；磁盘/CPU 工作进入受限 blocking pool；结果提交时再短暂比较 revision。

---

#### [ARCH-11] 单文件 2.5k+ 行混合 transport、state、analysis、diagnostics 与 language features

**Severity: Medium**

**推荐方案**

拆为 `server.rs`、`state.rs`、`scheduler.rs`、`workspace.rs`、`diagnostics.rs`、`features/{hover,completion,symbols}.rs`。所有 feature handler 只依赖 snapshot/query interface。

---

## 9. 测试体系

### `crates/rss-testgen`、`fuzz/*`、各 crate tests

**正面结论**

类型导向 program generation、差分/无 panic 测试、自举 parity、JIT sanitizer 与 fuzz 是项目的重要资产。当前测试更擅长证明“正常输入与生成程序的一致性”，下一阶段应补足“边界组合与资源耗尽”。

#### [TEST-01] 缺少本文关键攻击面的 PR 回归矩阵

**Severity: High**

**必须增加的测试集**

1. **文件系统**：vendor/lock/manifest/REIR output 的 symlink、junction、parent replacement、并发写与断电模拟。
2. **门禁顺序**：native build script 写 sentinel 文件；策略拒绝时 sentinel 必须不存在。
3. **feature/cache**：native cargo features 改变行为与 cache key。
4. **算法预算**：菱形 package DAG、泛型替换、JIT 大调用图、超大 evidence。
5. **并发**：TCP/WebSocket read+write；10k timer；channel slow consumer；process grandchild cancellation。
6. **网络**：无限 chunked body、慢头、慢 body、巨大 content-length、retry storm、credential redaction。
7. **LSP**：高频 revision/cancel、大 workspace、锁竞争。
8. **跨平台**：Windows Job Object/reparse point；macOS Metal 真机；Unix fd-relative confinement。

---

#### [TEST-02] fuzz 范围应扩展到原始字节与 adapter/schema 边界

**Severity: Medium**

**问题**

类型导向生成器倾向产生可接受程序，难以覆盖 lexer/parser、TOML/JSON/HCL、native manifest 和 ABI 中的畸形输入。

**推荐方案**

增加 raw-byte fuzz targets：

- source lexer/parser/check no-panic + bounded work；
- package manifest/lockfile round-trip；
- Terraform/HCL adapter；
- REIR schema/policy；
- native ABI descriptor；
- bytecode verifier/JIT validated boundary。

所有 fuzzer 配置最大输入和内部 budget，避免“fuzzer 自己 OOM”掩盖缺陷。

---

#### [TEST-03] 性能测试需要从“吞吐数字”升级为复杂度与资源不变量

**Severity: Medium**

**推荐方案**

基准不仅记录 wall time，还记录：节点访问次数、Review 次数、分配字节、峰值 RSS、线程数、打开 fd、输出字节和取消延迟。CI 对增长率设阈值，例如 DAG 层数翻倍时工作量不应四倍/指数增长。

---

# Architecture 综合评价

## 模块划分

总体方向合理，但目前属于“**分层 monorepo + 若干大型 orchestration island**”，还不是严格 Clean Architecture：

- 编译器纯逻辑与 runtime/package/CLI 已有概念边界；
- 但 `std::fs`、`Command`、环境变量、全局 runtime、直接网络/DB client 创建仍深入 use case；
- 安全策略并未通过类型系统控制所有 build/load/execute 入口；
- 巨型文件使局部模块边界不足以约束变更。

## SOLID

- **SRP：部分不满足。** `domain.rs`、LSP main、vm-jit lib、reg_vm mod、REIR adapter/main 是明显热点。
- **OCP：中等。** adapter/backend 数量说明可扩展性不错，但执行门禁与资源策略需要在多处手工复制，新后端容易漏约束。
- **LSP（里氏替换原则）：** 未发现典型继承替换问题；Rust trait 使用总体保守。
- **ISP：部分不满足。** crate root 大量 re-export、宽 runtime API 使调用者可接触不需要的内部能力。
- **DIP：不足。** 核心 use case 直接依赖文件系统、进程、网络和全局运行时，测试替身与部署策略不易注入。

## Clean Architecture 目标结构

```text
apps/
  rss-cli
  rss-lsp
  reir-cli

usecases/
  check_package
  review_package
  prepare_execution
  reconcile_evidence

core/
  syntax-hir
  review-model
  package-graph
  reir-decision
  resource-policy

ports/
  source_store
  artifact_store
  process_supervisor
  http_service
  native_builder
  clock/timer

adapters/
  local-fs
  tokio-runtime
  cargo-native
  terraform-json
  sqlite/sqlx
  vm/jit/aot
```

依赖方向必须从 apps/adapters 指向 usecases/core；core 不依赖 Tokio、reqwest、Cargo command、当前目录或环境变量。

---

# 推荐重构路线

## Phase 0：安全止血

1. VM/AOT/native 执行统一经过 `prepare_executable_package`。
2. native features 纳入 build spec/cache。
3. `vendor`、lock、manifest、REIR output 改用受限、原子 writer。
4. TCP/WebSocket split；HTTP body/timeout；VM/AOT output limits。
5. timer/channel/process 增加硬上限与取消。
6. Terraform 禁链接、加预算；复杂 HCL 只接受结构化 JSON 证据。
7. Rayon overflow 返回错误。

## Phase 1：统一资源与副作用抽象

1. 引入 `ResourcePolicy`，所有入口默认安全配置。
2. 引入 `ArtifactStore`、`ProcessSupervisor`、`HttpService`、`TimerService` ports。
3. 将全局 singleton 改为显式 `Services`/context 注入。
4. 将错误统一为 typed error，禁止用户可控路径 panic。

## Phase 2：数据结构与模块拆分

1. package tree → normalized DAG；Review memoization。
2. LSP immutable snapshots + incremental cache。
3. reg_vm、vm-jit、REIR adapter、domain.rs 按不变量拆分。
4. 限制 public re-export，建立 facade API。

## Phase 3：生产化与治理

1. 定义 trusted-local / CI-untrusted / multi-tenant 支持矩阵。
2. native/JIT 在非 trusted-local 档位进入独立 worker/sandbox。
3. CI 全固定、供应链审计、SBOM/provenance。
4. 性能与资源不变量成为 release gate。
5. 文档/测试/feature/schema 矩阵自动生成。

---

# 建议的统一资源策略示例

```rust
#[derive(Clone, Debug)]
pub struct ResourcePolicy {
    pub source_bytes: u64,
    pub tree_files: u64,
    pub tree_depth: u32,
    pub analysis_work: u64,
    pub vm_steps: u64,
    pub vm_heap_bytes: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub host_calls: u64,
    pub child_processes: usize,
    pub child_wall_time: Duration,
    pub http_request_bytes: u64,
    pub http_response_bytes: u64,
    pub http_wall_time: Duration,
    pub timers: usize,
    pub channels: usize,
    pub channel_buffer_items: usize,
}

impl ResourcePolicy {
    pub fn ci_untrusted_defaults() -> Self {
        // 数值由 workload benchmark 与威胁模型校准；关键点是默认必须有界。
        todo!()
    }
}
```

每次超限返回统一的：

```rust
ResourceLimitExceeded {
    resource: ResourceKind,
    limit: u64,
    observed: u64,
    phase: ExecutionPhase,
}
```

这样 CLI、SARIF、REIR 与 telemetry 可以一致报告，而不是每个模块使用不同字符串。

---

# 安全类别结论

- **SQL Injection：** 参数化 SQLite/SQLx API 基本正确；raw SQL API 仍允许调用者自行拼接，必须以危险命名/能力隔离和文档警告处理。
- **XSS/CSRF：** 仓库不是浏览器 Web 应用，当前不是主要攻击面；HTTP server 示例主要返回纯文本。若未来提供管理 UI，再单独建 Web threat model。
- **Path Traversal：** 读取侧较强；写入侧 symlink/reparse、Terraform 递归跟链接是主要缺口。
- **权限控制：** REIR 决策核心 fail-closed，但 native build/load 尚未被统一授权类型保护；能力证据也不等于 runtime sandbox。
- **Token/敏感信息：** URL、HTTP body、DB URL、配置和 artifact diagnostics 需要统一 secret/redaction 类型。
- **命令注入：** 多数路径使用 argv，未见核心路径将用户字符串直接交给 shell；但依赖 PATH 的 `kill` 与 arbitrary Cargo/native build 仍属于环境/代码执行边界。
- **内存/资源安全：** Rust 降低了传统 UAF，但 OOM、线程/进程耗尽、panic abort、GPU/JIT/FFI 边界仍是主要生产风险。

---

# 最终结论

RSScript 已经不是“随手写的原型”：它有清晰愿景、真实的规范体系、较强的测试文化和主动暴露风险的文档。当前最需要避免的是把这些优点误认为生产安全边界已经闭合。项目的下一阶段不应优先增加更多 adapter 或语言特性，而应先完成三件事：

1. **让 Review/Policy 成为所有执行路径不可绕过的类型与流程边界；**
2. **让所有副作用都在受限、可取消、可计量、可原子提交的 adapter 中发生；**
3. **让 package graph、LSP、VM/JIT 和 REIR 从递归复制/巨型模块演进到规范化数据与窄接口。**

完成这些 P0/P1 后，项目才适合从“可信本地研究工具”迈向“CI 中处理不可信仓库”；若目标是多租户执行不可信 native/JIT 代码，还必须增加独立进程、OS sandbox 与更强的供应链隔离，不能仅依赖语义 Review。
