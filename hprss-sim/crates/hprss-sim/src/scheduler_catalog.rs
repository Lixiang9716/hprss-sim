use clap::ValueEnum;
use hprss_scheduler::{
    CpEdfScheduler, EdfScheduler, EdfVdScheduler, FederatedScheduler, FixedPriorityScheduler,
    HeftScheduler, LlfScheduler,
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
    }
}
