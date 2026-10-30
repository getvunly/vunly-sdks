<?php

declare(strict_types=1);

namespace Vunly\Models;

/** The outcome of {@see \Vunly\Services\UsageService::record()}. */
final readonly class UsageResult implements \JsonSerializable
{
    public function __construct(
        public LedgerEntry $event,
        /** The credits left after this event. */
        public int $remaining,
        /** True when this id had already been recorded: nothing was debited again. A replay is a success. */
        public bool $replayed,
        /** True when the project is on a paid plan and this event is past the plan's included monthly volume. Always false for a replay. */
        public bool $planOverage,
        public ?string $requestId,
    ) {
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'event' => $this->event,
            'remaining' => $this->remaining,
            'replayed' => $this->replayed,
            'planOverage' => $this->planOverage,
            'requestId' => $this->requestId,
        ];
    }
}
