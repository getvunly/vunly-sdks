<?php

declare(strict_types=1);

namespace Vunly\Models;

/** A plan of the project. */
final readonly class Plan implements \JsonSerializable
{
    /** @param list<string> $features */
    public function __construct(
        public string $key,
        public string $name,
        public ?int $includedCredits,
        public ?string $meter,
        public array $features,
        public ?\DateTimeImmutable $updatedAt,
    ) {
    }

    /**
     * @param array<string, mixed> $d
     *
     * @internal
     */
    public static function fromArray(array $d): self
    {
        $features = [];
        foreach (is_array($d['features'] ?? null) ? $d['features'] : [] as $f) {
            if (is_string($f)) {
                $features[] = $f;
            }
        }

        return new self(
            Fields::string($d, 'key'),
            Fields::string($d, 'name'),
            Fields::optInt($d, 'includedCredits'),
            Fields::optString($d, 'meter'),
            $features,
            Fields::optDate($d, 'updatedAt'),
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'key' => $this->key,
            'name' => $this->name,
            'includedCredits' => $this->includedCredits,
            'meter' => $this->meter,
            'features' => $this->features,
            'updatedAt' => Fields::iso($this->updatedAt),
        ];
    }
}
