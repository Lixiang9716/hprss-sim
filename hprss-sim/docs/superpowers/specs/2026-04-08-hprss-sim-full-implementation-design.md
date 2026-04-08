# HPRSS-SIM 全程实现设计规格

**日期**: 2026-04-08  
**状态**: 已批准 (v2 — rubber-duck 审查后修订)  
**目标**: 论文产出优先 (WATERS/RTSS-BP)

---

## 0. 实现落地更新（Phase2 最终核对）

> 本文最初是设计规格；以下条目用于覆盖后续实现状态，优先于下文的“未实现/未来工作”历史描述。

- 调度器能力已扩展为：`fp, edf, edfvd, llf, heft, cpedf, federated`
- CLI 已支持 replay 模式：`--replay-json` 与 `--replay-csv-tasks/--replay-csv-jobs`
- `hprss-devices` 已提供虚拟设备抢占模型测试（fully/limited/interrupt/non-preemptive）
- Sweep 输出已包含扩展论文指标与复现实验元数据（算法标签、运行指纹/版本信息）
- 绘图工作流固定为 `python3 scripts/plot_experiments.py ...` 与 `make plot-test` 回归检查

推荐命令（与当前实现一致）：

```bash
# 多调度器扫参
cargo run --release -p hprss-sim -- --platform configs/platform_ft2000_full.toml \
  sweep --schedulers fp,edf,edfvd,llf,heft,cpedf,federated \
  --utilizations 0.5:0.1:0.9 --task-counts 10,50,100 --seeds 1:5 --output sweep_results.csv

# replay 重放（CSV）
cargo run -p hprss-sim -- --platform configs/platform_ft2000_full.toml \
  --replay-csv-tasks crates/hprss-workload/tests/fixtures/replay_tasks.csv \
  --replay-csv-jobs crates/hprss-workload/tests/fixtures/replay_jobs.csv \
  --scheduler llf

# 绘图
python3 scripts/plot_experiments.py --csv sweep_results.csv --output-dir plots
```

---

## 1. 项目现状

### 1.1 已完成（截至 Phase2）
- DES 引擎闭环（事件驱动 + 4 种抢占模型 + 版本失效）
- 调度器集合：FP / EDF / EDF-VD / LLF / HEFT / CP-EDF / Federated
- 平台 TOML 加载 + 总线仲裁 + 数据传输 + DAG 流程
- 工作负载：UUniFast 生成 + Replay(JSON/CSV) + CLI 单次运行/并行扫参
- 可视化工作流：`scripts/plot_experiments.py` + `make plot-test`

### 1.2 仍在迭代
- `hprss-validate` 仍以增量补齐为主（非空桩，但持续扩展中）
- 高级并行仿真（PDES）与更深层理论验证仍属后续阶段

### 1.3 现存 Bug (Phase 1 必须修复)

**⚠️ BLOCKING: `sample_exec_time()` 始终使用 CPU WCET**

当前 `engine.rs:893-935` 的 `sample_exec_time()` 在 Job 释放时采样执行时间，始终选择 CPU 的 WCET。当 Job 后续被调度到 GPU/DSP/FPGA 时，仍使用 CPU 时间计算 JobComplete 事件。

**影响**: 所有异构调度实验结果不正确。

**修复**: Phase 1 的 `p1-job-exec-time` 任务将修复此问题 — Job 释放时 `actual_exec_ns = None`，dispatch 时根据目标设备解析。

---

## 2. 设计决策

### 2.1 实现策略
**方案 C: 混合策略** — 引擎先支持 DAG，调度器分批上线

### 2.2 关键参数
| 参数 | 选择 |
|------|------|
| 调度器集合 | FP + EDF + HEFT (3个，LLF 推迟) |
| 验证深度 | Level 1-5 (含 SHAPE/HARD 复现) |
| RL 集成时机 | 论文 #1 投稿后 |
| 可视化 | CSV + 甘特图 + Python |
| 设备抽象 | PreemptionPolicy + ExecutionModel (轻量策略) |
| 多核 CPU | 每核作为独立设备 |

### 2.3 Rubber-duck 审查修订 (v2)

| 原设计 | 问题 | 修订 |
|--------|------|------|
| DAG 节点完成触发后继 | 跨设备传输未完成就释放后继 | **边级别 token**：传输完成才满足边 |
| DeviceSimulator 完整状态机 | 与引擎 DES 所有权冲突 | **轻量策略对象** PreemptionPolicy |
| Job 释放时确定执行时间 | 设备未分配时 WCET 不确定 | **设备分配时解析** actual_exec_ns |
| 每设备一个 running job | 多核 CPU 无法建模 | **每核独立设备** |
| HEFT 作为主要调度器 | 周期任务干扰下不是真正 HEFT | **HEFT 限定单 DAG baseline** |
| LLF 在 Phase 2 | 回调模型不支持 laxity 连续变化 | **推迟到 Phase 3+** |

---

## 3. DAG 引擎扩展

### 3.1 边级别依赖追踪 (关键修订)

**原问题**: 节点完成即释放后继，忽略跨设备传输延迟。

**修订设计**: 使用 **边 token** 模型：

```rust
pub struct DagTracker {
    instances: HashMap<DagInstanceId, DagInstance>,
}

pub struct DagInstance {
    dag_task_id: TaskId,
    release_time: Nanos,
    absolute_deadline: Nanos,
    node_jobs: HashMap<SubTaskIdx, JobId>,
    /// 边级别 token: (from_node, to_node) → 是否已完成（包括传输）
    edge_satisfied: HashMap<(SubTaskIdx, SubTaskIdx), bool>,
    /// 每条边的传输状态
    edge_transfers: HashMap<(SubTaskIdx, SubTaskIdx), EdgeTransferState>,
}

pub enum EdgeTransferState {
    NotStarted,
    Transferring { job_id: JobId },  // TransferManager 跟踪
    Completed,
}
```

### 3.2 工作流 (修订)

1. **DAG 到达** → 创建 `DagInstance` → 初始化所有边为 `NotStarted`
2. **释放源节点** → 无入边的节点立即创建 Job
3. **节点 Job 完成** → 对每条出边：
   - 同设备后继：边直接标记 `Completed`
   - 跨设备后继：发起边传输，状态变为 `Transferring`
4. **边传输完成** (`EdgeTransferComplete` 事件) → 边标记 `Completed`
5. **检查后继就绪** → 所有入边 `Completed` 时创建后继 Job
6. **所有节点完成** → DAG 实例完成，检查端到端 deadline

### 3.3 新增事件类型

```rust
pub enum EventKind {
    // ... 现有事件 ...
    
    /// DAG 边传输完成
    EdgeTransferComplete {
        dag_instance_id: DagInstanceId,
        from_node: SubTaskIdx,
        to_node: SubTaskIdx,
        expected_version: u64,
    },
}
```

### 3.4 Job 与 DAG 关联 (新增字段)

```rust
pub struct Job {
    // ... 现有字段 ...
    
    /// DAG 来源（独立任务为 None）
    pub dag_provenance: Option<DagProvenance>,
}

pub struct DagProvenance {
    pub dag_instance_id: DagInstanceId,
    pub node_idx: SubTaskIdx,
}
```

### 3.5 设计原则

- **Scheduler trait 基本不改** — DAG 依赖解析在引擎层完成，调度器只看普通 Job
- **可选 DAG 元数据** — `SchedulerView` 可提供 `dag_info(job_id)` 查询 DAG 上下文
- **边传输自动插入** — 跨设备边由引擎调用 TransferManager
- **ompTG 兼容**: 支持 `target_hint`、per-edge `data_size`、barrier 节点

### 3.6 SubTask → Task 代理 (架构补充)

**问题**: `Scheduler::on_job_arrival(&Job, &Task, &SchedulerView)` 需要 `&Task`，但 DAG 节点是 `SubTask`。

**解决方案**: DagTracker 在 DAG 注册时为每个 SubTask 合成代理 Task：

```rust
impl DagTracker {
    /// 注册 DAG 任务，为每个 SubTask 创建代理 Task
    pub fn register_dag(&mut self, dag: DagTask, task_registry: &mut Vec<Task>) -> TaskId {
        let base_id = task_registry.len() as u32;
        
        for (idx, subtask) in dag.nodes.iter().enumerate() {
            let proxy_task = Task {
                id: TaskId(base_id + idx as u32),
                name: format!("{}_node{}", dag.name, idx),
                arrival: ArrivalModel::Aperiodic,  // DAG 节点由 DagTracker 触发
                period: 0,
                deadline: dag.deadline,  // 继承 DAG 整体 deadline
                priority: dag.priority,
                criticality: dag.criticality,
                exec_times: subtask.exec_times.clone(),
                affinity: subtask.affinity.clone(),
                data_size: 0,  // 边传输由 DagTracker 管理
                chain_id: None,
            };
            task_registry.push(proxy_task);
        }
        
        TaskId(base_id)  // 返回第一个节点的 TaskId
    }
}
```

### 3.7 TransferManager 边 Token 扩展

**问题**: 当前 `ActiveTransfer { job_id }` 无法区分同一后继的多条入边。

**解决方案**: 扩展传输标识为边级别：

```rust
/// 边传输标识
pub struct EdgeTransferId {
    pub dag_instance_id: DagInstanceId,
    pub from_node: SubTaskIdx,
    pub to_node: SubTaskIdx,
}

/// 扩展 TransferManager API
impl TransferManager {
    /// DAG 边传输（区分同一后继的多条入边）
    pub fn initiate_edge_transfer(
        &mut self,
        edge_id: EdgeTransferId,
        source_device: DeviceId,
        target_device: DeviceId,
        data_size: u64,
        priority: u32,
        now: Nanos,
    ) -> Vec<ScheduledEvent>;
}
```

DagTracker 维护每个后继节点的入边计数器，仅当所有入边传输完成时才释放后继 Job。

### 3.8 DAG 工作负载生成

| 类型 | 参数 |
|------|------|
| Erdős-Rényi | (n_nodes, edge_prob, ccr) |
| 层次化 | (layers, width_range, ccr) |
| JSON 导入 | ompTG 格式兼容 |

---

## 4. 设备抽象 (轻量策略模式)

### 4.1 设计修订

**原问题**: `DeviceSimulator` 完整状态机与引擎 DES 所有权冲突。

**修订**: 使用轻量策略对象，引擎保持 DES 事件调度的唯一所有权。

### 4.2 PreemptionPolicy Trait

```rust
/// 抢占策略 — 不管理状态，只提供决策
pub trait PreemptionPolicy: Send + Sync {
    /// 是否允许在当前时刻抢占
    fn can_preempt_now(&self, job_progress: Nanos, now: Nanos) -> bool;
    
    /// 下一个抢占窗口时刻（用于调度 PreemptionPoint 事件）
    fn next_preemption_window(&self, job_start: Nanos, now: Nanos) -> Option<Nanos>;
    
    /// 抢占开销
    fn preemption_overhead(&self) -> Nanos;
}
```

### 4.3 四种抢占策略实现

| 策略 | 行为 |
|------|------|
| `FullyPreemptive` | `can_preempt_now` 始终 true |
| `LimitedPreemptive { granularity }` | 每 granularity ns 返回 true |
| `InterruptLevel { isr, dma_block }` | ISR 可抢占，DMA 区间不可 |
| `NonPreemptive { reconfig_time }` | 始终 false，完成后才能切换 |

### 4.4 ExecutionModel Trait

```rust
/// 执行模型 — 决定 job 在特定设备上的执行时间
pub trait ExecutionModel: Send + Sync {
    /// 给定任务的执行时间（设备分配时调用）
    fn resolve_exec_time(
        &self,
        task: &Task,
        subtask_idx: Option<SubTaskIdx>,
        rng: &mut impl Rng,
    ) -> Nanos;
    
    /// 速度因子（用于 work_to_wall 转换）
    fn speed_factor(&self) -> f64;
}
```

### 4.5 多核 CPU 建模

**决策**: 每核作为独立设备。

```toml
# platform.toml 示例
[[device]]
name = "FT2000-core0"
device_type = "Cpu"
cores = 1
preemption = { type = "FullyPreemptive" }

[[device]]
name = "FT2000-core1"
device_type = "Cpu"
cores = 1
preemption = { type = "FullyPreemptive" }

# ... core2, core3
```

调度器通过 `DeviceConfig.device_group` 字段识别同属一个物理 CPU 的核。

### 4.6 架构关系 (修订)

```
SimEngine (DES 事件所有权)
    │
    ├── DeviceManager
    │       ├── devices: Vec<DeviceState>
    │       ├── preemption_policies: Vec<Box<dyn PreemptionPolicy>>
    │       └── execution_models: Vec<Box<dyn ExecutionModel>>
    │
    └── Job 执行时间在 dispatch 时解析
            engine.resolve_job_exec_time(job_id, device_id)
```

### 4.7 Job 执行时间设备相关 (关键修订)

**原问题**: Job 释放时 `actual_exec_ns` 已确定，但设备未分配。

**修订**: 
- Job 释放时 `actual_exec_ns = None`
- dispatch 到设备时调用 `ExecutionModel.resolve_exec_time()`
- 记录到 `job.actual_exec_ns`

```rust
// engine.rs dispatch_job() 中
let exec_model = self.device_mgr.execution_model(device_id);
let actual_exec_ns = exec_model.resolve_exec_time(task, dag_node, &mut self.rng);
job.actual_exec_ns = Some(actual_exec_ns);
```

### 4.8 扩展接口

预留 gem5/存算一体模拟器接入点 — 实现 `ExecutionModel` trait，通过 IPC 获取执行时间。

---

## 5. 调度器集合

### 5.1 FP-Het (已实现 ✅)
- 固定优先级，Rate-Monotonic
- ~148 LOC

### 5.2 EDF-Het (新增)
- 动态优先级，deadline 最近优先
- `effective_priority` = `u32::MAX - (deadline / 1000)` (避免溢出)
- ~150 LOC

### 5.3 LLF-Het (架构阻塞 — 不在本规格范围)

**⚠️ 永久性架构限制**: LLF 需要计算 `laxity = deadline - now - remaining_exec`。但 `SchedulerView` 设计上隐藏了 `remaining_ns`（为了匹配真实硬件可观测性），调度器无法获取 Job 的剩余执行时间。

**选项**:
1. 暴露 WCET (悲观 `remaining ≤ wcet - elapsed`) — 可接受于分析工具
2. 接受 LLF 是悲观近似，明确记录

**决定**: LLF 不在本规格范围内。如需实现，需要先决定上述选项并修改 SchedulerView API。

### 5.4 HEFT (新增 — 单 DAG baseline)

**定位修订**: HEFT 是 **offline makespan 优化基线**，不是主要实时调度贡献。

**适用场景**: 单次 DAG 执行的 makespan 实验，不涉及周期任务干扰。

```rust
pub struct HeftPlanner {
    /// DAG 模板 → (节点 → rank_u, 节点 → 目标设备)
    plans: HashMap<TaskId, HeftPlan>,
}

pub struct HeftPlan {
    rank_u: Vec<f64>,               // 按 SubTaskIdx 索引
    device_mapping: Vec<DeviceId>,  // 按 SubTaskIdx 索引
}

pub struct HeftScheduler {
    planner: HeftPlanner,
}
```

**局限性**:
- 周期 DAG 流场景下退化为静态映射
- 不处理运行时资源竞争
- 不提供实时性保证

### 5.5 后续扩展 (Phase 4+)
- CP-EDF (DAG 关键路径 EDF)
- Federated Scheduling (高利用率 DAG)
- SHAPE 算法 (自挂起模型 RTA)
- EDF-VD (混合关键性)
- LLF-Het (需要 Scheduler trait 扩展)

---

## 6. 任务模型支持

### 6.1 已支持
| 模型 | 状态 |
|------|------|
| 周期任务 | ✅ 完整 |
| 偶发任务 | ✅ 完整 |
| 非周期任务 | ✅ 基础 |
| 自挂起 | ✅ `Transferring` 状态 |

### 6.2 本次新增
| 模型 | 实现方式 |
|------|---------|
| DAG 任务 | DagTracker |
| 任务链 | `TaskChain` 端到端分析 |

### 6.3 预留扩展
- 条件 DAG (if/else 路径)
- 多帧任务 (ExecutionTimeModel 扩展)
- 服务器 (CBS/SS/PS)

---

## 7. 验证框架

### 7.1 架构

```rust
pub trait ValidationTest {
    fn name(&self) -> &str;
    fn level(&self) -> u8;
    fn run(&self, engine_factory: &dyn Fn() -> SimEngine) -> TestResult;
}

pub struct TestResult {
    passed: bool,
    metric: String,
    expected: f64,
    actual: f64,
    tolerance: f64,
}
```

### 7.2 Level 1 — 经典理论精确匹配 (6 tests)

**适用子集**: 单处理器、全抢占、CPU-only

| 测试 | 验证内容 |
|------|---------|
| `liu_layland_rm_bound` | U ≤ n(2^(1/n)-1) 时 FP 无 miss |
| `edf_exact_bound` | U ≤ 1.0 时 EDF 无 miss |
| `joseph_pandya_rta` | R_sim ≤ R_rta |
| `audsley_opa` | 最优优先级分配 |
| `non_preemptive_rta` | George-Rivière RTA |
| `dm_optimality` | DM = RM for implicit deadline |

### 7.3 Level 2 — 小规模穷举 (3 tests)

**适用子集**: 2-3 任务，可枚举所有调度序列

- 2 任务 + 1 CPU 穷举
- 3 任务 + 2 设备手工验证
- **新增**: 小规模异构 DAG 精确参考解释器

### 7.4 Level 3 — 跨模拟器对比 (修订范围)

**限定 CPU-only 子集** — SimSo 不支持异构传输/抢占语义

- SimSo JSON 导入导出
- 同配置 deadline miss 对比
- **注意**: 仅验证 FP/EDF 在同构 CPU 上的行为一致性

### 7.5 Level 4 — 异构特有 (5 tests)
- GPU kernel 边界抢占验证
- DSP DMA 阻塞项验证
- FPGA PR 重配开销验证
- 跨设备传输延迟精确性
- 混合抢占 RTA B_blocking

### 7.6 Level 5 — SOTA 复现 (3 tests)
- SHAPE 可调度率曲线 (趋势 ±5%)
- HEFT makespan 经典结果 (单 DAG)
- 自挂起 RTA 对比

### 7.7 RTA 求解器模块
```rust
pub fn rta_fp_uniprocessor(tasks: &[Task]) -> Vec<Nanos>;
pub fn rta_fp_heterogeneous(tasks: &[Task], devices: &[DeviceConfig]) -> Vec<Nanos>;
pub fn schedulability_test_edf(tasks: &[Task]) -> bool;
```

---

## 8. 可视化与输出

### 8.1 输出层级

| 层级 | 格式 | 用途 |
|------|------|------|
| Trace | JSON-lines | 调试 + 甘特图数据 |
| Summary | CSV | 批量实验统计 |
| Gantt | JSON | 可视化数据 |

### 8.2 论文指标集 (新增)

| 指标 | 说明 |
|------|------|
| `schedulable` | 是否可调度 (无 deadline miss) |
| `miss_ratio` | deadline miss 率 |
| `makespan` | DAG 完成时间 |
| `avg_response_time` | 平均响应时间 |
| `per_device_utilization` | 各设备利用率 |
| `transfer_overhead` | 传输时间占比 |
| `blocking_breakdown` | 阻塞时间分解 (抢占/传输/等待) |
| `end_to_end_latency` | 任务链端到端延迟 |
| `scheduler_overhead` | 调度决策耗时 (实际运行时) |

### 8.3 可复现性元数据

每次实验输出包含:
- `seed`: 随机种子
- `config_hash`: 配置文件 hash
- `git_commit`: 代码版本
- `timestamp`: 运行时间

### 8.4 Trace 格式

```json
{"t": 0, "event": "arrival", "task": 3, "job": 0, "device": "CPU-0"}
{"t": 50000, "event": "preempt", "victim_job": 0, "by_job": 1, "device": "GPU"}
{"t": 100000, "event": "complete", "job": 1, "response_time": 100000}
```

### 8.3 Python 脚本 (`scripts/`)

| 脚本 | 功能 |
|------|------|
| `plot_schedulability.py` | 利用率 vs 可调度率曲线 |
| `plot_gantt.py` | 甘特图 (matplotlib) |
| `plot_response_time.py` | 响应时间 CDF/箱线图 |
| `plot_comparison.py` | 多调度器对比条形图 |

### 8.4 任务链分析输出

```
Chain: sensor→dsp→gpu→cpu
  reaction_time_max: 12.3ms (deadline: 20ms) ✅
  data_age_max: 15.1ms (deadline: 20ms) ✅
```

---

## 9. 实现阶段 (修订)

### Phase 1: 类型与边界重定义 [基础设施]

**目标**: 解决 rubber-duck 指出的所有权边界问题

- Job 添加 `dag_provenance: Option<DagProvenance>` 字段
- Job 执行时间改为设备分配时解析 (`actual_exec_ns: Option<Nanos>`)
- 多核 CPU 建模为独立设备 (平台配置修改)
- DAG 边传输模型 (`EdgeTransferId` 替代 job-level 传输)
- PreemptionPolicy / ExecutionModel trait (轻量策略)
- DagTracker 边级别 token 追踪
- DAG 工作负载生成 (Erdős-Rényi + 层次化)
- 集成测试: DAG 在 4 设备执行 + 边传输验证

### Phase 2: EDF-Het + HEFT [核心功能]

- EDF-Het 调度器实现 + 单元测试
- HEFT Planner (离线 rank_u + 设备映射)
- HEFT Scheduler (单 DAG makespan baseline)
- CLI `--scheduler fp/edf/heft` 切换
- 初步对比实验 (独立任务 + 单 DAG)

### Phase 3: Trace + 可视化 + 任务链 [完善]

- JSON-lines trace 输出
- 论文指标集实现 (makespan, utilization, blocking breakdown)
- 任务链端到端分析
- Python 绘图脚本 (4 个)
- 可复现性元数据输出

### Phase 4: 验证框架 [质量保证]

**分层架构**:
- `hprss-validate/analytic/` — 纯函数 RTA 求解器
- `hprss-validate/scenario/` — 引擎场景测试
- `hprss-validate/differential/` — 跨模拟器对比 (CPU-only)
- `hprss-validate/benchmark/` — SOTA 复现

- Level 1: 经典理论 (单核 CPU-only 子集)
- Level 2: 小规模穷举 + 异构精确参考
- Level 3: SimSo 对比 (限定 CPU-only)
- Level 4: 异构特有验证

### Phase 5: Level 5 复现 + 论文实验 [论文产出]

- SHAPE 算法复现 + 曲线对比
- HEFT makespan 经典结果复现
- 论文 #1 全部实验数据
- 论文图表生成

### Phase 6+ (可选): LLF + 高级调度 [扩展]

- Scheduler trait 扩展 (ReevaluateEvent)
- LLF-Het 实现
- CP-EDF / Federated Scheduling
- EDF-VD (混合关键性)

---

## 10. 文件结构变更 (修订)

```
hprss-sim/
├── crates/
│   ├── hprss-types/src/
│   │   ├── dag.rs              # 新增: DagProvenance, EdgeTransferId, DagInstanceId
│   │   └── policy.rs           # 新增: PreemptionPolicy, ExecutionModel traits
│   ├── hprss-engine/src/
│   │   ├── dag_tracker.rs      # 新增: 边级别 token 追踪
│   │   └── trace_writer.rs     # 新增
│   ├── hprss-devices/src/
│   │   ├── lib.rs              # PreemptionPolicy 实现
│   │   ├── fully_preemptive.rs
│   │   ├── limited_preemptive.rs
│   │   ├── interrupt_level.rs
│   │   └── non_preemptive.rs
│   ├── hprss-scheduler/src/
│   │   ├── edf.rs              # 新增
│   │   └── heft.rs             # 新增 (planner + scheduler)
│   ├── hprss-validate/src/
│   │   ├── lib.rs
│   │   ├── analytic/           # RTA 求解器
│   │   ├── scenario/           # 引擎场景测试
│   │   ├── differential/       # 跨模拟器对比
│   │   └── benchmark/          # SOTA 复现
│   └── hprss-workload/src/
│       └── dag_generator.rs    # 新增
└── scripts/
    ├── plot_schedulability.py
    ├── plot_gantt.py
    ├── plot_response_time.py
    └── plot_comparison.py
```

---

## 11. 风险与缓解 (修订)

| 风险 | 缓解措施 |
|------|---------|
| **边界重定义复杂度** | Phase 1 先做类型扩展，确保编译通过后再改引擎逻辑 |
| **DAG 边传输语义** | 先实现链式 DAG (每节点单前驱)，验证正确后扩展 fan-in/fan-out |
| **设备策略与引擎耦合** | 保持引擎为 DES 唯一所有者，策略对象只返回计算结果 |
| **HEFT 运行时失效** | 明确定位为单 DAG baseline，不用于周期任务场景 |
| **LLF 实现困难** | 推迟到 Phase 6，先评估是否需要 Scheduler trait 扩展 |
| **多核建模** | 每核独立设备，通过 device_group 字段关联 |
| **Level 3 SimSo 兼容性** | 限定 CPU-only 子集，异构场景用 Level 2 精确参考 |

---

## 12. 成功标准 (修订)

- [ ] Job 支持 DAG provenance + 设备相关执行时间
- [ ] DAG 边传输正确实现 (fan-in 场景无提前释放)
- [ ] 3 调度器 (FP/EDF/HEFT) 全部通过单元测试
- [ ] 多核 CPU (4 核) 正确建模为独立设备
- [ ] Level 1-4 验证在各自适用子集内全部通过
- [ ] 生成可用于论文的可调度率曲线图
- [ ] SHAPE 趋势复现误差 ≤ 5%
- [ ] 论文指标集完整输出 (makespan, utilization, blocking)

---

## 13. Rubber-duck 审查总结

**审查次数**: 5 轮  
**关键发现**: 6 个阻塞问题，均已在 v2 修订中解决

| # | 问题 | 解决方案 |
|---|------|---------|
| 1 | DAG 节点就绪条件错误 | 边级别 token，传输完成才满足 |
| 2 | Job 执行时间设备无关 | 设备分配时解析 actual_exec_ns |
| 3 | DeviceSimulator 与引擎所有权冲突 | 改用轻量策略对象 PreemptionPolicy |
| 4 | 多核 CPU 无法建模 | 每核独立设备 |
| 5 | HEFT 不适合周期任务场景 | 定位为单 DAG baseline |
| 6 | LLF 回调模型不支持 | 推迟到 Phase 6 |
| 7 | 验证框架过于单一 | 分层: analytic/scenario/differential/benchmark |
