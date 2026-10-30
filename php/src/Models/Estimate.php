<?php

declare(strict_types=1);

namespace Vunly\Models;

/** The usage a check should price in. */
final readonly class Estimate
{
    public function __construct(
        public string $meter,
        public int $quantity,
    ) {
    }
}
