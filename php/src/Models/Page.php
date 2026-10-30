<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * One page of a list. `nextCursor` is opaque: pass it back as the `cursor` of the next call, never parse or build one.
 * It is null on the last page.
 *
 * @template T
 */
final readonly class Page implements \JsonSerializable
{
    /** @param list<T> $data */
    public function __construct(
        public array $data,
        public ?string $nextCursor,
    ) {
    }

    public function hasMore(): bool
    {
        return $this->nextCursor !== null;
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return ['data' => $this->data, 'nextCursor' => $this->nextCursor];
    }
}
