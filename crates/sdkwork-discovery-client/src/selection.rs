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

/// Selects the best currently-admissible target.
///
/// Ordering is the QoS contract: lower `priority` first, then higher `weight`,
/// then a stable instance id. Targets in a circuit-breaker cooldown or in a
/// non-discoverable status are skipped. `None` means the caller degrades to the
/// authored topology value.
pub fn select_target(states: &[TargetState], now_ms: u64) -> Option<HttpTarget> {
    states
        .iter()
        .filter(|state| state.target.status.is_discoverable())
        .filter(|state| state.down_until_ms <= now_ms)
        .min_by(|left, right| {
            left.target
                .priority
                .cmp(&right.target.priority)
                .then_with(|| right.target.weight.cmp(&left.target.weight))
                .then_with(|| left.target.instance_id.cmp(&right.target.instance_id))
        })
        .map(|state| state.target.clone())
}
