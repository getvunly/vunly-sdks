<?php

declare(strict_types=1);

namespace Vunly;

/**
 * A successful answer after decoding, as the services see it.
 *
 * @template-covariant T
 *
 * @internal
 */
final readonly class Reply
{
    /** @param T $value */
    public function __construct(
        public mixed $value,
        public int $status,
        public ?string $requestId,
        public bool $replayed,
        public bool $planOverage,
    ) {
    }
}
