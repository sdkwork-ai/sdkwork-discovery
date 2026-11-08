use crate::registry::HttpTarget;

/// Runtime state attached to one registry target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetState {
    /// The registered target.
    pub target: HttpTarget,
    /// Consecutive failed calls since the last success.
    pub consecutive_failures: u32,
    /// Epoch milliseconds until which the target is demoted.
    pub down_until_ms: u64,
}

impl TargetState {
    /// Builds healthy state for a target discovered from the database.
    pub fn healthy(target: HttpTarget) -> Self {
        Self {
            target,
            consecutive_failures: 0,
            down_until_ms: 0,
        }
    }
}

/// Load-balancing strategy applied to the currently-admissible targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadBalanceStrategy {
    /// Lower `priority` first, then higher `weight`, then stable instance id.
    #[default]
    Priority,
    /// Rotate through the admissible targets in priority/weight order.
    RoundRobin,
    /// Uniform random choice over the admissible targets.
    Random,
    /// Weighted random choice (`weight` acts as the relative probability).
    WeightedRandom,
}

impl LoadBalanceStrategy {
    /// Parses the operator-facing strategy name.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "priority" | "priority_weighted" | "weighted_priority" => Some(Self::Priority),
            "round_robin" | "roundrobin" | "rr" => Some(Self::RoundRobin),
            "random" => Some(Self::Random),
            "weighted_random" | "weighted" => Some(Self::WeightedRandom),
            _ => None,
        }
    }

    /// Stable label for logs and configuration diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::RoundRobin => "round_robin",
            Self::Random => "random",
            Self::WeightedRandom => "weighted_random",
        }
    }

    /// Every supported strategy name.
    pub const SUPPORTED: [&'static str; 4] =
        ["priority", "round_robin", "random", "weighted_random"];
}

/// Selects the best currently-admissible target using the priority strategy.
///
/// Ordering is the QoS contract: lower `priority` first, then higher `weight`,
/// then a stable instance id. Targets in a circuit-breaker cooldown or in a
/// non-discoverable status are skipped. `None` means the caller degrades to the
/// authored topology value.
pub fn select_target(states: &[TargetState], now_ms: u64) -> Option<HttpTarget> {
    let mut cursor = 0;
    let mut seed = 0x9E37_79B9_7F4A_7C15;
    select_target_with(
        states,
        now_ms,
        LoadBalanceStrategy::Priority,
        &mut cursor,
        &mut seed,
    )
}

/// Selects a target with an explicit strategy.
///
/// `cursor` is advanced for round-robin; `seed` is advanced for the random
/// strategies. Both are caller-owned so the resolver can keep one state per
/// process without locking inside this pure function.
pub fn select_target_with(
    states: &[TargetState],
    now_ms: u64,
    strategy: LoadBalanceStrategy,
    cursor: &mut usize,
    seed: &mut u64,
) -> Option<HttpTarget> {
    let mut candidates: Vec<&TargetState> = states
        .iter()
        .filter(|state| state.target.status.is_discoverable())
        .filter(|state| state.down_until_ms <= now_ms)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    // Priority is the QoS tier: all strategies spread traffic only inside the
    // best admissible tier, so a lower-priority DR site never receives traffic
    // while a healthy primary exists.
    let best_priority = candidates
        .iter()
        .map(|state| state.target.priority)
        .min()
        .unwrap_or_default();
    candidates.retain(|state| state.target.priority == best_priority);
    candidates.sort_by(|left, right| {
        left.target
            .priority
            .cmp(&right.target.priority)
            .then_with(|| right.target.weight.cmp(&left.target.weight))
            .then_with(|| left.target.instance_id.cmp(&right.target.instance_id))
    });

    match strategy {
        LoadBalanceStrategy::Priority => candidates.first().map(|state| state.target.clone()),
        LoadBalanceStrategy::RoundRobin => {
            let index = cursor.wrapping_rem(candidates.len());
            *cursor = cursor.wrapping_add(1);
            candidates.get(index).map(|state| state.target.clone())
        }
        LoadBalanceStrategy::Random => {
            let index = (next_u64(seed) as usize) % candidates.len();
            candidates.get(index).map(|state| state.target.clone())
        }
        LoadBalanceStrategy::WeightedRandom => {
            let total: u64 = candidates
                .iter()
                .map(|state| u64::from(state.target.weight.max(1)))
                .sum();
            let mut pick = next_u64(seed) % total.max(1);
            for candidate in &candidates {
                let weight = u64::from(candidate.target.weight.max(1));
                if pick < weight {
                    return Some(candidate.target.clone());
                }
                pick -= weight;
            }
            candidates.last().map(|state| state.target.clone())
        }
    }
}

/// xorshift64* step; deterministic for a given seed, adequate for weighted
/// load spreading, not for cryptography.
fn next_u64(seed: &mut u64) -> u64 {
    let mut value = *seed;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *seed = value;
    value.wrapping_mul(0x2545_F491_4F6C_DD1D)
}
