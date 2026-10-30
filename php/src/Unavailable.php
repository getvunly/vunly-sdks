<?php

declare(strict_types=1);

namespace Vunly;

/**
 * No answer (network, timeout, a body that broke), a 429, a 5xx that survived the SDK's own retries, or a 2xx whose body
 * is malformed (code "invalid_response"). The outcome may be unknown: trying again later with the SAME id is safe.
 *
 * A 429 whose details["reason"] is "plan_included_volume" is the plan's monthly volume stop, not a rate limit: it is never
 * retried automatically and {@see isRetryable()} is false for it.
 */
final class Unavailable extends VunlyError
{
    public function isPlanStop(): bool
    {
        return $this->status === 429 && ($this->details['reason'] ?? null) === 'plan_included_volume';
    }

    public function isRetryable(): bool
    {
        // a redirect means the configured address is wrong: trying again later does not help
        return !$this->isPlanStop() && $this->errorCode !== 'unexpected_redirect';
    }
}
