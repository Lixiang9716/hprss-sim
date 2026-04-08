use hprss_devices::{
    DispatchTimingInput, PreemptionCheckInput, PreemptionDecision, VirtualDeviceModel,
};
use hprss_types::device::PreemptionModel;

#[test]
fn fully_preemptive_allows_immediate_preemption_and_no_boundary() {
    let model = VirtualDeviceModel::from_preemption(PreemptionModel::FullyPreemptive);
    let timing = model.evaluate_dispatch_timing(DispatchTimingInput {
        now_ns: 100,
        context_switch_ns: 20,
        remaining_exec_wall_ns: 1_000,
    });
    assert_eq!(timing.completion_time_ns, 1_120);
    assert_eq!(timing.next_preemption_point_ns, None);

    let decision = model.evaluate_preemption(PreemptionCheckInput {
        at_preemption_point: false,
    });
    assert_eq!(decision, PreemptionDecision::AllowNow);
}

#[test]
fn limited_preemptive_requires_boundary_and_sets_granularity_point() {
    let model = VirtualDeviceModel::from_preemption(PreemptionModel::LimitedPreemptive {
        granularity_ns: 500,
    });
    let timing = model.evaluate_dispatch_timing(DispatchTimingInput {
        now_ns: 10,
        context_switch_ns: 5,
        remaining_exec_wall_ns: 100,
    });
    assert_eq!(timing.completion_time_ns, 115);
    assert_eq!(timing.next_preemption_point_ns, Some(510));

    let blocked = model.evaluate_preemption(PreemptionCheckInput {
        at_preemption_point: false,
    });
    assert_eq!(blocked, PreemptionDecision::DeferUntilPreemptionPoint);

    let allowed = model.evaluate_preemption(PreemptionCheckInput {
        at_preemption_point: true,
    });
    assert_eq!(allowed, PreemptionDecision::AllowNow);
}

#[test]
fn interrupt_level_requires_boundary_and_uses_dma_window() {
    let model = VirtualDeviceModel::from_preemption(PreemptionModel::InterruptLevel {
        isr_overhead_ns: 40,
        dma_non_preemptive_ns: 900,
    });
    let timing = model.evaluate_dispatch_timing(DispatchTimingInput {
        now_ns: 0,
        context_switch_ns: 10,
        remaining_exec_wall_ns: 200,
    });
    assert_eq!(timing.completion_time_ns, 210);
    assert_eq!(timing.next_preemption_point_ns, Some(900));

    let decision = model.evaluate_preemption(PreemptionCheckInput {
        at_preemption_point: false,
    });
    assert_eq!(decision, PreemptionDecision::DeferUntilPreemptionPoint);
}

#[test]
fn non_preemptive_never_preempts_and_reports_reconfiguration_cost() {
    let model = VirtualDeviceModel::from_preemption(PreemptionModel::NonPreemptive {
        reconfig_time_ns: 2_000,
    });
    let timing = model.evaluate_dispatch_timing(DispatchTimingInput {
        now_ns: 25,
        context_switch_ns: 10,
        remaining_exec_wall_ns: 300,
    });
    assert_eq!(timing.completion_time_ns, 335);
    assert_eq!(timing.next_preemption_point_ns, None);
    assert_eq!(timing.additional_dispatch_delay_ns, 2_000);

    let decision = model.evaluate_preemption(PreemptionCheckInput {
        at_preemption_point: true,
    });
    assert_eq!(decision, PreemptionDecision::Never);
}
