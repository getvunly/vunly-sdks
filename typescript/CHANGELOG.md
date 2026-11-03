# Changelog: @vunly/sdk

All notable changes to this package. Format: [Keep a Changelog](https://keepachangelog.com), versions follow [Semantic Versioning](https://semver.org) (before 1.0 a minor version may contain breaking changes, listed under "Breaking"). Sandbox only until a release says otherwise.

## [0.1.1] - 2026-11-02

Changes since 0.1.0 (nothing else changed in the code).

### Added

- `usage.recordBatch(events)`: 1 to 100 events in one request, one result per event, in order (`accepted`, `duplicate`,
  `insufficient_credits`, `too_late`, `meter_not_found`, `plan_limit_reached`, `error`, `unavailable`). `unavailable` events were
  not recorded and are sent again with the same ids.
- `usage.batcher(options?)`: a buffer in front of `recordBatch` (`add`, `flush`, `close`) that sends up to 100 events at a time,
  waits at least the `Retry-After` before sending `unavailable` events again, and holds at most `maxPending` events.
- `PlanLimitReached`: a `402 plan_limit_reached` (the plan's monthly volume is used up) is raised as `PlanLimitReached`, not as
  `LimitReached` and not as `Unavailable`. It is never retried: nothing was recorded and waiting does not clear it before
  `error.details.resetsAt` or a plan change.
- `ProjectFrozen` (`403 project_frozen`) and the `project_frozen` error code: the project is read-only; not a key problem.

### Changed

- A plan stop answered by an older server as a `429` with `details.reason` `plan_included_volume` is classified as
  `PlanLimitReached` too, so it is never waited on.
- `LimitReached` is now only `402 insufficient_credits`: your customer's balance is too low.
- Documentation: the README's batch section, error table and retry rules; `reconciliation.resend` says that a row already
  marked sent is never sent again.

### Retry rules

Unchanged from 0.1.0: network failures, timeouts, `429` (after `Retry-After`, up to 30 seconds) and `500`, `502`, `503`, `504`
are retried; any other `4xx` is not. The one difference is the `402 plan_limit_reached` above, which is now its own error class.

## [0.1.0] - 2026-10-30

First version.
