use clap::ValueEnum;
use hprss_scheduler::{
    CpEdfScheduler, EdfScheduler, EdfVdScheduler, FederatedScheduler, FixedPriorityScheduler,
    GPreemptScheduler, GangScheduler, GcapsScheduler, GlobalEdfScheduler,
    GpuPreemptivePriorityScheduler, HeftScheduler, LlfScheduler, MatchScheduler, RtgpuScheduler,
    XSchedScheduler,
};
use hprss_types::Scheduler;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SchedulerKind {
    Fp,
    Edf,
    Edfvd,
    Llf,
    Heft,
    Cpedf,
    Federated,
    GlobalEdf,
    Gang,
    Xsched,
    Gcaps,
    Gpreempt,
    Rtgpu,
    Match,
    GpuPreemptivePriority,
}

pub(crate) fn parse_scheduler_list(s: &str) -> Result<Vec<SchedulerKind>, String> {
    let mut out = Vec::new();
    for token in s.split(',').map(|v| v.trim()).filter(|v| !v.is_empty()) {
        let kind = match token.to_ascii_lowercase().as_str() {
            "fp" => SchedulerKind::Fp,
            "edf" => SchedulerKind::Edf,
            "edfvd" => SchedulerKind::Edfvd,
            "llf" => SchedulerKind::Llf,
            "heft" => SchedulerKind::Heft,
            "cpedf" => SchedulerKind::Cpedf,
            "federated" => SchedulerKind::Federated,
            "global-edf" | "global_edf" | "globaledf" | "gedf" => SchedulerKind::GlobalEdf,
            "gang" => SchedulerKind::Gang,
            "xsched" | "x-sched" => SchedulerKind::Xsched,
            "gcaps" => SchedulerKind::Gcaps,
            "gpreempt" | "g-preempt" => SchedulerKind::Gpreempt,
            "rtgpu" | "rt-gpu" => SchedulerKind::Rtgpu,
            "match" => SchedulerKind::Match,
            "gpu-preemptive-priority"
            | "gpu_preemptive_priority"
            | "gpu-preempt-priority"
            | "gpupp" => SchedulerKind::GpuPreemptivePriority,
            _ => return Err(format!("unknown scheduler: {token}")),
        };
        out.push(kind);
    }
    if out.is_empty() {
        return Err("scheduler list is empty".to_string());
    }
    Ok(out)
}

pub(crate) fn build_scheduler(kind: SchedulerKind) -> Box<dyn Scheduler> {
    match kind {
        SchedulerKind::Fp => Box::new(FixedPriorityScheduler),
        SchedulerKind::Edf => Box::new(EdfScheduler),
        SchedulerKind::Edfvd => Box::new(EdfVdScheduler::default()),
        SchedulerKind::Llf => Box::new(LlfScheduler::default()),
        SchedulerKind::Heft => Box::new(HeftScheduler::default()),
        SchedulerKind::Cpedf => Box::new(CpEdfScheduler::default()),
        SchedulerKind::Federated => Box::new(FederatedScheduler::default()),
        SchedulerKind::GlobalEdf => Box::new(GlobalEdfScheduler),
        SchedulerKind::Gang => Box::new(GangScheduler::default()),
        SchedulerKind::Xsched => Box::new(XSchedScheduler),
        SchedulerKind::Gcaps => Box::new(GcapsScheduler),
        SchedulerKind::Gpreempt => Box::new(GPreemptScheduler),
        SchedulerKind::Rtgpu => Box::new(RtgpuScheduler),
        SchedulerKind::Match => Box::new(MatchScheduler::default()),
        SchedulerKind::GpuPreemptivePriority => Box::new(GpuPreemptivePriorityScheduler),
    }
}

pub(crate) fn scheduler_label(kind: SchedulerKind) -> &'static str {
    match kind {
        SchedulerKind::Fp => "FP-Het",
        SchedulerKind::Edf => "EDF-Het",
        SchedulerKind::Edfvd => "EDF-VD-Het",
        SchedulerKind::Llf => "LLF-Het",
        SchedulerKind::Heft => "HEFT",
        SchedulerKind::Cpedf => "CP-EDF",
        SchedulerKind::Federated => "Federated",
        SchedulerKind::GlobalEdf => "Global-EDF",
        SchedulerKind::Gang => "Gang",
        SchedulerKind::Xsched => "XSched",
        SchedulerKind::Gcaps => "GCAPS",
        SchedulerKind::Gpreempt => "GPreempt",
        SchedulerKind::Rtgpu => "RTGPU",
        SchedulerKind::Match => "MATCH",
        SchedulerKind::GpuPreemptivePriority => "GPU-Preemptive-Priority",
    }
}

pub(crate) fn scheduler_key(kind: SchedulerKind) -> &'static str {
    match kind {
        SchedulerKind::Fp => "fp",
        SchedulerKind::Edf => "edf",
        SchedulerKind::Edfvd => "edfvd",
        SchedulerKind::Llf => "llf",
        SchedulerKind::Heft => "heft",
        SchedulerKind::Cpedf => "cpedf",
        SchedulerKind::Federated => "federated",
        SchedulerKind::GlobalEdf => "global-edf",
        SchedulerKind::Gang => "gang",
        SchedulerKind::Xsched => "xsched",
        SchedulerKind::Gcaps => "gcaps",
        SchedulerKind::Gpreempt => "gpreempt",
        SchedulerKind::Rtgpu => "rtgpu",
        SchedulerKind::Match => "match",
        SchedulerKind::GpuPreemptivePriority => "gpu-preemptive-priority",
    }
}

pub(crate) fn scheduler_family(kind: SchedulerKind) -> &'static str {
    match kind {
        SchedulerKind::Fp | SchedulerKind::Federated | SchedulerKind::Gang => "fixed-priority",
        SchedulerKind::Edf | SchedulerKind::Edfvd | SchedulerKind::GlobalEdf => "deadline-driven",
        SchedulerKind::Llf => "laxity-driven",
        SchedulerKind::Heft | SchedulerKind::Cpedf => "dag-aware",
        SchedulerKind::Xsched
        | SchedulerKind::Gcaps
        | SchedulerKind::Gpreempt
        | SchedulerKind::Rtgpu
        | SchedulerKind::Match
        | SchedulerKind::GpuPreemptivePriority => "paper-heterogeneous",
    }
}
