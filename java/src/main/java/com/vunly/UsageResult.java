package com.vunly;

/**
 * The outcome of {@link UsageApi#record}.
 *
 * @param remaining the credits left after this event
 * @param replayed true when this id had already been recorded: nothing was debited again. A replay is a success.
 * @param planOverage true when the project is on a paid plan and this event is past the plan's included monthly
 *     volume. Paid plans keep recording; this lets you notice. Always false for a replay.
 * @param requestId the API's Request-Id
 */
public record UsageResult(LedgerEntry event, long remaining, boolean replayed, boolean planOverage, String requestId) {}
