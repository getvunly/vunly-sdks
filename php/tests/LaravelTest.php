<?php

declare(strict_types=1);

namespace Vunly\Tests;

use Illuminate\Http\Request;
use Illuminate\Support\ServiceProvider;
use Orchestra\Testbench\TestCase;
use Vunly\Client;
use Vunly\Laravel\Facades\Vunly;
use Vunly\Laravel\Http\Middleware\MeterUsage;
use Vunly\Laravel\VunlyServiceProvider;
use Vunly\Tests\Support\FakeServer;

final class LaravelTest extends TestCase
{
    private const KEY = 'vk_test_synthetic_php_laravel_0000001';

    /** @var list<FakeServer> */
    private array $servers = [];
    private int $worked = 0;

    protected function getPackageProviders($app): array
    {
        return [VunlyServiceProvider::class];
    }

    protected function defineEnvironment($app): void
    {
        $app['config']->set('vunly.api_key', self::KEY);
        $app['config']->set('vunly.base_url', 'http://127.0.0.1:1');
        $app['config']->set('vunly.max_retries', 0);
        $app['config']->set('vunly.timeout', 3);
    }

    protected function tearDown(): void
    {
        MeterUsage::resolveCustomerUsing(null);
        foreach ($this->servers as $s) {
            $s->stop();
        }
        parent::tearDown();
    }

    /** @param list<array<string, mixed>> $script */
    private function serve(array $script): FakeServer
    {
        $server = $this->servers[] = new FakeServer($script);
        $this->app['config']->set('vunly.base_url', $server->url);
        $this->app->forgetInstance(Client::class);

        return $server;
    }

    private function route(int $status = 200, ?int $quantity = null): void
    {
        $this->app['router']->post('/generate', function (Request $request) use ($status, $quantity) {
            $this->worked++;
            if ($quantity !== null) {
                $request->attributes->set('vunly.quantity', $quantity);
            }

            return response()->json(['output' => 'ok'], $status);
        })->middleware('vunly:generate,tokens,100');
        MeterUsage::resolveCustomerUsing(static fn (Request $r): ?string => $r->header('X-Customer'));
    }

    /** @return array{respond: array<string, mixed>} */
    private static function reply(int $status, mixed $body, array $headers = []): array
    {
        return ['respond' => ['status' => $status, 'json' => $body, 'headers' => $headers]];
    }

    private static function allowed(bool $allowed = true): array
    {
        return self::reply(200, ['allowed' => $allowed, 'reason' => $allowed ? 'ok' : 'no_active_plan', 'remaining' => 50, 'required' => 1, 'plan' => 'starter']);
    }

    private static function recorded(int $balance = 40, array $headers = []): array
    {
        return self::reply(200, ['event' => ['id' => 'led_1', 'customerId' => 'cus_1', 'kind' => 'debit', 'credits' => -10, 'createdAt' => '2026-01-01T00:00:00Z'], 'balance' => $balance], $headers);
    }

    // ---- service provider, facade, config ----------------------------------------------------------------------------

    public function testTheProviderSharesOneClientAndAliasesIt(): void
    {
        $client = $this->app->make(Client::class);
        $this->assertSame($client, $this->app->make(Client::class));
        $this->assertSame($client, $this->app->make('vunly'));
        $this->assertStringNotContainsString(self::KEY, (string) $client);
        $published = ServiceProvider::pathsToPublish(VunlyServiceProvider::class, 'vunly-config');
        $this->assertCount(1, $published);
        $this->assertFileExists((string) array_key_first($published));
    }

    public function testTheFacadeForwardsToTheSharedClient(): void
    {
        $server = $this->serve([self::reply(200, ['credits' => 90, 'consumed' => 10, 'granted' => 100])]);
        $this->assertSame(90, Vunly::credits()->balance('acme')->remaining);
        $this->assertSame('Bearer ' . self::KEY, $server->requests()[0]['headers']['authorization']);
    }

    public function testMissingSettingsFailWhenTheClientIsResolvedAndNameTheSetting(): void
    {
        foreach (['vunly.api_key' => 'VUNLY_API_KEY', 'vunly.base_url' => 'VUNLY_API_URL'] as $setting => $env) {
            $this->app['config']->set($setting, null);
            $this->app->forgetInstance(Client::class);
            try {
                $this->app->make(Client::class);
                $this->fail("expected an exception for $setting");
            } catch (\InvalidArgumentException $e) {
                $this->assertStringContainsString($env, $e->getMessage());
                $this->assertStringNotContainsString(self::KEY, $e->getMessage());
            }
            $this->app['config']->set('vunly.api_key', self::KEY);
            $this->app['config']->set('vunly.base_url', 'http://127.0.0.1:1');
        }
    }

    public function testComposerDeclaresAutoDiscovery(): void
    {
        /** @var array{extra: array{laravel: array{providers: list<string>, aliases: array<string, string>}}} $composer */
        $composer = json_decode((string) file_get_contents(__DIR__ . '/../composer.json'), true, 32, JSON_THROW_ON_ERROR);
        $this->assertSame([VunlyServiceProvider::class], $composer['extra']['laravel']['providers']);
        $this->assertSame(['Vunly' => Vunly::class], $composer['extra']['laravel']['aliases']);
        $this->assertTrue(class_exists(VunlyServiceProvider::class));
    }

    // ---- middleware: check, then record --------------------------------------------------------------------------------

    public function testCheckThenWorkThenRecordWithTheRequestIdAsTheIdempotencyKey(): void
    {
        $server = $this->serve([self::allowed(), self::recorded(40)]);
        $this->route();

        $res = $this->postJson('/generate', [], ['X-Customer' => 'acme', 'X-Request-Id' => 'req_laravel_0001']);

        $res->assertOk()->assertHeader('X-Vunly-Credits-Remaining', '40')->assertHeader('X-Vunly-Replayed', 'false');
        $this->assertSame(1, $this->worked);
        $reqs = $server->requests();
        $this->assertSame(['POST /v1/entitlements/check', 'POST /v1/usage-events'], array_map(static fn (array $r): string => "{$r['method']} {$r['path']}", $reqs));
        // the caller's X-Request-Id is ignored by default: the id is made on the server
        $this->assertMatchesRegularExpression('/^req_[0-9a-f]{32}$/', $reqs[1]['headers']['idempotency-key']);
        $this->assertSame(['customerId' => 'acme', 'feature' => 'generate', 'estimate' => ['meter' => 'tokens', 'quantity' => 100]], json_decode($reqs[0]['body'], true));
        $this->assertSame(['customerId' => 'acme', 'meter' => 'tokens', 'quantity' => 100], json_decode($reqs[1]['body'], true));
    }

    public function testAReplayIsReportedAndTheControllersQuantityIsRecorded(): void
    {
        $server = $this->serve([self::allowed(), self::recorded(40, ['Idempotent-Replayed' => 'true'])]);
        $this->route(quantity: 1050);

        $this->postJson('/generate', [], ['X-Customer' => 'acme', 'X-Request-Id' => 'req_laravel_0002'])
            ->assertOk()->assertHeader('X-Vunly-Replayed', 'true');
        $this->assertSame(1050, json_decode($server->requests()[1]['body'], true)['quantity']);
    }

    public function testARepeatedRequestIdHeaderDoesNotRunBillableWorkForFreeByDefault(): void
    {
        // The API charges nothing for an id it has seen ("replayed"). If the header were the idempotency id, repeating it would
        // run the work again at no cost. By default the header is ignored: every request gets its own id, so both are charged.
        $server = $this->serve([self::allowed(), self::recorded(40), self::allowed(), self::recorded(39)]);
        $this->route();
        $h = ['X-Customer' => 'acme', 'X-Request-Id' => 'req_laravel_replay_1'];
        $this->postJson('/generate', [], $h)->assertOk();
        $this->postJson('/generate', [], $h)->assertOk();
        $this->assertSame(2, $this->worked);
        $keys = array_values(array_filter(array_map(static fn (array $r): ?string => $r['headers']['idempotency-key'] ?? null, $server->requests())));
        $this->assertCount(2, $keys);
        $this->assertNotSame($keys[0], $keys[1], 'two requests must be recorded under two ids');
        $this->assertNotContains('req_laravel_replay_1', $keys);
    }

    public function testTrustingTheHeaderIsAnOptInBoundToCustomerFeatureAndMeter(): void
    {
        $this->app['config']->set('vunly.trust_request_id', true);
        $server = $this->serve([self::allowed(), self::recorded(), self::allowed(), self::recorded(), self::allowed(), self::recorded()]);
        $this->route();
        $this->postJson('/generate', [], ['X-Customer' => 'acme', 'X-Request-Id' => 'req_laravel_trust_1'])->assertOk();
        $this->postJson('/generate', [], ['X-Customer' => 'acme', 'X-Request-Id' => 'req_laravel_trust_1'])->assertOk();
        $this->postJson('/generate', [], ['X-Customer' => 'other', 'X-Request-Id' => 'req_laravel_trust_1'])->assertOk();
        $keys = array_values(array_filter(array_map(static fn (array $r): ?string => $r['headers']['idempotency-key'] ?? null, $server->requests())));
        $this->assertSame($keys[0], $keys[1], 'the same header, the same customer: one id (a replay)');
        $this->assertNotSame($keys[0], $keys[2], 'another customer must not share the id');
        $this->assertMatchesRegularExpression('/^req_[0-9a-f]{40}$/', $keys[0]);
    }

    public function testWithoutARequestIdOneIsMadeOncePerRequest(): void
    {
        $server = $this->serve([self::allowed(), self::recorded()]);
        $this->route();
        $this->postJson('/generate', [], ['X-Customer' => 'acme'])->assertOk();
        $this->assertMatchesRegularExpression('/^req_[0-9a-f]{32}$/', $server->requests()[1]['headers']['idempotency-key']);
    }

    public function testANoMeansTheControllerDoesNotRun(): void
    {
        $server = $this->serve([self::allowed(false)]);
        $this->route();
        $this->postJson('/generate', [], ['X-Customer' => 'acme'])
            ->assertForbidden()->assertJson(['error' => 'not_allowed', 'reason' => 'no_active_plan']);
        $this->assertSame(0, $this->worked);
        $this->assertCount(1, $server->requests());
    }

    public function testAFailedCheckFailsClosed(): void
    {
        $server = $this->serve([['drop' => true]]);
        $this->route();
        $this->postJson('/generate', [], ['X-Customer' => 'acme'])
            ->assertStatus(503)->assertJson(['error' => 'billing_unavailable']);
        $this->assertSame(0, $this->worked);
        $this->assertCount(1, $server->requests());
    }

    public function testARefusedKeyIsAServerSideSetupProblemNotTheCallers(): void
    {
        $this->serve([self::reply(401, ['error' => ['code' => 'unauthenticated', 'message' => 'no']])]);
        $this->route();
        $res = $this->postJson('/generate', [], ['X-Customer' => 'acme'])->assertStatus(500)->assertJson(['error' => 'billing_misconfigured']);
        $this->assertStringNotContainsString(self::KEY, $res->getContent() ?: '');
        $this->assertSame(0, $this->worked);
    }

    public function testNoCreditsAtRecordTimeIsA402(): void
    {
        $this->serve([self::allowed(), self::reply(402, ['error' => ['code' => 'insufficient_credits', 'message' => 'no', 'requestId' => 'req_402']])]);
        $this->route();
        $this->postJson('/generate', [], ['X-Customer' => 'acme'])
            ->assertStatus(402)->assertJson(['error' => 'insufficient_credits', 'requestId' => 'req_402']);
    }

    public function testRecordUnavailableIsA503TheCallerMayRepeatWithTheSameId(): void
    {
        $this->serve([self::allowed(), ['drop' => true]]);
        $this->route();
        $this->postJson('/generate', [], ['X-Customer' => 'acme', 'X-Request-Id' => 'req_laravel_0003'])
            ->assertStatus(503)->assertJson(['error' => 'billing_unavailable']);
    }

    public function testAnErrorResponseFromTheControllerRecordsNothing(): void
    {
        $server = $this->serve([self::allowed()]);
        $this->route(status: 500);
        $this->postJson('/generate', [], ['X-Customer' => 'acme'])->assertStatus(500);
        $this->assertSame(1, $this->worked);
        $this->assertCount(1, $server->requests(), 'only the check was made');
    }

    public function testAnUnresolvedCustomerIsA401AndNothingIsSent(): void
    {
        $server = $this->serve([]);
        $this->route();
        $this->postJson('/generate')->assertUnauthorized()->assertJson(['error' => 'customer_unresolved']);
        $this->assertSame([], $server->requests());
    }

    public function testABadEstimateIsAProgrammingError(): void
    {
        $this->serve([]);
        $this->app['router']->post('/bad', static fn () => 'x')->middleware('vunly:generate,tokens,lots');
        MeterUsage::resolveCustomerUsing(static fn (): string => 'acme');
        $this->withoutExceptionHandling();
        $this->expectException(\InvalidArgumentException::class);
        $this->postJson('/bad');
    }
}
