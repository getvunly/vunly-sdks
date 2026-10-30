<?php

declare(strict_types=1);

namespace Vunly\Models;

/** A customer's plan. `status` is "active" or "canceled" (any later value is kept as it came). */
final readonly class Subscription implements \JsonSerializable
{
    public function __construct(
        public string $id,
        public string $customerId,
        public Plan $plan,
        public string $status,
        public ?\DateTimeImmutable $periodStart,
        public ?\DateTimeImmutable $periodEnd,
        public ?\DateTimeImmutable $createdAt,
    ) {
    }

    /**
     * @param array<string, mixed> $d
     *
     * @internal
     */
    public static function fromArray(array $d): self
    {
        return new self(
            Fields::string($d, 'id'),
            Fields::string($d, 'customerId'),
            Plan::fromArray(Fields::object($d, 'plan')),
            Fields::string($d, 'status'),
            Fields::optDate($d, 'periodStart'),
            Fields::optDate($d, 'periodEnd'),
            Fields::optDate($d, 'createdAt'),
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'id' => $this->id,
            'customerId' => $this->customerId,
            'plan' => $this->plan,
            'status' => $this->status,
            'periodStart' => Fields::iso($this->periodStart),
            'periodEnd' => Fields::iso($this->periodEnd),
            'createdAt' => Fields::iso($this->createdAt),
        ];
    }
}
