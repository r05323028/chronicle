#!/usr/bin/env bash
# P2 dispatch for the shared user-intent lifecycle scenario. The implementation
# is self-contained (own data dir, cgroup subtree, targets), so it runs equally
# under the P2 runtime without depending on daemon state.
scenario_p2_user_intent_lifecycle() {
	user_intent_lifecycle_impl
}
