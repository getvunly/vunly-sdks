<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * A customer you bill. `id` is Vunly's own id (cus_...); you never have to store it: every call accepts your
 * `externalId` instead.
 */
final readonly class Customer implements \JsonSerializable
{
    public function __construct(
        public string $id,
        public string $externalId,
        public ?string $name,
        public \DateTimeImmutable $createdAt,
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
            Fields::string($d, 'externalId'),
            Fields::optString($d, 'name'),
            Fields::date(Fields::string($d, 'createdAt')),
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return ['id' => $this->id, 'externalId' => $this->externalId, 'name' => $this->name, 'createdAt' => Fields::iso($this->createdAt)];
    }
}
