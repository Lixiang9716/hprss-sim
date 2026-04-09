use hprss_types::{CriticalityLevel, task::DeviceType};
use hprss_workload::adapt_openmp_specialized_json_str;

#[test]
fn openmp_adapter_maps_regions_instances_and_assumptions() {
    let input = r#"
    {
      "regions": [
        {
          "region_id": 10,
          "name": "omp-matmul",
          "priority": 2,
          "criticality": "Hi",
          "requested_threads": 16,
          "schedule_kind": "dynamic",
          "chunk_size": 4,
          "loop_iteration_count": 2048,
          "affinity": ["Cpu", "Gpu"],
          "device_profiles": [
            { "device_type": "Cpu", "wcet_ns": 50000 },
            { "device_type": "Gpu", "wcet_ns": 12000 }
          ],
          "relative_deadline_ns": 90000,
          "data_size": 4096
        }
      ],
      "instances": [
        {
          "region_id": 10,
          "release_ns": 1000,
          "absolute_deadline_ns": 70000,
          "observed_exec_ns": 10000
        },
        {
          "region_id": 10,
          "release_ns": 80000,
          "observed_exec_ns": null
        }
      ]
    }
    "#;

    let replay = adapt_openmp_specialized_json_str(input).expect("openmp json should adapt");
    assert_eq!(replay.tasks.len(), 1);
    assert_eq!(replay.jobs().len(), 2);

    let task = &replay.tasks[0];
    assert_eq!(task.task_id, 10);
    assert_eq!(task.criticality, CriticalityLevel::Hi);
    assert_eq!(task.affinity, vec![DeviceType::Cpu, DeviceType::Gpu]);

    assert_eq!(replay.jobs()[0].release_ns, 1000);
    assert_eq!(replay.jobs()[0].actual_exec_ns, Some(10000));
    assert_eq!(replay.jobs()[1].release_ns, 80000);

    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|assumption| assumption.code == "omp-region-collapsed-to-task")
    );
    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|assumption| assumption.code == "omp-missing-observed-exec")
    );
}

#[test]
fn openmp_adapter_rejects_invalid_thread_count() {
    let input = r#"
    {
      "regions": [
        {
          "region_id": 1,
          "name": "bad-threads",
          "priority": 1,
          "requested_threads": 0,
          "schedule_kind": "static",
          "affinity": ["Cpu"],
          "device_profiles": [{ "device_type": "Cpu", "wcet_ns": 1000 }],
          "relative_deadline_ns": 2000,
          "data_size": 0
        }
      ],
      "instances": [{ "region_id": 1, "release_ns": 0 }]
    }
    "#;

    let err = adapt_openmp_specialized_json_str(input).expect_err("must reject zero threads");
    assert!(err.to_string().contains("requested_threads must be > 0"));
}

#[test]
fn openmp_adapter_derives_deadline_and_records_assumption() {
    let input = r#"
    {
      "regions": [
        {
          "region_id": 2,
          "name": "derived-deadline",
          "priority": 3,
          "requested_threads": 8,
          "schedule_kind": "guided",
          "affinity": ["Gpu"],
          "device_profiles": [{ "device_type": "Gpu", "wcet_ns": 20000 }],
          "data_size": 512
        }
      ],
      "instances": [{ "region_id": 2, "release_ns": 5000 }]
    }
    "#;

    let replay = adapt_openmp_specialized_json_str(input).expect("adapt should work");
    assert_eq!(replay.tasks[0].deadline_ns, 20000);
    assert_eq!(replay.jobs()[0].absolute_deadline_ns, Some(25000));
    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|assumption| assumption.code == "omp-derived-relative-deadline")
    );
}
