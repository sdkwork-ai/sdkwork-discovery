use sdkwork_discovery_client::{select_target, HttpTarget, RegistryStatus, TargetState};

fn target(instance: &str, priority: u32, weight: u32) -> HttpTarget {
    HttpTarget {
        instance_id: instance.to_owned(),
        endpoint: format!("https://{instance}.example.com"),
        priority,
        weight,
        status: RegistryStatus::Serving,
    }
}

#[test]
fn selection_prefers_lower_priority_then_higher_weight() {
    let states = vec![
        TargetState::healthy(target("b", 10, 100)),
        TargetState::healthy(target("a", 10, 200)),
        TargetState::healthy(target("c", 20, 1000)),
    ];
    let selected = select_target(&states, 0).expect("a target is selected");
    assert_eq!(selected.instance_id, "a");
}

#[test]
fn selection_skips_targets_in_cooldown() {
    let mut demoted = TargetState::healthy(target("a", 10, 200));
    demoted.down_until_ms = 5_000;
    let states = vec![demoted, TargetState::healthy(target("b", 10, 100))];
    let selected = select_target(&states, 4_999).expect("the healthy target is selected");
    assert_eq!(selected.instance_id, "b");
    let after = select_target(&states, 5_000).expect("the cooldown expired");
    assert_eq!(after.instance_id, "a");
}

#[test]
fn selection_skips_non_serving_targets() {
    let mut target = target("a", 10, 200);
    target.status = RegistryStatus::NotServing;
    let selected = select_target(&[TargetState::healthy(target)], 0);
    assert!(selected.is_none());
}
