<?php

declare(strict_types=1);

namespace Vunly\Models;

/** One line of a customer's ledger. `credits` is signed: positive for a grant, negative for a debit. */
final readonly class LedgerEntry implements \JsonSerializable
{
    public function __construct(
        public string $id,
        public string $customerId,
        /** "grant" or "debit"; any other value the API may add is kept as it came. */
        public string $kind,
        public int $credits,
        public ?string $meter,
        public ?int $quantity,
        public ?string $reason,
        public ?string $idempotencyKey,
        public \DateTimeImmutable $createdAt,
    ) {
    }

    public function knownKind(): ?LedgerKind
    {
        return LedgerKind::tryFrom($this->kind);
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
            Fields::string($d, 'kind'),
            Fields::int($d, 'credits'),
            Fields::optString($d, 'meter'),
            Fields::optInt($d, 'quantity'),
            Fields::optString($d, 'reason'),
            Fields::optString($d, 'idempotencyKey'),
            Fields::date(Fields::string($d, 'createdAt')),
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'id' => $this->id,
            'customerId' => $this->customerId,
            'kind' => $this->kind,
            'credits' => $this->credits,
            'meter' => $this->meter,
            'quantity' => $this->quantity,
            'reason' => $this->reason,
            'idempotencyKey' => $this->idempotencyKey,
            'createdAt' => Fields::iso($this->createdAt),
        ];
    }
}
