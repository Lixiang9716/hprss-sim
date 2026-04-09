use hprss_workload::{adapt_karami_paper_profile_json, adapt_karami_paper_profile_json_str};

#[test]
fn karami_adapter_parses_fixture_and_deterministically_maps_replay() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/karami_profile_sample.json");
    let replay = adapt_karami_paper_profile_json(&path).expect("fixture should adapt");

    assert_eq!(
        replay.metadata.source.as_deref(),
        Some("karami-paper-profile")
    );
    assert_eq!(replay.tasks.len(), 2);
    assert_eq!(replay.jobs().len(), 5);

    assert_eq!(replay.tasks[0].task_id, 1);
    assert_eq!(replay.tasks[1].task_id, 3);
    assert_eq!(replay.jobs()[0].release_ns, 0);
    assert_eq!(replay.jobs()[0].task_id, 3);
    assert_eq!(replay.jobs()[1].release_ns, 20000);
    assert_eq!(replay.jobs()[1].task_id, 1);
    assert_eq!(replay.jobs()[4].release_ns, 180000);

    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|a| a.code == "karami-paper-profile")
    );
    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|a| a.code == "karami-burst-window")
    );
}

#[test]
fn karami_adapter_derives_defaults_and_records_assumptions() {
    let input = r#"
    {
      "profile_name": "derived-defaults",
      "scenarios": [
        {
          "scenario_id": 7,
          "name": "decoder-token",
          "priority": 4,
          "period_ns": 50000,
          "release_offsets_ns": [1000, 2000],
          "affinity": ["Gpu"],
          "execution_profiles": [{ "device_type": "Gpu", "wcet_ns": 15000 }]
        }
      ]
    }
    "#;

    let replay = adapt_karami_paper_profile_json_str(input).expect("should adapt with defaults");
    assert_eq!(replay.tasks[0].deadline_ns, 50000);
    assert_eq!(replay.jobs()[0].absolute_deadline_ns, Some(51000));
    assert_eq!(replay.jobs()[1].absolute_deadline_ns, Some(52000));
    assert_eq!(replay.jobs()[0].actual_exec_ns, None);
    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|a| a.code == "karami-derived-relative-deadline")
    );
    assert!(
        replay
            .metadata
            .assumptions
            .iter()
            .any(|a| a.code == "karami-missing-observed-exec")
    );
}

#[test]
fn karami_adapter_rejects_duplicate_scenario_id() {
    let input = r#"
    {
      "profile_name": "duplicate-id",
      "scenarios": [
        {
          "scenario_id": 1,
          "name": "a",
          "priority": 1,
          "period_ns": 10000,
          "release_offsets_ns": [0],
          "affinity": ["Cpu"],
          "execution_profiles": [{ "device_type": "Cpu", "wcet_ns": 1000 }]
        },
        {
          "scenario_id": 1,
          "name": "b",
          "priority": 2,
          "period_ns": 20000,
          "release_offsets_ns": [0],
          "affinity": ["Gpu"],
          "execution_profiles": [{ "device_type": "Gpu", "wcet_ns": 1000 }]
        }
      ]
    }
    "#;

    let err = adapt_karami_paper_profile_json_str(input).expect_err("must reject duplicate id");
    assert!(err.to_string().contains("duplicate scenario_id 1"));
}
