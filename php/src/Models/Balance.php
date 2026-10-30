<?php

declare(strict_types=1);

namespace Vunly\Models;

/** A customer's credits. remaining = granted - consumed and is never negative. */
final readonly class Balance implements \JsonSerializable
{
    public function __construct(
        public int $remaining,
        public int $consumed,
        public int $granted,
    ) {
    }

    /**
     * @param array<string, mixed> $d
     *
     * @internal
     */
    public static function fromArray(array $d): self
    {
        return new self(Fields::int($d, 'credits'), Fields::int($d, 'consumed'), Fields::int($d, 'granted'));
    }

    /** @return array<string, int> */
    public function jsonSerialize(): array
    {
        return ['remaining' => $this->remaining, 'consumed' => $this->consumed, 'granted' => $this->granted];
    }
}
