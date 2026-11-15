//! Requires DATABASE_URL. Invalid transitions return errors and are audited.

use axom_core::audit;
use axom_core::db::{self, TaskPlacement, TransitionDbError};
use axom_core::domain::{Actor, Capabilities, Task};
use axom_core::state::TaskStatus;
use axom_core::{Decimal, Uuid};
use sqlx::PgPool;

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn invalid_task_transition_is_rejected_and_audited(pool: PgPool) {
    let t = Task {
        id: Uuid::new_v4(),
        objective: "write landing page".into(),
        required_capabilities: Capabilities::new(["postiz"], ["copywriting"]),
        priority: 2,
        deadline: None,
        assignee: None,
        status: TaskStatus::Queued,
        result: None,
        cost_usd: Decimal::ZERO,
        start_at: None, worker_kind: None,
    };
    db::insert_task(&pool, &t, &TaskPlacement::default()).await.unwrap();

    let err = db::transition(&pool, &Actor::System, t.id, TaskStatus::Done, "skip ahead").await.unwrap_err();
    assert!(matches!(err, TransitionDbError::Invalid(_)));
    assert_eq!(db::load_task(&pool, t.id).await.unwrap().unwrap().status, TaskStatus::Queued);

    db::transition(&pool, &Actor::System, t.id, TaskStatus::Assigned, "routed").await.unwrap();
    let events = audit::by_action(&pool, "task.transition").await.unwrap();
    let outcomes: Vec<&str> = events.iter().map(|e| e.outcome.as_str()).collect();
    assert_eq!(outcomes, ["rejected", "applied"]);
    assert_eq!(events[0].inputs["to"], "done");
}
