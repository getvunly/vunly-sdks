<?php

declare(strict_types=1);

namespace Vunly;

/** A 402: the customer has too few credits. Nothing was debited. Do not run the work. Not retryable. */
final class LimitReached extends VunlyError
{
    /** The customer's balance, when the API sent it with the 402. */
    public function balance(): ?int
    {
        $v = $this->details['balance'] ?? null;

        return is_int($v) ? $v : null;
    }

    /** The cost the refused request needed, when the API sent it. */
    public function required(): ?int
    {
        $v = $this->details['required'] ?? null;

        return is_int($v) ? $v : null;
    }
}
