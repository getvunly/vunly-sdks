<?php

declare(strict_types=1);

namespace Vunly\Services;

use Vunly\Client;
use Vunly\Executor;
use Vunly\Models\Balance;
use Vunly\Models\Fields;
use Vunly\Models\LedgerEntry;
use Vunly\Models\Page;
use Vunly\OpClass;
use Vunly\Unavailable;

/** Grants credits and reads balances and the ledger. */
final class CreditsService
{
    private const ENTRY = ['id' => 'string', 'customerId' => 'string', 'kind' => 'string', 'credits' => 'int', 'createdAt' => 'string'];

    /** @internal */
    public function __construct(private readonly Executor $client)
    {
    }

    /**
     * Adds credits (a sandbox top-up). `id` is the Idempotency-Key, 8 to 128 characters of A-Za-z0-9_.:- : the same id
     * grants once, and a repeat returns the original entry.
     */
    public function grant(mixed $id, mixed $customer, int $credits, ?string $reason = null): LedgerEntry
    {
        Client::checkId($id); // `mixed`: a null id is an InvalidRequest, not a TypeError
        \assert(is_string($id));
        $body = ['credits' => $credits];
        if ($reason !== null && $reason !== '') {
            $body['reason'] = $reason;
        }

        return $this->client->fetch(
            'POST',
            '/v1/customers/' . Client::segment($customer, 'customer') . '/credit-grants',
            LedgerEntry::fromArray(...),
            self::ENTRY,
            body: $body,
            idem: $id,
            class: OpClass::Keyed,
        )->value;
    }

    /** A customer's credits. */
    public function balance(mixed $customer): Balance
    {
        return $this->client->fetch(
            'GET',
            '/v1/customers/' . Client::segment($customer, 'customer') . '/balance',
            Balance::fromArray(...),
            ['credits' => 'int', 'consumed' => 'int', 'granted' => 'int'],
        )->value;
    }

    /**
     * One page of a customer's ledger entries, newest first.
     *
     * @return Page<LedgerEntry>
     */
    public function ledger(mixed $customer, ?int $limit = null, ?string $cursor = null): Page
    {
        return $this->client->fetch(
            'GET',
            '/v1/customers/' . Client::segment($customer, 'customer') . '/ledger',
            static fn (array $d): Page => new Page(array_map(LedgerEntry::fromArray(...), Fields::rows($d, 'data')), Fields::optString($d, 'nextCursor')),
            ['data' => 'array', 'nextCursor' => 'string?'],
            query: ['limit' => $limit, 'cursor' => $cursor],
        )->value;
    }

    /**
     * A customer's whole ledger, one page at a time, only as you consume it.
     *
     * @return \Generator<int, LedgerEntry>
     */
    public function allLedger(mixed $customer, ?int $limit = null): \Generator
    {
        $cursor = null;
        $seen = [];
        $empty = 0;
        do {
            $page = $this->ledger($customer, $limit, $cursor);
            yield from $page->data;
            $cursor = $page->nextCursor;
            $empty = $page->data === [] ? $empty + 1 : 0;
            if ($cursor !== null && $empty >= Client::MAX_EMPTY_PAGES) {
                throw new Unavailable('the API returned too many empty pages in a row; paging stopped', 'invalid_response', 200);
            }
            if ($cursor !== null) {
                if (isset($seen[$cursor])) {
                    throw new Unavailable('the API returned a page cursor it had already returned; paging stopped', 'invalid_response', 200);
                }
                $seen[$cursor] = true;
            }
        } while ($cursor !== null);
    }
}
