# Current product backlog

This is an active backlog for the current Home API architecture. The material in
`_bmad-output/` remains historical and is not the implementation queue.

## Continuous automatic one-way sync

**Status:** deferred, explicitly requested for follow-up after one-shot sync.

Offer an opt-in Home policy that automatically schedules eligible completed
seedbox files as fresh inventory generations arrive. Reuse one-shot sync's
eligibility, exact-file authorization, Job deduplication, and library safeguards.

Acceptance boundaries:
- Disabled by default; merely running the Home services does not enable it.
- Explicit enable, pause, resume and inspection through the Home API and CLI/TUI.
- No manual per-title Want duplication when the policy is enabled.
- No seedbox/local deletion, implicit overwrite, auto-approval, or grabber HTTP
  outside the existing seedbox/gateway boundary.
- Respect committed inventory freshness, disk/concurrency budgets and existing
  Jobs; recover safely after restart without replaying completed work.
- Define retention of completed request records and behavior when policy, roots,
  library paths, or eligibility rules change.
- Add policy/controller tests and local end-to-end verification before release.

Do not implement or enable this policy as part of the one-shot sync feature.
