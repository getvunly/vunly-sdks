<?php

declare(strict_types=1);

namespace Vunly;

use Vunly\Models\Fields;

/** A verified webhook delivery. */
final readonly class WebhookEvent
{
    /** @param array<string, mixed>|list<mixed> $data */
    public function __construct(
        public string $id,
        public string $type,
        public ?\DateTimeImmutable $createdAt,
        public array $data,
    ) {
    }

    /** @param array<string, mixed> $d */
    public static function fromArray(array $d): self
    {
        $data = $d['data'] ?? [];

        return new self(
            Fields::string($d, 'id'),
            Fields::string($d, 'type'),
            Fields::optDate($d, 'createdAt'),
            is_array($data) ? $data : [],
        );
    }
}
