# HPRSS-SIM 全程实现设计规格

**日期**: 2026-04-08  
**状态**: 已批准  
**目标**: 论文产出优先 (WATERS/RTSS-BP)

---

## 1. 项目现状

### 1.1 已完成 (4,210 LOC, 35 tests)
- DES 引擎闭环 (事件驱动 + 4 种抢占模型 + 版本失效)
- FP-Het 调度器 (到达/完成/抢占点/MC 切换)
- 平台 TOML 加载 + 总线仲裁 + 数据传输
- UUniFast 工作负载生成 + CLI (单次运行 + 并行扫参)
- BTreeMap 优化 (13.2x 加速)

### 1.2 未实现
- hprss-validate: 空桩
- hprss-devices: 空桩
- 仅 1 个调度器 (FP)，无 EDF/LLF/HEFT
- 无 DAG 任务支持
- 无可视化输出

---

## 2. 设计决策

### 2.1 实现策略
**方案 C: 混合策略** — 引擎先支持 DAG，调度器分批上线

### 2.2 关键参数
| 参数 | 选择 |
|------|------|
| 调度器集合 | FP + EDF + LLF + HEFT (4个) |
| 验证深度 | Level 1-5 (含 SHAPE/HARD 复现) |
| RL 集成时机 | 论文 #1 投稿后 |
| 可视化 | CSV + 甘特图 + Python |
| 设备模拟器 | 抽离为独立状态机 |

---

## 3. DAG 引擎扩展

### 3.1 DagTracker (新增 `dag_tracker.rs`)

```rust
pub struct DagTracker {
    dag_instances: HashMap<DagInstanceId, DagInstance>,
}

pub struct DagInstance {
    dag_task_id: TaskId,
    release_time: Nanos,
    absolute_deadline: Nanos,
    node_jobs: Vec<(SubTaskIdx, JobId)>,
    completed_nodes: BitSet,
    ready_nodes: Vec<SubTaskIdx>,
}
```

### 3.2 工作流

1. DAG 到达 → 创建 `DagInstance` → 释放所有源节点的 Job
2. 子任务 Job 完成 → 检查后继节点依赖
3. 依赖满足 → 创建后继 Job → `TaskArrival` 事件
4. 所有节点完成 → DAG 实例完成，检查端到端 deadline

### 3.3 设计原则

- **Scheduler trait 不改动** — DAG 依赖解析在引擎层完成
- **数据传输自动插入** — 跨设备前驱→后继自动调用 TransferManager
- **ompTG 兼容**: 支持 `target_hint`、`DataDep`、barrier 节点

### 3.4 DAG 工作负载生成

| 类型 | 参数 |
|------|------|
| Erdős-Rényi | (n_nodes, edge_prob, ccr) |
| 层次化 | (layers, width_range, ccr) |
| JSON 导入 | ompTG 格式兼容 |

---

## 4. 设备模拟器抽离

### 4.1 DeviceSimulator Trait

```rust
pub trait DeviceSimulator: Send {
    fn start_job(&mut self, job_id: JobId, work_ns: Nanos, now: Nanos) -> DeviceEvents;
    fn preempt(&mut self, job_id: JobId, now: Nanos) -> PreemptResult;
    fn status(&self) -> DeviceStatus;
    fn on_tick(&mut self, now: Nanos) -> DeviceEvents;
}
```

### 4.2 四种设备实现

| 设备 | 状态机 | 特殊行为 |
|------|--------|---------|
| `CpuSimulator` | Idle ↔ Running | 全抢占，多核 partitioned/global |
| `GpuSimulator` | Idle → Transferring → Running → PreemptionWindow | kernel 边界抢占 |
| `DspSimulator` | Idle → Running → DmaBlocked | ISR 中断级，DMA 不可抢占 |
| `FpgaSimulator` | Idle → Configuring → Running | PR 重配时间 |

### 4.3 架构关系

```
SimEngine ──owns──→ DeviceManager ──owns──→ Vec<Box<dyn DeviceSimulator>>
                        │
                        ├── 路由 dispatch_job()
                        ├── 聚合状态 → SchedulerView
                        └── ready queue 管理
```

### 4.4 扩展接口

预留 gem5/存算一体模拟器接入点 — 通过 IPC 实现 `DeviceSimulator` trait。

---

## 5. 调度器集合

### 5.1 FP-Het (已实现 ✅)
- 固定优先级，Rate-Monotonic
- ~148 LOC

### 5.2 EDF-Het (新增)
- 动态优先级，deadline 最近优先
- `effective_priority` = `u32::MAX - deadline`
- ~150 LOC

### 5.3 LLF-Het (新增)
- 动态优先级，laxity = deadline - now - remaining
- 每个 preemption_point 重算 laxity
- ~200 LOC

### 5.4 HEFT (新增 — DAG 专用)
- 离线: rank_u 计算 + 设备映射
- 在线: 查表 dispatch
- ~350 LOC

```rust
pub struct HeftPlanner {
    rank_u: HashMap<SubTaskIdx, f64>,
    device_mapping: HashMap<SubTaskIdx, DeviceId>,
}

pub struct HeftScheduler {
    planner: HeftPlanner,
}
```

### 5.5 后续扩展 (Phase 6+)
- CP-EDF (DAG 关键路径)
- Federated Scheduling
- SHAPE 算法 (自挂起模型)
- EDF-VD (MC 调度)

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
| 测试 | 验证内容 |
|------|---------|
| `liu_layland_rm_bound` | U ≤ n(2^(1/n)-1) 时 FP 无 miss |
| `edf_exact_bound` | U ≤ 1.0 时 EDF 无 miss |
| `joseph_pandya_rta` | R_sim ≤ R_rta |
| `audsley_opa` | 最优优先级分配 |
| `non_preemptive_rta` | George-Rivière RTA |
| `dm_optimality` | DM = RM for implicit deadline |

### 7.3 Level 2 — 小规模穷举 (3 tests)
- 2 任务 + 1 CPU 穷举
- 3 任务 + 2 设备手工验证

### 7.4 Level 3 — 跨模拟器对比 (2 tests)
- SimSo JSON 导入导出
- 同配置 deadline miss 对比

### 7.5 Level 4 — 异构特有 (5 tests)
- GPU kernel 边界抢占
- DSP DMA 阻塞项
- FPGA PR 重配开销
- 跨设备传输延迟
- 混合抢占 RTA B_blocking

### 7.6 Level 5 — SOTA 复现 (3 tests)
- SHAPE 可调度率曲线 (趋势 ±5%)
- HEFT makespan 经典结果
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

### 8.2 Trace 格式

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

## 9. 实现阶段

### Phase 1: DAG 引擎 + 设备状态机 [基础设施]
- DagTracker 实现
- DeviceSimulator trait + 4 种设备
- DAG 工作负载生成
- 集成测试: DAG 在 4 设备执行

### Phase 2: EDF-Het + HEFT [核心功能]
- EDF-Het 实现 + 单元测试
- HEFT (rank 计算 + 在线 dispatch)
- CLI `--scheduler` 切换
- 初步对比实验

### Phase 3: LLF-Het + 任务链 + Trace [完善]
- LLF-Het 实现
- 任务链端到端分析
- JSON trace 输出
- Python 绘图脚本

### Phase 4: 验证框架 [质量保证]
- Level 1-4 全部测试
- RTA 求解器

### Phase 5: Level 5 复现 + 论文实验 [论文产出]
- SHAPE 复现
- HEFT 经典结果
- 论文 #1 实验数据
- 论文图表生成

---

## 10. 文件结构变更

```
hprss-sim/
├── crates/
│   ├── hprss-engine/src/
│   │   ├── dag_tracker.rs      # 新增
│   │   └── trace_writer.rs     # 新增
│   ├── hprss-devices/src/
│   │   ├── lib.rs              # DeviceSimulator trait
│   │   ├── cpu.rs              # CpuSimulator
│   │   ├── gpu.rs              # GpuSimulator
│   │   ├── dsp.rs              # DspSimulator
│   │   └── fpga.rs             # FpgaSimulator
│   ├── hprss-scheduler/src/
│   │   ├── edf.rs              # 新增
│   │   ├── llf.rs              # 新增
│   │   └── heft.rs             # 新增
│   ├── hprss-validate/src/
│   │   ├── lib.rs              # ValidationTest trait
│   │   ├── rta.rs              # RTA 求解器
│   │   ├── level1/             # 经典理论
│   │   ├── level2/             # 穷举验证
│   │   ├── level3/             # 跨模拟器
│   │   ├── level4/             # 异构特有
│   │   └── level5/             # SOTA 复现
│   └── hprss-workload/src/
│       └── dag_generator.rs    # 新增
└── scripts/
    ├── plot_schedulability.py
    ├── plot_gantt.py
    ├── plot_response_time.py
    └── plot_comparison.py
```

---

## 11. 风险与缓解

| 风险 | 缓解措施 |
|------|---------|
| DAG 引擎复杂度高 | 先实现简单链式 DAG，再扩展通用 DAG |
| HEFT rank 计算性能 | 离线一次计算，缓存结果 |
| Level 5 复现难度 | 先趋势对比 (±5%)，不追求精确复现 |
| ompTG 格式未定 | 设计灵活 JSON schema，后续适配 |

---

## 12. 成功标准

- [ ] 4 调度器全部实现并通过单元测试
- [ ] DAG 任务在 4 设备上正确执行
- [ ] Level 1-4 验证全部通过
- [ ] 生成可用于论文的可调度率曲线图
- [ ] SHAPE 趋势复现误差 ≤ 5%
