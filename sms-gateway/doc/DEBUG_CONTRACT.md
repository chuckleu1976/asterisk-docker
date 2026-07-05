# Debug Contract (No Claim Without Proof)

Use this contract for every bug fix in this repo.

## Required sequence

1. Reproduce the bug with concrete evidence.
2. Identify root cause in code or data.
3. Patch the minimal fix.
4. Run verification gate: `./tools/verify-fix.sh`.
5. Re-check evidence after patch.
6. Only then report "fixed".

## Required evidence in responses

1. Root cause statement.
2. Evidence from at least one of: DB query, API payload, or deterministic test.
3. Build/test output summary.
4. Residual risk (if any).

## Verification gate

Run this from `sms-gateway/`:

```bash
./tools/verify-fix.sh
```

It enforces:

1. `cargo check -q`
2. `cargo test -q db_direction_tests`
3. `frontend` production build
4. SQLite sanity check for unresolved blank inbound sender rows

## Release checklist for chat/inbox issues

1. Sender resolution is stable across reload.
2. Left list grouping and right detail panel show same thread.
3. Mark-read persists after leave/re-enter.
4. Empty/legacy contact_id rows are either backfilled or deterministically inferred.
