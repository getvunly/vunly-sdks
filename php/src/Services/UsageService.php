<?php

declare(strict_types=1);

namespace Vunly\Services;

use Vunly\Client;
use Vunly\Executor;
use Vunly\Models\Fields;
use Vunly\Models\LedgerEntry;
use Vunly\Models\UsageResult;
use Vunly\OpClass;

/** Records usage events. */
final class UsageService
{
    /** @internal */
    public function __construct(private readonly Executor $client)
    {
    }

    /**
     * Records usage once and debits credits. It is final: a LimitReached means the credits were not there when you
     * recorded, even if a check said yes earlier; nothing was debited. A repeat of the same id returns the original
     * result with `replayed === true`.
     *
     * @param string $id       names the unit of work (your request or job id); sent as the Idempotency-Key, 8 to 128
     *                         characters of A-Za-z0-9_.:- . Retry with the SAME id; the API applies it at most once.
     * @param int    $quantity in the meter's own units (for example tokens), not credits: the meter converts it
     */
    public function record(mixed $id, mixed $customer, string $meter, int $quantity): UsageResult
    {
        // `mixed`, not `string`: a null or an int id (a missing request id) is the same InvalidRequest the API would give,
        // not a TypeError, and nothing is sent.
        Client::checkId($id);
        \assert(is_string($id));
        Client::segment($customer, 'customer'); // null, a non-string, "", "." and ".." are refused before anything is sent
        \assert(is_string($customer));
        $reply = $this->client->fetch(
            'POST',
            '/v1/usage-events',
            static fn (array $d): array => [LedgerEntry::fromArray(Fields::object($d, 'event')), Fields::int($d, 'balance')],
            ['balance' => 'int', 'event.id' => 'string', 'event.customerId' => 'string', 'event.kind' => 'string', 'event.credits' => 'int', 'event.createdAt' => 'string'],
            body: ['customerId' => $customer, 'meter' => $meter, 'quantity' => $quantity],
            idem: $id,
            class: OpClass::Keyed,
        );
        [$event, $balance] = $reply->value;

        return new UsageResult($event, $balance, $reply->replayed, $reply->planOverage, $reply->requestId);
    }
}
