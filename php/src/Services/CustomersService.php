<?php

declare(strict_types=1);

namespace Vunly\Services;

use Vunly\Client;
use Vunly\Executor;
use Vunly\Models\Customer;
use Vunly\Models\Fields;
use Vunly\Models\Page;
use Vunly\OpClass;
use Vunly\Unavailable;

/** Creates and reads the customers you bill. */
final class CustomersService
{
    private const SHAPE = ['id' => 'string', 'externalId' => 'string', 'createdAt' => 'string'];

    /** @internal */
    public function __construct(private readonly Executor $client)
    {
    }

    /**
     * Creates the customer or returns the existing one for this id (your own id). Asking twice leaves one customer.
     */
    public function upsert(mixed $id, ?string $name = null): Customer
    {
        Client::segment($id, 'id'); // refuses null, a non-string, "", "." and ".." before anything is sent
        \assert(is_string($id));
        $body = ['externalId' => $id];
        if ($name !== null && $name !== '') {
            $body['name'] = $name;
        }

        return $this->client->fetch('POST', '/v1/customers', Customer::fromArray(...), self::SHAPE, body: $body, class: OpClass::Natural)->value;
    }

    /** One customer, by your id or Vunly's. A customer that does not exist is an InvalidRequest with code "not_found". */
    public function get(mixed $customer): Customer
    {
        return $this->client->fetch('GET', '/v1/customers/' . Client::segment($customer, 'customer'), Customer::fromArray(...), self::SHAPE)->value;
    }

    /**
     * One page of customers, newest first. `limit` is 1 to 100 (null: the API's default).
     *
     * @return Page<Customer>
     */
    public function list(?int $limit = null, ?string $cursor = null): Page
    {
        return $this->client->fetch(
            'GET',
            '/v1/customers',
            static fn (array $d): Page => new Page(array_map(Customer::fromArray(...), Fields::rows($d, 'data')), Fields::optString($d, 'nextCursor')),
            ['data' => 'array', 'nextCursor' => 'string?'],
            query: ['limit' => $limit, 'cursor' => $cursor],
        )->value;
    }

    /**
     * Every customer, fetching one page at a time and only as you consume them. Stop iterating to stop fetching.
     *
     * @return \Generator<int, Customer>
     */
    public function all(?int $limit = null): \Generator
    {
        $cursor = null;
        $seen = [];
        $empty = 0;
        do {
            $page = $this->list($limit, $cursor);
            yield from $page->data;
            $cursor = $page->nextCursor;
            // pages with no records that keep promising more, each with a fresh cursor, would loop for as long as the server likes
            $empty = $page->data === [] ? $empty + 1 : 0;
            if ($cursor !== null && $empty >= Client::MAX_EMPTY_PAGES) {
                throw new Unavailable('the API returned too many empty pages in a row; paging stopped', 'invalid_response', 200);
            }
            if ($cursor !== null) {
                // a server that keeps returning a cursor it already returned would make this loop fetch forever
                if (isset($seen[$cursor])) {
                    throw new Unavailable('the API returned a page cursor it had already returned; paging stopped', 'invalid_response', 200);
                }
                $seen[$cursor] = true;
            }
        } while ($cursor !== null);
    }
}
