<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * The project's plan and this month's recorded usage.
 *
 * `unconfirmedLimits` is a working hypothesis, not approved and not a guarantee: it carries `confirmed === false`. Do not
 * show it to your customers as a quota, a price or a commitment.
 */
final readonly class PlanUsage implements \JsonSerializable
{
    /**
     * @param array{key: string, name: ?string}               $plan
     * @param array{start: \DateTimeImmutable, end: ?\DateTimeImmutable} $period
     * @param array{confirmed: bool, note: ?string, includedMonthlyEvents: ?int, activityRetentionDays: ?int} $unconfirmedLimits
     */
    public function __construct(
        public array $plan,
        public array $period,
        public int $events,
        public array $unconfirmedLimits,
    ) {
    }

    /**
     * @param array<string, mixed> $d
     *
     * @internal
     */
    public static function fromArray(array $d): self
    {
        $plan = Fields::object($d, 'plan');
        $period = Fields::object($d, 'period');
        $usage = Fields::object($d, 'usage');
        $limits = Fields::object($d, 'unconfirmedLimits');

        return new self(
            ['key' => Fields::string($plan, 'key'), 'name' => Fields::optString($plan, 'name')],
            ['start' => Fields::date(Fields::string($period, 'start')), 'end' => Fields::optDate($period, 'end')],
            Fields::int($usage, 'events'),
            [
                'confirmed' => Fields::bool($limits, 'confirmed'),
                'note' => Fields::optString($limits, 'note'),
                'includedMonthlyEvents' => Fields::optInt($limits, 'includedMonthlyEvents'),
                'activityRetentionDays' => Fields::optInt($limits, 'activityRetentionDays'),
            ],
        );
    }

    /** @return array<string, mixed> */
    public function jsonSerialize(): array
    {
        return [
            'plan' => $this->plan,
            'period' => ['start' => Fields::iso($this->period['start']), 'end' => Fields::iso($this->period['end'])],
            'usage' => ['events' => $this->events],
            'unconfirmedLimits' => $this->unconfirmedLimits,
        ];
    }
}
