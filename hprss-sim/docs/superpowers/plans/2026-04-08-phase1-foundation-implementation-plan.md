# HPRSS-SIM Phase 1 基础设施实现计划（可执行版）

**日期**: 2026-04-08  
**输入规格**: `docs/superpowers/specs/2026-04-08-hprss-sim-full-implementation-design.md` (v3)  
**目标**: 完成 DAG + 异构执行时间 + 边传输语义 + 多核建模 的基础能力，保证后续 EDF/HEFT 可落地。

---

## 1. 范围与边界

### 1.1 In Scope（本计划覆盖）
- 修复现存阻塞 bug：`sample_exec_time()` CPU 偏置采样
- Job 增加 DAG provenance 信息
- DAG 边级别传输类型与事件定义
- `PreemptionPolicy` / `ExecutionModel` 轻量策略抽象
- 多核 CPU 按“每核一个设备”建模（含 `device_group`）
- DAG 工作负载生成（Erdos-Renyi + 层次化）
- `DagTracker` + `SimEngine` DAG 集成（含 fan-in/fan-out 边 token）
- Phase 1 集成测试

### 1.2 Out of Scope（本计划不做）
- EDF/HEFT 调度器实现（Phase 2）
- LLF（当前架构阻塞）
- Level 1-5 验证框架实现（Phase 4）
- SHAPE/HEFT 复现实验（Phase 5）

---

## 2. 执行顺序（按依赖）

## Step A — 先修阻塞 bug（必须第一个完成）

### A1. `p1-job-exec-time`（优先级 P0）
**目标**: 执行时间在 dispatch 时按目标设备解析，而不是 release 时。

**改动文件**
- `crates/hprss-types/src/job.rs`
- `crates/hprss-engine/src/engine.rs`
- `crates/hprss-engine/tests/closed_loop.rs`

**实施要点**
1. `Job.actual_exec_ns: Nanos -> Option<Nanos>`
2. `Job::new()` 接口更新（允许未解析执行时间）
3. `schedule_job_release()` 不再调用 `sample_exec_time()`
4. `dispatch_job()` 中依据 `device_id -> DeviceType` 解析执行时间并写回 `job.actual_exec_ns`
5. `remaining_ns()` 语义改为对 `None` 安全（未解析时返回 0 或通过调用路径保证已解析）
6. 移除或重写 `sample_exec_time()`，避免 CPU 优先 fallback

**完成判据**
- GPU/DSP/FPGA 执行时间与 `Task.exec_times` 对应设备一致
- 现有单元与集成测试通过

---

## Step B — 类型层打底（DAG/策略/多核）

### B1. `p1-dag-provenance`
**改动文件**
- `crates/hprss-types/src/dag.rs`（新增）
- `crates/hprss-types/src/job.rs`
- `crates/hprss-types/src/lib.rs`

**实施要点**
1. 新增 `DagInstanceId`, `SubTaskIdx`, `DagProvenance`
2. `Job` 增加 `dag_provenance: Option<DagProvenance>`
3. 对 serde 序列化保持兼容

### B2. `p1-edge-transfer-types`
**改动文件**
- `crates/hprss-types/src/dag.rs`（新增 `EdgeTransferId`）
- `crates/hprss-types/src/event.rs`
- `crates/hprss-types/src/lib.rs`

**实施要点**
1. 新增 `EdgeTransferId { dag_instance_id, from_node, to_node }`
2. `EventKind` 增加 `EdgeTransferComplete`
3. 保留原 `TransferComplete` 以兼容非 DAG 工作负载

### B3. `p1-preemption-policy` + `p1-execution-model`
**改动文件**
- `crates/hprss-types/src/policy.rs`（新增）
- `crates/hprss-types/src/task.rs`
- `crates/hprss-types/src/device.rs`
- `crates/hprss-types/src/lib.rs`

**实施要点**
1. 定义 `PreemptionPolicy` trait（仅策略，不持有运行态）
2. 定义 `ExecutionModel` trait（按设备解析执行时间）
3. 先用适配方式落地：`PreemptionModel` 与 `ExecutionTimeModel` 提供 trait 实现，避免一次性大改

### B4. `p1-multicore-config`
**改动文件**
- `crates/hprss-types/src/device.rs`
- `crates/hprss-platform/src/config.rs`
- `configs/platform_ft2000_full.toml`

**实施要点**
1. `DeviceConfig` 新增 `device_group: Option<String>`
2. 配置层支持显式 per-core 设备定义（`FT2000-core0..3`）
3. 保持向后兼容：已有单设备配置仍可加载

---

## Step C — 传输层与 DAG 运行时集成

### C1. `p1-dag-workload`
**改动文件**
- `crates/hprss-workload/src/dag_generator.rs`（新增）
- `crates/hprss-workload/src/lib.rs`

**实施要点**
1. Erdos-Renyi DAG 生成（保证无环）
2. 层次化 DAG 生成（更接近 OpenMP task 图）
3. 基础导入导出结构（为后续 ompTG JSON 适配留口）

### C2. `p1-dag-tracker`
**改动文件**
- `crates/hprss-engine/src/dag_tracker.rs`（新增）
- `crates/hprss-engine/src/lib.rs`

**实施要点**
1. DAG 实例状态：节点映射、边满足状态、未满足入边计数
2. SubTask -> Task 代理机制（给 Scheduler callback 使用）
3. fan-in 场景保证“所有入边完成才释放后继”

### C3. `p1-engine-dag-integration`
**改动文件**
- `crates/hprss-engine/src/engine.rs`
- `crates/hprss-engine/src/transfer_manager.rs`
- `crates/hprss-engine/src/device_manager.rs`（必要时）

**实施要点**
1. `TransferManager` 增加 edge-token API：
   - `initiate_edge_transfer(...)`
   - `on_edge_transfer_complete(...)`
2. 引擎事件循环处理 `EdgeTransferComplete`
3. DAG 节点完成 -> 边传输 -> 后继节点释放完整闭环
4. 保持非 DAG 任务路径行为不变

---

## Step D — 测试与收敛

### D1. `p1-dag-tests`
**改动文件**
- `crates/hprss-engine/tests/closed_loop.rs`（扩展）
- `crates/hprss-engine/tests/dag_flow.rs`（新增）
- `crates/hprss-workload/src/dag_generator.rs`（测试）

**最小测试集**
1. **异构执行时间修复测试**: 同一 Task 分派到 CPU/GPU 时完成时间不同且正确
2. **边 token 正确性**: 两前驱一后继（fan-in）不会提前释放后继
3. **跨设备传输链路**: 传输完成事件触发后继释放
4. **每核独立设备**: CPU 4 核可并行运行 4 个 job
5. **回归**: 现有 FP-Het 测试保持通过

---

## 3. 与 SQL Todo 的映射

| 执行步骤 | SQL Todo IDs |
|---|---|
| Step A | `p1-job-exec-time` |
| Step B | `p1-dag-provenance`, `p1-edge-transfer-types`, `p1-preemption-policy`, `p1-execution-model`, `p1-multicore-config` |
| Step C | `p1-dag-workload`, `p1-dag-tracker`, `p1-engine-dag-integration` |
| Step D | `p1-dag-tests` |

---

## 4. 实施策略（避免大爆炸改动）

1. **先修 bug 再扩展 DAG**：先让异构执行时间正确，避免后续所有测试基线失真。  
2. **先类型后行为**：先把类型和事件定义补齐，再接入引擎逻辑。  
3. **双路径兼容**：DAG 新路径上线期间，保证原有非 DAG 路径继续通过测试。  
4. **小步提交**：每个 Step 最少一个独立 commit，便于回滚与审阅。  

---

## 5. 进入实现前检查清单

- [ ] 规格 v3 已确认为当前唯一设计基线  
- [ ] SQL ready todos 已核对（7 个）  
- [ ] 从 `p1-job-exec-time` 开始，按依赖推进  
- [ ] 每完成一个 todo 即更新状态并提交变更  

