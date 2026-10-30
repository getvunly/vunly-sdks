<?php

declare(strict_types=1);

namespace Vunly;

/**
 * A webhook delivery that did not verify. `reason` is one of "missing_header", "malformed_header", "stale_timestamp",
 * "no_matching_signature" or "invalid_payload". Answer with a 400 and do not process the payload.
 */
final class WebhookError extends \RuntimeException
{
    public function __construct(public readonly string $reason)
    {
        parent::__construct("Vunly: webhook verification failed ($reason)");
    }
}
