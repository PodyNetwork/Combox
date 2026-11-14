//! State machines for tasks, workers, and payouts.
//!
//! Each status type lists its allowed transitions explicitly. Anything not in
//! the list is an error; [`crate::db::transition_task`] and friends also write
//! the rejected attempt to the audit log.

use serde::{Deserialize, Serialize};

use crate::str_enum;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[error("invalid {machine} transition: {from} -> {to}")]
pub struct TransitionError {
    pub machine: &'static str,
    pub from: String,
    pub to: String,
}

/// A status enum with an explicit transition table.
pub trait StateMachine: Copy + Eq + std::fmt::Display + Sized + 'static {
    const NAME: &'static str;

    /// States a new record may start in.
    fn initial_states() -> &'static [Self];

    /// States reachable in one step from `self`.
    fn allowed_next(&self) -> &'static [Self];

    fn is_terminal(&self) -> bool {
        self.allowed_next().is_empty()
    }

    fn can_transition_to(&self, to: Self) -> bool {
        self.allowed_next().contains(&to)
    }

    /// Validate a transition, returning the new state.
    fn transition(self, to: Self) -> Result<Self, TransitionError> {
        if self.can_transition_to(to) {
            Ok(to)
        } else {
            Err(TransitionError { machine: Self::NAME, from: self.to_string(), to: to.to_string() })
        }
    }

    /// Validate an initial state for a new record.
    fn start(state: Self) -> Result<Self, TransitionError> {
        if Self::initial_states().contains(&state) {
            Ok(state)
        } else {
            Err(TransitionError { machine: Self::NAME, from: "(new)".into(), to: state.to_string() })
        }
    }
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Assigned,
    InProgress,
    Blocked,
    Done,
    Failed,
}
str_enum!(TaskStatus, "task status", {
    Queued => "queued", Assigned => "assigned", InProgress => "in_progress",
    Blocked => "blocked", Done => "done", Failed => "failed",
});

impl StateMachine for TaskStatus {
    const NAME: &'static str = "task";

    fn initial_states() -> &'static [Self] {
        &[TaskStatus::Queued]
    }

    fn allowed_next(&self) -> &'static [Self] {
        use TaskStatus::*;
        match self {
            // Queued tasks get assigned by routing.
            Queued => &[Assigned, Blocked],
            // Assigned tasks start, or go back to the queue on reassignment.
            Assigned => &[InProgress, Queued, Blocked],
            InProgress => &[Done, Failed, Blocked, Queued],
            Blocked => &[Queued, Assigned, InProgress, Failed],
            Done => &[],
            // A failed task may be retried by re-queueing it.
            Failed => &[Queued],
        }
    }
}

impl TaskStatus {
    /// Statuses that count toward a worker's current load.
    pub const OPEN_FOR_LOAD: &'static [TaskStatus] =
        &[TaskStatus::Assigned, TaskStatus::InProgress, TaskStatus::Blocked];
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Candidate,
    Active,
    OnProbation,
    Suspended,
    Fired,
}
str_enum!(WorkerStatus, "worker status", {
    Candidate => "candidate", Active => "active", OnProbation => "on_probation",
    Suspended => "suspended", Fired => "fired",
});

impl StateMachine for WorkerStatus {
    const NAME: &'static str = "worker";

    fn initial_states() -> &'static [Self] {
        // AI agents are created active (autonomous hire); humans start as candidates.
        &[WorkerStatus::Candidate, WorkerStatus::Active]
    }

    fn allowed_next(&self) -> &'static [Self] {
        use WorkerStatus::*;
        match self {
            // A rejected candidate is marked fired (terminal) for simplicity.
            Candidate => &[Active, Fired],
            Active => &[OnProbation, Suspended, Fired],
            OnProbation => &[Active, Suspended, Fired],
            Suspended => &[Active, OnProbation, Fired],
            Fired => &[],
        }
    }
}

impl WorkerStatus {
    /// Statuses that may receive new task assignments.
    pub fn can_receive_work(&self) -> bool {
        matches!(self, WorkerStatus::Active | WorkerStatus::OnProbation)
    }
}

// ---------------------------------------------------------------------------
// Payout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoutStatus {
    PendingApproval,
    Approved,
    Sent,
    Finalized,
    Failed,
    Rejected,
    Expired,
    CapExceeded,
    /// A first-payout gate failed (unverified address or missing tax form).
    Blocked,
    /// Cross-chain only: NEAR Intents could not deliver and returned the
    /// deposit to the treasury (as wNEAR). The cap stays committed.
    Refunded,
}
str_enum!(PayoutStatus, "payout status", {
    PendingApproval => "pending_approval", Approved => "approved", Sent => "sent",
    Finalized => "finalized", Failed => "failed", Rejected => "rejected",
    Expired => "expired", CapExceeded => "cap_exceeded", Blocked => "blocked",
    Refunded => "refunded",
});

impl StateMachine for PayoutStatus {
    const NAME: &'static str = "payout";

    fn initial_states() -> &'static [Self] {
        use PayoutStatus::*;
        // Approved = automatic under-threshold attestation.
        &[PendingApproval, Approved, CapExceeded, Blocked]
    }

    fn allowed_next(&self) -> &'static [Self] {
        use PayoutStatus::*;
        match self {
            PendingApproval => &[Approved, Rejected, Expired],
            Approved => &[Sent, Failed],
            // Refunded: a cross-chain deposit NEAR Intents returned to the treasury.
            Sent => &[Finalized, Failed, Refunded],
            // A failed send may be retried; after the last retry it stays failed.
            Failed => &[Sent],
            // Blocked payouts are re-created on the next payroll run once the gate passes.
            Finalized | Rejected | Expired | CapExceeded | Blocked | Refunded => &[],
        }
    }
}

impl PayoutStatus {
    /// Statuses whose cap reservation should be released.
    pub fn releases_reservation(&self) -> bool {
        matches!(self, PayoutStatus::Rejected | PayoutStatus::Expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_happy_path() {
        let s = TaskStatus::start(TaskStatus::Queued).unwrap();
        let s = s.transition(TaskStatus::Assigned).unwrap();
        let s = s.transition(TaskStatus::InProgress).unwrap();
        let s = s.transition(TaskStatus::Done).unwrap();
        assert!(s.is_terminal());
    }

    #[test]
    fn task_invalid_transitions() {
        assert!(TaskStatus::Queued.transition(TaskStatus::Done).is_err());
        assert!(TaskStatus::Done.transition(TaskStatus::Queued).is_err());
        assert!(TaskStatus::start(TaskStatus::Done).is_err());
        let err = TaskStatus::Queued.transition(TaskStatus::InProgress).unwrap_err();
        assert_eq!(err.machine, "task");
        assert_eq!(err.from, "queued");
        assert_eq!(err.to, "in_progress");
    }

    #[test]
    fn worker_fired_is_terminal() {
        for &s in WorkerStatus::ALL {
            assert!(WorkerStatus::Fired.transition(s).is_err());
        }
        assert!(WorkerStatus::Active.transition(WorkerStatus::Fired).is_ok());
        assert!(WorkerStatus::Candidate.transition(WorkerStatus::OnProbation).is_err());
        assert!(!WorkerStatus::Suspended.can_receive_work());
    }

    #[test]
    fn payout_transitions() {
        use PayoutStatus::*;
        assert!(PendingApproval.transition(Approved).is_ok());
        assert!(PendingApproval.transition(Sent).is_err(), "cannot skip approval");
        assert!(CapExceeded.transition(Approved).is_err(), "approval cannot override the cap");
        assert!(Rejected.transition(Approved).is_err());
        assert!(Expired.is_terminal());
        assert!(PayoutStatus::start(Sent).is_err());
        assert!(Approved.transition(Sent).and_then(|s| s.transition(Finalized)).is_ok());
        assert!(Sent.transition(Refunded).is_ok());
        assert!(Approved.transition(Refunded).is_err(), "only a sent deposit can be refunded");
        assert!(Refunded.is_terminal());
    }

    #[test]
    fn every_target_state_is_reachable() {
        fn check<S: StateMachine + std::fmt::Debug>(all: &[S]) {
            for target in all {
                let reachable = S::initial_states().contains(target)
                    || all.iter().any(|s| s.allowed_next().contains(target));
                assert!(reachable, "{target:?} unreachable");
            }
        }
        check(TaskStatus::ALL);
        check(WorkerStatus::ALL);
        check(PayoutStatus::ALL);
    }
}
