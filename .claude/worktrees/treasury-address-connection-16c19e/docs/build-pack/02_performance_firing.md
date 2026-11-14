# Milestone 3: Performance, firing, and owner notifications

Read 00_shared_context.md first. Its rules apply throughout this milestone.

## Metrics

Compute these per worker from task records, not from LLM summaries:

- Completion rate
- On-time rate
- Cost per completed task
- Quality score

## Quality score

- Prefer objective checks: tests passing or acceptance criteria met.
- LLM-judged quality must use a different model than the worker being scored.
- Send a random 10% of reviews to an owner spot-check queue and report the disagreement rate.

## Review schedule

- Run a review every N completed tasks or at the end of each review period, whichever comes first.
- Review periods and thresholds live in config.
- Store every review.

## Firing rule

- A firing requires both conditions: at least 10 completed tasks and at least 14 days of data, and objective metrics below threshold for two consecutive review periods.
- An LLM quality score alone can never trigger a firing.
- The firing executes autonomously. No human approval is required, but the owner notification in this milestone is mandatory.

## Firing execution

- Reassign open tasks through the planner's routing. If no worker can take a task, create a hiring request.
- Revoke the worker's tool access.
- Record the reason, the metrics, and a final review in the audit log.
- Set the worker's status to fired.
- Generate an owner notification within one minute. Send it by email through our SMTP server and add it to the dashboard feed. The notification contains the reason, the metrics, and an audit link. Build the notification queue here. The dashboard UI comes in milestone 7.

## Tests

- A worker with 10 completed tasks over 3 days does not fire. A worker with 14 days of data and 5 completed tasks does not fire.
- Two consecutive below-threshold review periods fire the worker. One below-threshold period does not.
- A low LLM-only quality score does not fire a worker.
- Reassignment works. Tasks no one can take create hiring requests.
- The owner notification is generated with the reason, metrics, and audit link, within the time limit.
- Spot-check sampling selects about 10% of reviews over a large run, and the disagreement rate is reported.
