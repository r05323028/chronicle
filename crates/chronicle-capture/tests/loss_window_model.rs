//! Portable loss-window state-machine boundary matrix (migration 3.6).
//!
//! Split from chronicle-capture-ebpf privileged feasibility suite: sampler
//! window/no-growth transitions and reason classification are pure logic and
//! run on any host; the eBPF sampler must satisfy this contract on supported
//! Linux. This copy guards the model everywhere.

#[derive(Debug, PartialEq, Eq)]
enum LossModelObservation {
    NoWalGrowth,
    Window {
        start_ns: u64,
        end_ns: u64,
        delta: u64,
        reason: Option<&'static str>,
        final_sample: bool,
    },
}

struct FakeLossSampler {
    attach_ns: u64,
    previous: Option<(u64, u64, u64, Vec<u64>)>,
}

impl FakeLossSampler {
    fn new(attach_ns: u64) -> Self {
        Self {
            attach_ns,
            previous: None,
        }
    }

    fn sample(
        &mut self,
        now_ns: u64,
        map_id: u64,
        clock_id: u64,
        current: Option<Vec<u64>>,
        final_sample: bool,
    ) -> LossModelObservation {
        let start_ns = self
            .previous
            .as_ref()
            .map_or(self.attach_ns, |previous| previous.0);
        let Some(current) = current else {
            return LossModelObservation::Window {
                start_ns,
                end_ns: now_ns,
                delta: 0,
                reason: Some("final_sample_failure"),
                final_sample,
            };
        };
        let reason = self.previous.as_ref().and_then(|previous| {
            if previous.1 != map_id {
                Some("map_replacement")
            } else if previous.2 != clock_id {
                Some("clock_mismatch")
            } else if previous.3.len() != current.len() {
                Some("partial_read")
            } else if previous
                .3
                .iter()
                .zip(&current)
                .any(|(before, after)| after < before)
            {
                Some("counter_reset")
            } else {
                None
            }
        });
        let delta = if reason.is_some() {
            0
        } else if let Some(previous) = &self.previous {
            previous
                .3
                .iter()
                .zip(&current)
                .map(|(before, after)| after - before)
                .sum()
        } else {
            current.iter().sum()
        };
        self.previous = Some((now_ns, map_id, clock_id, current));
        if delta == 0 && reason.is_none() {
            LossModelObservation::NoWalGrowth
        } else {
            LossModelObservation::Window {
                start_ns,
                end_ns: now_ns,
                delta,
                reason,
                final_sample,
            }
        }
    }
}

#[test]
fn loss_sampling_state_machine_covers_all_gate_a_boundaries() {
    let mut sampler = FakeLossSampler::new(10);
    assert_eq!(
        sampler.sample(100_000_010, 1, 7, Some(vec![3, 4]), false),
        LossModelObservation::Window {
            start_ns: 10,
            end_ns: 100_000_010,
            delta: 7,
            reason: None,
            final_sample: false,
        }
    );
    assert_eq!(
        sampler.sample(200_000_010, 1, 7, Some(vec![3, 4]), false),
        LossModelObservation::NoWalGrowth
    );
    assert_eq!(
        sampler.sample(475_000_010, 1, 7, Some(vec![5, 7]), false),
        LossModelObservation::Window {
            start_ns: 200_000_010,
            end_ns: 475_000_010,
            delta: 5,
            reason: None,
            final_sample: false,
        }
    );
    for (map, clock, values, reason) in [
        (1, 7, vec![2, 7], "counter_reset"),
        (2, 7, vec![2, 7], "map_replacement"),
        (2, 8, vec![2, 7], "clock_mismatch"),
        (2, 8, vec![2], "partial_read"),
    ] {
        assert!(matches!(
            sampler.sample(575_000_010, map, clock, Some(values), false),
            LossModelObservation::Window {
                reason: Some(actual),
                ..
            } if actual == reason
        ));
    }
    assert!(matches!(
        sampler.sample(675_000_010, 2, 8, None, true),
        LossModelObservation::Window {
            reason: Some("final_sample_failure"),
            final_sample: true,
            ..
        }
    ));
}
