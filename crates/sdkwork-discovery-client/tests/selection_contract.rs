use std::collections::HashMap;

use sdkwork_discovery_client::{
    select_target, select_target_with, HttpTarget, LoadBalanceStrategy, RegistryStatus, TargetState,
};

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

#[test]
fn round_robin_rotates_across_admissible_targets() {
    let states = vec![
        TargetState::healthy(target("a", 10, 200)),
        TargetState::healthy(target("b", 10, 100)),
    ];
    let mut cursor = 0;
    let mut seed = 0x1234;
    let first = select_target_with(
        &states,
        0,
        LoadBalanceStrategy::RoundRobin,
        &mut cursor,
        &mut seed,
    )
    .expect("first target");
    let second = select_target_with(
        &states,
        0,
        LoadBalanceStrategy::RoundRobin,
        &mut cursor,
        &mut seed,
    )
    .expect("second target");
    let third = select_target_with(
        &states,
        0,
        LoadBalanceStrategy::RoundRobin,
        &mut cursor,
        &mut seed,
    )
    .expect("third target");
    assert_ne!(first.instance_id, second.instance_id);
    assert_eq!(first.instance_id, third.instance_id);
}

#[test]
fn weighted_random_favours_the_heavier_target() {
    let states = vec![
        TargetState::healthy(target("heavy", 10, 9)),
        TargetState::healthy(target("light", 10, 1)),
    ];
    let mut cursor = 0;
    let mut seed = 0x5EED_1234;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for _ in 0..1_000 {
        let selected = select_target_with(
            &states,
            0,
            LoadBalanceStrategy::WeightedRandom,
            &mut cursor,
            &mut seed,
        )
        .expect("weighted target");
        *counts.entry(selected.instance_id).or_default() += 1;
    }
    let heavy = *counts.get("heavy").unwrap_or(&0);
    let light = *counts.get("light").unwrap_or(&0);
    assert!(heavy > light * 3, "heavy={heavy} light={light}");
}

#[test]
fn load_balancing_stays_within_the_best_priority_tier() {
    let states = vec![
        TargetState::healthy(target("primary", 10, 100)),
        TargetState::healthy(target("dr", 100, 1_000)),
    ];
    let mut cursor = 0;
    let mut seed = 0x42;
    for _ in 0..5 {
        let selected = select_target_with(
            &states,
            0,
            LoadBalanceStrategy::RoundRobin,
            &mut cursor,
            &mut seed,
        )
        .expect("primary target");
        assert_eq!(selected.instance_id, "primary");
    }
}

#[test]
fn load_balancer_names_parse_to_strategies() {
    assert_eq!(
        LoadBalanceStrategy::parse("round_robin"),
        Some(LoadBalanceStrategy::RoundRobin)
    );
    assert_eq!(
        LoadBalanceStrategy::parse("WEIGHTED"),
        Some(LoadBalanceStrategy::WeightedRandom)
    );
    assert_eq!(LoadBalanceStrategy::parse("unknown"), None);
    for name in LoadBalanceStrategy::SUPPORTED {
        assert!(
            LoadBalanceStrategy::parse(name).is_some(),
            "supported name parses: {name}"
        );
    }
}
