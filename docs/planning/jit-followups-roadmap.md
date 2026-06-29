# JIT 后续优化路线图(按难度从低到高)

> 这是 [`vm-optimizing-jit-plan.md`](./vm-optimizing-jit-plan.md) 的聚焦补充,只记录
> **还没做、接下来可能做** 的 JIT 性能项,按**实现难度从低到高**排序。每项标注难度 /
> ROI / 前置依赖 / 解锁什么 / 做法概要 / 风险。基准数据见
> [`../../benchmarks/vm-jit/README.md`](../../benchmarks/vm-jit/README.md)。

## 背景:现在到了什么程度

JIT 性能有两条轴:
- **轴 A(覆盖率 / eligibility)**——让更多代码"能 native 跑"。VM→native 的大头在这(15–50×)。
- **轴 B(代码质量)**——缩小 native 与手写 Rust 的差距。

**轴 A 的容易收益基本吃完**:OSR×J2/J3、native 递归、scalar replacement、checked-int 消除、
堆读全覆盖(Int/Bool/Float)、可回滚的堆写(field/list/map/deque + flat 就地写)、query-folding
都已 shipped。剩下的核心判断:

> **native codegen 已接近 Rust;瓶颈在覆盖率,尤其"堆分配"还几乎全 bail。**
> 最后一块"一把解锁一大片"的通用解是 **J0.4(native 堆分配 + 写后精确 deopt)**。

铁律(所有项都遵守):**native 只搬运,解释器定语义;能用可回滚兜底就别碰精确 deopt。**

---

## 排序总表

| # | 项目 | 难度 | ROI | 前置 | 一句话 |
|---|---|---|---|---|---|
| 1 | 🔶Handle-value / 非 Int key 集合写(**key 轴 + value 轴均已落地**) | 低 | **低** | 无 | **key 轴:** `MapInsertHandleKeyInt`(key 经 `heap_read_handle`→`VmMapKey` 复用宿主哈希、journaled、scalar 值);`Map<String,Int>` 热循环已 OSR 字节对齐(`native_osr_map_insert_string_key_matches_interpreter`)。**value 轴:** `ListPushHandle`(resolve value handle→journaled push)+ growth-veto 改为 type-aware(放行非参数/handle 列表增长,参数 flat 缓冲仍 veto——pin 安全;params-only 已核实),`List<String>` 累加循环已 OSR 字节对齐(`native_osr_list_push_handle_matches_interpreter`)。**Set/SortedSet<String>.insert + SortedMap<String,Int>.insert** 亦已落地(`SetInsertHandle`/`SortedSetInsertHandle`/`SortedMapInsertHandleKeyInt`,同套路)。**heap-key/value 集合写已全覆盖**:Map/List/Set/SortedSet/SortedMap insert + struct `FieldSetHandle` COW 写 + Deque front/back push(`DequePushBackHandle`/`DequePushFrontHandle`)。每条都有 OSR 字节对齐测试,差分 33/0。**#1 集合写轴完成。** |
| 2 | ~~Deque-pop Option native 融合~~ ✅**已支持(OSR 路径)** | 中 | 低–中 | 无 | OSR 用的 in-region scalar-replace(`passes.rs:5662`)已 seed deque-pop 并以"恒 Some tag + bail-on-empty 兜 None"融合;测试 `native_osr_enters_loop_with_transactional_deque_pop_front_int` 绿。(整函数 pass 未 seed,但无关紧要——热循环走 OSR) |
| 3 | 轴 B:热堆读 host-call 边界内联 | 中 | 中 | 无 | 堆读现比 Rust 慢 ~13×,瓶颈是每次跨调用边界 |
| 4 | ~~嵌套循环 OSR~~ ✅**已支持** | 中–高 | 中 | 无 | OSR 管线本就多循环感知(`detect_natural_loops`→`select_osr_candidate_loop`);嵌套/兄弟循环已可 OSR(新增回归测试坐实) |
| 5 | ~~**J0.4 S1–S3:仅分配的 native 堆写**~~ ✅**已完成** | 中–高 | **最高** | 无(不需 J0.1/J0.5) | 解锁 alloc-bound(string/json/集合构造);10 个 `AllocatesResult` helper 已落地并验证 |
| 6 | ✅**J0.5:生成代码内 `VmLimits` 记账(step+cancel+mem 全落地 OSR 层)** | 高 | 中 | 无(S4 的前置) | OSR 在 step/cancel armed 时跑(沙箱);**2026-06-29:mem 也落地(6057f48)**——`ListPush*`(唯一被解释器计入 `mem_budget` 的 native-subset op)在 helper 内按 `checked_push_accounted` 的 `grew` 记账;超预算 bail→现有 rollback+rerun 让解释器在精确那次 push 报错(免费 exact parity);clean exit 提交 `live_bytes`。判别式测试 `native_osr_list_push_int_charges_mem_budget`(宽松→OSR+完成;紧→native 与解释器逐字节同错)。#6 完成。 |
| 7 | 🔶完整 J0.1:内联帧链 + 堆值重建(**scalar Result/Option live-after 全覆盖,含两臂**) | 很高 | 高(地基) | 无(S4 的前置) | 已落地:always-Ok Result / always-Some Option live-after 重建;**两臂 scalar Result** 经 tag+payload 溶解,**dead-at-boundary 与 live-after 都支持**(live-after 在 OSR-exit 按 tag 重建 Ok/Err——`native_scalar_replace_two_armed_results_in_region` + `ResultRecipe(variant,payload,Option<tag>)` + `result_err_layout`;测试 `native_osr_j3_two_armed_scalar_result_matches_interpreter`、`native_osr_j3_two_armed_result_live_after_reconstructs`,均字节对齐+osr_entries>0,差分 33/0)。**2026-06-29 新增:heap-payload + 不同类型臂 dead-at-boundary 全覆盖**——`Result<String,String>`(同类型堆,980e7d3:修了 string-fold 误 bail + 翻译器 handle-alias 死循环→改 union-find)与 `Result<Int,String>`(异类型臂,498f041:**per-arm payload 寄存器**,`ResultRecipe(variant,ok_pay,err_pay,tag)`)现都 OSR(测试 `native_osr_two_armed_heap_string_result_*`、`native_osr_two_armed_mixed_result_*`,osr_entries>0,差分 33/0)。**2026-06-29:live-after + heap payload 也已落地(04a24de)**——`DeoptValue::Handle` 携带堆句柄,OSR-exit 经 `heap_read_handle` 解析重建;配合 try_osr 运行期参数类型 seeding(taint-gate:仅 seed 在区内只被 dissolved-payload `Move` 读的参数,堆集合 key/value 参数被 helper spec 定型故排除)。测试 `native_osr_two_armed_heap_result_live_after_reconstructs`,差分 33/0,5 个 #1 集合写循环无回归。**#7 两臂 Result 全维度完成:scalar+heap × 同/异类型 × dead-at-boundary/live-after 均 OSR。** 剩:内联帧链(输出测不出的硬核,需定向 repro/forced-deopt,#8 的地基) |
| 8 | ✅**J0.4 S4:别名堆就地写**(**能力已落地并验证**) | 很高 | 高/广 | J0.1 + J0.5 | 调用方别名的堆就地写已在 native OSR 层落地并验证**跨堆类型**:flat `List<Int/Float>` 直写(`ListSetIntDirect`);**struct 标量字段 RMW**(OSR scalar-field replacement 溶解为 loop-carried 标量 + OSR-exit 写回 + mut-param 传播给调用方);**Map insert / Deque push**(`Rc<RefCell>` 就地 helper)。靠幂等 rollback+rerun 保 §7.2。**2026-06-29 验证**:`native_osr_aliased_struct_field_write_matches_interpreter`、`native_osr_aliased_map_insert_matches_interpreter`、`native_osr_aliased_deque_push_matches_interpreter`——均判别式读回(经调用方,非仅 callee 返回)+ osr_entries≥1 + 解释器逐字节对齐。**关键**:这些循环仅当外层函数被 I/O 包裹(整函数 tier-0-INELIGIBLE)才进 OSR 路径;无 I/O 的函数体在调用点被 tier-0 直派、根本不进 OSR(故"osr_entries=0"是没进 OSR,**不是** decline)。剩:**写后精确续跑**(不 rollback、写后从循环中段精确续跑)——是 rollback+rerun 之上的**优化**,建在 #7 内联帧链上,非正确性缺口 |
| 9 | 🔶async / 挂起函数 native(**task_group spawn/join OSR 融合已落地**) | 很高 | 类相关 | — | 循环内 `task_group { async let x = f(..); await x }` 的纯 spawn/join 已内联进 native OSR(`native_callee_inlinable_j3_with_spawns`;测试 `native_osr_enters_task_group_spawn_loop`)。剩:跨 await 点的真正 park/resume 帧状态(架构性,可能不做) |

**⚠️ 2026-06-28 核实结论:9 项全部已 Done 或 In-progress(无纯 Pending)。** #2/#3/#4/#5 **Done**;
#1/#6/#7/#8/#9 **均有已落地并验证的 slice**(此前 docs 把 #1/#8/#9 误标为 Pending——代码里其实已有真实
进展):#1 = String-key map insert;#6 = step+cancel(OSR 层)在生成代码内强制;#7 = Result/Option live-after
重建;#8 = flat-list 别名直写(`ListSetIntDirect`);#9 = task_group spawn/join 的 OSR 融合。**各项剩余**(均为
更大/更硬的尾巴,非"未开始"):#1 handle-**值**写 + 其他 key 类型(按需);#6 mem_budget 记账 + 整函数/递归层;
#7 内联帧链 + heap-payload variant 重建(silent-bug-prone,需定向 repro,勿盲冲);#9 跨 await 的 park/resume(架构性)。
**2026-06-29 更新:#8 别名复合写能力已落地并验证**(struct 标量字段 RMW / Map insert / Deque push 均 OSR + 调用方判别式
对齐——见上表)。#8 仅剩**写后精确续跑**(rollback+rerun 之上的优化,建在 #7 帧链上,非正确性缺口)。**真正剩余的硬核
是 #7 内联帧链**(输出测不出,需 forced-deopt repro)**与 #9 async park/resume**(架构性)——#7 是 #8 精确续跑与
heap-payload variant / live-out OSR 的地基。

**(历史)推荐切入顺序:** ~~先 **#5(J0.4 S1–S3)**~~ ✅**#5 已完成**——
10 个 `AllocatesResult` helper(`StringFromInt`/`StringConcat`/`StringSlice`/`StringPadLeft`/`StringSplit`/
`StringLiteral`/`JsonParse`/`JsonField`/`BytesSlice`/`ListNewInt`)经 `JIT_HEAP_RESULTS` 输出表 +
`escaping_output_handle` 逃逸分析 + Model-A(`mem_budget` armed 时拒绝 native)落地并验证
(`native_string_from_int_return_allocates_heap_result` / `native_string_concat_handle_feeds_string_len` /
§7.2 双子 `native_heap_result_force_deopt_leaves_output_table_empty`)。**下一步主线:**
**#3(轴 B 堆读内联)→ #4(嵌套循环 OSR)→ #7(J0.1)→ #6(J0.5)→ #8(S4)**;**#1 / #2** 按需,**#9** 缓做。

---

## 逐项详述

### 1. Handle-value / 非 Int key 集合写 —— 难度低,ROI 低(按需)
- **现状缺口:** 集合里 Handle 类型的 value 只有读(`ListGetHandle` / `FieldHandle`),**没有写**:
  没有 `MapInsertHandle` / `ListPushHandle` / `SetInsertHandle` / `FieldSetHandle`;map/set 只支持
  `jit_int_key`(Int key),`Map<String,_>` / `Set<String>` 虽合法(String 是 Hashable)但 native 不认。
  (注:**Float 不能做 key**——不是 Hashable,checker 直接拒,这条不存在。)
- **做法:** 沿用 `MapInsertFloat` 套路,加 Handle 版 helper;key 复用解释器自己的 `VmMapKey` /
  哈希函数(**绝不在 native 重写哈希语义**),native 只在堆表里搬 handle + COW 回写。
- **为什么 ROI 低:** 贵的部分(字符串哈希、堆分配、哈希表机制)无论如何都在 host helper 里跑,
  native 只能削掉外面那薄薄一层派发(占总成本百分之几)。**唯一有意义的场景**是"一个热循环里混着
  一个这种操作,导致整个循环没法 native"——那时补它是为了**保住周围 native 循环**,不是加速这次写。
- **触发条件:** profiling 显示真有这种热循环再做;否则不投。

### 2. Deque-pop Option native 融合 —— 难度中,ROI 低–中
- **现状缺口:** `match Deque.pop_front()` 对 **Int 和 Float 都进不了 native**。`DequePopFront` 的 dst 是
  `Option`,要靠 J3(`native_scalar_replace_options`)把它和后面的 `MatchOption` 融合掉;但 J3 只从
  `MakeSome`/`LoadNone` 播种 OPT,**不认 deque-pop 直接产出的 Option**。而且 deque-pop 的 native 形态是
  "空则 bail",和 J3"产出 is_some 判别位"的模型对不上。
- **做法:** 扩 J3——把 `DequePopFront/Back` 也当 Option 生产者播种,并协调"bail-on-None"与"is_some 判别"
  两种模型(或给 deque-pop 一条独立的融合路径)。`DequePop*Float` 的 helper + lowering 已就绪
  (parity 验证过),融合一通它们立刻 native。
- **收益:** Int + Float 一起解锁,但 deque-pop 热循环本就不常见,ROI 有限。

### 3. 轴 B:热堆读 host-call 边界内联 —— 难度中,ROI 中
- **现状:** native 标量已接近 Rust(0.9–2.1×),但**堆读比 Rust 慢 ~13×**(`native_read_heap`),
  因为每次 `list_len`/`list_get` 都跨 host-helper 调用边界(§7.1)。
- **做法:** 把最热的只读 helper(`list_len`/`list_get_int/float`/`field_*`)的快路径**内联进生成代码**
  (直接读 `TypedVec` 的长度 / 缓冲指针,慢路径再 fall back 到 host call),削掉调用 + 寄存器保存开销。
- **收益:** 对已经 native 的 heap-heavy 代码直接提速;通用、风险中等(要小心 §7.2 的指针 pin 协议与边界检查)。
- **注意:** 这是**轴 B(代码质量)**,不增加覆盖率;和 J0.4 正交,可独立做。

### 4. 嵌套循环 OSR —— ✅**已支持**(原评:难度中–高,ROI 中)
- **核实(2026-06-28):** 这条的"缺口"是**误判**。`detect_single_natural_loop` **不在 OSR 管线里**——它只用于
  诊断报告(`mod.rs:2512`)和单测。OSR 管线全程用**多循环感知**的 `detect_natural_loops` →
  `detect_natural_loop_at` → `select_osr_candidate_loop` → `mapped_osr_loop`,**嵌套/兄弟循环本就支持**。
  - 外层全 native 时:外层 region 更长、score 更高,整块嵌套 `[外头, 外出)` 编成**一个** native loop,
    内层 backedge 只是 region 内的 `Jump`(vm-jit 块构造器支持任意可归约 CFG / 多 backedge)。
  - 外层体有非 subset op(I/O 等)时:外层落选,`select_osr_candidate_loop` 直接选**内层**循环;
    内层 OSR-entry/exit 在每个外层迭代**可重入**(`osr_cache` 命中、`osr_state` 不被一次性消费)。
- **回归测试(新增):** `native_osr_nested_inner_loop_matches_interpreter`(外层全 native)+
  `native_osr_nested_inner_loop_with_dirty_outer_matches_interpreter`(脏外层,内层可重入)——
  两者均 byte-parity 解释器且 `osr_entries > 0`。
- **`string_text` 的真正 gate:** 不是嵌套循环,而是**未落地的 native string-READ helper**
  (`string_len`/`string_byte`,已设计未 land,因为之前"没有消费者")。嵌套循环既已支持,这个消费者
  现在存在了——若要推进 `string_text`,下一步是 land 这两个只读 helper(§7.2-safe)。

### 5. J0.4 S1–S3:仅分配的 native 堆写 —— ✅**已完成**(难度中–高,ROI 最高,无 J0.1/J0.5 前置)★旗舰
- **完成状态(2026-06-28 核实):** 已落地并验证。10 个 `AllocatesResult` host helper
  (`vm-jit/src/lib.rs` 578–756:`ListNewInt` / `StringFromInt` / `StringConcat` / `StringSlice` /
  `StringPadLeft` / `StringSplit` / `StringLiteral` / `JsonParse` / `JsonField` / `BytesSlice`)在 S0 的
  `JIT_HEAP_RESULTS` 输出表之上 `publish_heap_result` 分配一个**全新无别名** `VmValue`,经
  `escaping_output_handle` 逃逸分析(`translate.rs` 802–896 + gate 1837/1880)确保结果真被返回/下游消费。
  **`mem_budget` 用 Model-A 精确兜底**(`tier.rs` 868–881:armed 时拒绝 native),所以无需在 helper 内记账、
  也无双重计费——比计划里"在边界处计 `mem_budget`"更简洁且同样精确。§7.2 由"bail 即清空输出表 + `JitHeapResultsGuard`"
  保证,force-deopt 测试坐实。测试:`native_string_from_int_return_allocates_heap_result` /
  `native_string_concat_handle_feeds_string_len` / `native_heap_result_force_deopt_leaves_output_table_empty`
  (reg_vm/tests.rs)+ vm-jit lib 83/0。
- **基准注脚:** 如计划所述,alloc-bound 基准多为**已 fold 的字符串**,所以**基准搬针有限**;更广的
  "集合构造器就地写"收益落在 **#8(S4)**。S1–S3 的价值是把"一分配就退回解释器"这块通用能力补齐。
- **解锁:** 所有 **alloc-bound** kernel(反复 `String.concat`、`String.from_int`、建新 Map/List、json 构造,
  共 ~138ms)——现在它们因"一分配就退回解释器"而全 parity。
- **做法(effect-after-commit,不是回滚):** 在 S0(已 ship 的堆结果返回 ABI)之上,让 native **分配一个
  全新、无别名的值类型**,在 host-helper 边界处**计 `mem_budget`**,并在**一个不会再 bail 的尾段提交**——
  这样任何 bail 都发生在分配之前,没有 double-apply,**因此既不需要 J0.1 也不需要 J0.5**。
- **代价:** 中等偏大(~450–650 行 + parity 测试)。**它打破 §7.2(从"无副作用"变"有受控副作用")**,
  所以要补:① 替换等价性(replacement-equivalence)论证;② 一条 `mem_budget` 的 differential。
  **S1 一落地,`mem_budget` 就必须加入 native eligibility 判定。**
- **建议:** 这是接下来**最该先做**的一项——单点解锁面最大,且不依赖最难的 deopt 重建。

### 6. J0.5:生成代码内 `VmLimits` 记账 —— 🔶 **step + cancel 已落地(OSR 层);mem 仍 future**
- **现状(2026-06-28):** **OSR 层的 armed 变体现在在生成代码里强制 `step_budget` 和 `cancel`**
  (Exec-Spec §6.2 的*enforce*分支)。每条指令 +1 tick(与解释器 `tick()` 1:1,因为 `resume_ip`
  就是共享的指令索引);在**每个循环 header**(= 每条 backedge,含嵌套循环)测 `steps > step_budget`
  并 poll `cancel`;在**每个 native 出口**(干净 `Return` + 共享 `fallback` deopt 边)把 `steps`
  写回宿主 cell;trip 时 bail(resume_ip = header)回解释器由它报错——一条跨 native/解释器的
  tick 流,不重不漏,`cancel` 只观测不回滚。ABI:新增 `limits_ptr` 参数指向宿主
  `[steps, step_budget, cancel_addr]` cell(`call_with_limits`);未 armed 变体忽略它(与改前字节一致,
  热路径零开销)。`try_osr`/`resolve_osr_candidate` 现在**仅在 `mem_budget` armed 时**拒绝 OSR。
- **mem 维基本落地(OSR 层,按 parity):** native 必须**与解释器逐字节对齐**地计入 `mem_budget`。解释器每次迭代
  唯一的 `account_bytes` 点是 flat-list 容量增长(`List.push`/`List.append`)和 list/map **字面量**构造
  (`MakeList`/`MakeMap`);string/json/map/set/deque/sortedmap 插入都计 **0**。其中只有 `List.push` 可被 native lower
  (`MakeList`/`MakeMap`/`ListAppend` 不可,故含它们的循环根本不进 native)。所以 native OSR 循环计入恰等于解释器,
  **除非**它含 `ListPush*` —— 其余分配(字符串构建、map/set 插入…)两边都计 0,故 `mem_budget` armed 时照跑 native(精确 parity,
  无需生成代码内记账)。`jit_fn_has_unaccounted_mem_charge` 只对 `ListPush*` 循环 decline(而 live-in list 增长本就被
  OSR growth-admissibility veto,故这是双保险)。测试:`native_osr_nonallocating_loop_runs_under_mem_budget`、
  `native_osr_map_insert_loop_runs_under_mem_budget`(均正,`osr_entries>0`);hostile mem 套件全绿(list-alloc runaway 仍在解释器 trip)。
- **仍缺:** ① native `ListPush` 增长的生成代码内字节记账(绑 S4;OSR 里罕见);② 把强制扩展到整函数/递归层(仍 Model-A 拒绝)。
- **测试:** `native_osr_completes_under_generous_step_budget`(正,`osr_entries>0`)、
  `native_osr_trips_tight_step_budget`、`native_osr_cancel_flag_preempts`;hostile limits 套件全绿。

### 7. 完整 J0.1:内联帧链 + 堆值/live-out 重建 —— 难度很高,ROI 高(地基),S4 前置
- **缺口三块:** ① 内联 leaf 区域内 deopt 的**逻辑帧链**状态图格式;② **堆 payload 的 variant/Result**
  在循环后仍活时的**值重建**(把 native 造的复合堆值跨 bail 重新物化);③ live-out 复合值重建(perf)。
- **为什么最硬:** 要在**任意 bail 点**把 native 那套被打散的状态(寄存器、解构的复合值、内联帧链)**完整
  翻译回**解释器状态,还要堆停在一致中间态。**这种 bug 输出测不出来**(differential 抓不到),必须为每条
  slice 写定向 repro。等价于 HotSpot 的 deopt + scope descriptors + 逃逸对象 rematerialization——业界最硬子系统之一。
- **定位:** 这是"大魔王"的真正核心,S4 与"边写边精确续跑"都建在它上面。

### 8. J0.4 S4:任意别名堆就地写 —— 难度很高,ROI 高/广,需 J0.1 + J0.5
- **解锁:** 真正的"**随便写**"——就地改"调用方也持有引用"的堆,且写后能精确续跑。这是通用 native 分配/写的终态。
- **为什么放最后:** 它是单体式的,**同时**要 J0.1(精确 deopt 重建)和 J0.5(生成代码内记账)。在它们就位前无法安全做。

### 9. async / 挂起函数 native —— 难度很高,类相关,可能不做
- **现状:** 挂起函数(`await`/`task_group`)天然 native-ineligible,整类异步代码走解释器。
- **做法:** native 里支持 park/resume 帧状态——架构性大改,§7.2/deopt 交互复杂。
- **定位:** 收益只覆盖异步密集代码;优先级最低,先评估是否值得。

---

## 一句话收尾

> ~~接下来最该做的是 #5~~ ✅**#5(J0.4 S1–S3,仅分配的 native 堆写)已完成**(2026-06-28 核实:
> 10 个 `AllocatesResult` helper 落地 + 测试绿;运行期 401 functional pass / vm-jit 83/0,唯一红是
> 容器内已知坏的 perf-gate 基准)。**接下来该做 #3(轴 B:热堆读 host-call 边界内联)**——独立、广收益、
> 与已 native 的 heap-heavy 代码复利。再往上走 **#4(嵌套循环 OSR)→ #7 J0.1 → #6 J0.5 → #8 S4** 这条
> 精确-deopt 主线(JIT 最复杂的一块)。**#1 / #2** 按需,**#9** 缓做。
