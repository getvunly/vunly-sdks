<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * The answer of an entitlement check. It is advisory and reserves nothing: the authoritative limit is the debit made by
 * `usage->record()`. Unless `allowed` is true, do not run the work.
 */
final readonly class Entitlement implements \JsonSerializable
{
    public function __construct(
        public bool $allowed,
        /** "ok", "no_active_plan", "feature_not_in_plan" or "insufficient_credits"; a reason the API adds later is kept as it came. */
        public string $reason,
        public int $remaining,
        public int $required,
        public ?string $plan,
    ) {
    }

    public function knownReason(): ?EntitlementReason
    {
        return EntitlementReason::tryFrom($this->reason);
    }

    /**
     * @param array<string, mixed> $d
     *
     * @internal
     */
    public static function fromArray(array $d): self
    {
        return new self(
            Fields::bool($d, 'allowed'),
            Fields::string($d, 'reason'),
            Fields::int($d, 'remaining'),
            Fields::int($d, 'required'),
            Fields::optString($d, 'plan'),
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'allowed' => $this->allowed,
            'reason' => $this->reason,
            'remaining' => $this->remaining,
            'required' => $this->required,
            'plan' => $this->plan,
        ];
    }
}
