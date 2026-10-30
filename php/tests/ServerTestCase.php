<?php

declare(strict_types=1);

namespace Vunly\Tests;

use PHPUnit\Framework\TestCase;
use Vunly\Client;
use Vunly\Tests\Support\FakeServer;

abstract class ServerTestCase extends TestCase
{
    /** A credential-shaped value that is marked synthetic, as the repository audit requires. */
    protected const KEY = 'vk_test_synthetic_php_00000000000001';

    /** @var list<FakeServer> */
    private array $servers = [];

    /** @var list<float> */
    protected array $sleeps = [];

    protected function tearDown(): void
    {
        foreach ($this->servers as $s) {
            $s->stop();
        }
        $this->servers = [];
        $this->sleeps = [];
    }

    /** @param list<array<string, mixed>> $script */
    protected function serve(array $script): FakeServer
    {
        return $this->servers[] = new FakeServer($script);
    }

    /** A client for the server, whose retry waits are recorded in $this->sleeps instead of slept. */
    protected function client(FakeServer $server, int $maxRetries = 2, float $timeout = 5.0): Client
    {
        return new Client(
            apiKey: self::KEY,
            baseUrl: $server->url,
            timeout: $timeout,
            maxRetries: $maxRetries,
            sleeper: function (float $s): void {
                $this->sleeps[] = $s;
            },
        );
    }

    /** @return array{respond: array<string, mixed>} */
    protected static function json(int $status, mixed $body, array $headers = []): array
    {
        return ['respond' => ['status' => $status, 'json' => $body, 'headers' => $headers]];
    }

    /** @return array{respond: array<string, mixed>} */
    protected static function error(int $status, string $code, array $details = [], array $headers = []): array
    {
        return self::json($status, ['error' => ['code' => $code, 'message' => "m $code", 'requestId' => 'req_x', 'details' => $details === [] ? null : $details]], $headers);
    }

    protected static function entry(int $credits = 10): array
    {
        return ['id' => 'led_1', 'customerId' => 'cus_1', 'kind' => 'debit', 'credits' => $credits, 'createdAt' => '2026-01-01T00:00:00.000Z'];
    }

    protected static function usage(int $balance = 90): array
    {
        return ['event' => self::entry(-10), 'balance' => $balance];
    }

    protected static function balance(): array
    {
        return ['credits' => 90, 'consumed' => 10, 'granted' => 100];
    }
}
