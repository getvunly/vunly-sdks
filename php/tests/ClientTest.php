<?php

declare(strict_types=1);

namespace Vunly\Tests;

use PHPUnit\Framework\Attributes\DataProvider;
use Vunly\AuthError;
use Vunly\Client;
use Vunly\InvalidRequest;
use Vunly\LimitReached;
use Vunly\Models\CustomerToken;
use Vunly\Models\Estimate;
use Vunly\Secret;
use Vunly\Unavailable;
use Vunly\VunlyError;

final class ClientTest extends ServerTestCase
{
    // ---- construction and secrets -------------------------------------------------------------------------------------

    public function testConstructionRequiresKeyAndAddress(): void
    {
        foreach ([['', 'http://127.0.0.1:1'], ['   ', 'http://127.0.0.1:1'], [self::KEY, ''], [self::KEY, 'not a url'], [self::KEY, 'ftp://host']] as [$key, $url]) {
            try {
                new Client(apiKey: $key, baseUrl: $url);
                $this->fail('expected an exception');
            } catch (\InvalidArgumentException $e) {
                $this->assertStringNotContainsString(self::KEY, $e->getMessage());
            }
        }
    }

    public function testBaseUrlWithCredentialsIsRefusedWithoutEchoingThem(): void
    {
        try {
            new Client(apiKey: self::KEY, baseUrl: 'https://user:hunter2-synthetic@host.example');
            $this->fail('expected an exception');
        } catch (\InvalidArgumentException $e) {
            $this->assertStringNotContainsString('hunter2', $e->getMessage());
            $this->assertStringNotContainsString('hunter2', $e->getTraceAsString());
        }
    }

    public function testKeyNeverPrintsThroughAnyDumpFacility(): void
    {
        $client = new Client(apiKey: self::KEY, baseUrl: 'http://127.0.0.1:1');
        $token = CustomerToken::fromArray(['id' => 'tok_1', 'token' => 'tok_synthetic_secret_value', 'customerId' => 'cus_1', 'expiresAt' => '2026-01-01T00:00:00Z']);

        ob_start();
        var_dump($client, $token, $token->token);
        print_r($client);
        print_r($token);
        var_export($client);
        var_export($token);
        echo (string) $client, (string) $token->token, json_encode($client), json_encode($token), json_encode($token->token), json_encode([$client->credits, $client->usage]);
        echo var_export((array) $client, true), var_export((array) $token->token, true);
        echo (new \ReflectionObject($client))->__toString();
        $out = (string) ob_get_clean();

        $this->assertStringNotContainsString(self::KEY, $out);
        $this->assertStringNotContainsString('tok_synthetic_secret_value', $out);
        $this->assertStringContainsString('<hidden>', $out);
        $this->assertSame('tok_synthetic_secret_value', $token->token->reveal());

        foreach ([$client, $token, $token->token] as $object) {
            try {
                serialize($object);
                $this->fail('serialize must refuse ' . $object::class);
            } catch (\Throwable $e) {
                $this->assertStringNotContainsString(self::KEY, $e->getMessage());
            }
        }
    }

    public function testKeyNeverReachesAStackTrace(): void
    {
        try {
            new Client(apiKey: self::KEY, baseUrl: '');
            $this->fail('expected an exception');
        } catch (\InvalidArgumentException $e) {
            $this->assertStringNotContainsString(self::KEY, $e->getTraceAsString());
        }
        // and a failing call: the trace of an SDK error carries no header, no key
        $client = new Client(apiKey: self::KEY, baseUrl: 'http://127.0.0.1:1', maxRetries: 0, timeout: 1);
        try {
            $client->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertStringNotContainsString(self::KEY, (string) $e);
            $this->assertStringNotContainsString(self::KEY, $e->getTraceAsString());
            $this->assertStringNotContainsString('127.0.0.1', $e->getMessage(), 'the address must not be echoed');
            $this->assertSame(0, $e->status);
            $this->assertSame('unavailable', $e->errorCode);
        }
    }

    public function testBaseUrlIsNormalised(): void
    {
        $this->assertSame('https://h.example', Client::normalizeBaseUrl('https://h.example/'));
        $this->assertSame('https://h.example', Client::normalizeBaseUrl('https://h.example/v1'));
        $this->assertSame('http://127.0.0.1:8787', Client::normalizeBaseUrl(' http://127.0.0.1:8787/v1/ '));
    }

    // ---- requests ------------------------------------------------------------------------------------------------------

    public function testRecordSendsKeyAndIdempotencyKeyAndReportsOverage(): void
    {
        $server = $this->serve([self::json(200, self::usage(), ['Vunly-Plan-Overage' => 'true', 'Request-Id' => 'req_abc'])]);
        $r = $this->client($server)->usage->record('req_2026_0001', 'acme', 'tokens', 1050);

        $this->assertSame(90, $r->remaining);
        $this->assertFalse($r->replayed);
        $this->assertTrue($r->planOverage);
        $this->assertSame('req_abc', $r->requestId);
        $this->assertSame(-10, $r->event->credits);
        $req = $server->requests()[0];
        $this->assertSame('POST', $req['method']);
        $this->assertSame('/v1/usage-events', $req['path']);
        $this->assertSame('Bearer ' . self::KEY, $req['headers']['authorization']);
        $this->assertSame('req_2026_0001', $req['headers']['idempotency-key']);
        $this->assertMatchesRegularExpression('~^vunly-php/\d+\.\d+\.\d+ \(PHP \d+\.\d+\.\d+; \w+\)$~', $req['headers']['user-agent']);
        $this->assertArrayNotHasKey('vunly-project', $req['headers']);
        $this->assertArrayNotHasKey('vunly-environment', $req['headers']);
        $this->assertSame(['customerId' => 'acme', 'meter' => 'tokens', 'quantity' => 1050], json_decode($req['body'], true));
    }

    public function testReplayIsASuccessAndNeverReportsOverage(): void
    {
        $server = $this->serve([self::json(200, self::usage(), ['Idempotent-Replayed' => 'true', 'Vunly-Plan-Overage' => 'true'])]);
        $r = $this->client($server)->usage->record('req_2026_0001', 'acme', 'tokens', 1);
        $this->assertTrue($r->replayed);
        $this->assertFalse($r->planOverage);
    }

    /** @return iterable<string, array{string}> */
    public static function badIds(): iterable
    {
        yield 'too short' => ['short'];
        yield 'too long' => [str_repeat('a', 129)];
        yield 'bad character' => ['has space in it'];
        yield 'empty' => [''];
        yield 'newline' => ["req_2026_0001\n"];
    }

    #[DataProvider('badIds')]
    public function testBadIdempotencyIdIsRefusedBeforeAnyRequest(string $id): void
    {
        $server = $this->serve([self::json(200, self::usage())]);
        $client = $this->client($server);
        foreach ([
            fn () => $client->usage->record($id, 'acme', 'tokens', 1),
            fn () => $client->credits->grant($id, 'acme', 10),
        ] as $call) {
            try {
                $call();
                $this->fail('expected InvalidRequest');
            } catch (InvalidRequest $e) {
                $this->assertSame(400, $e->status);
                $this->assertSame('validation_error', $e->errorCode);
                $this->assertArrayHasKey('id', $e->details['fields'] ?? []);
            }
        }
        $this->assertSame([], $server->requests(), 'nothing may be sent for a bad id');
    }

    public function testLimitReachedCarriesBalanceAndIsNotRetried(): void
    {
        $server = $this->serve([self::error(402, 'insufficient_credits', ['balance' => 3, 'required' => 10])]);
        try {
            $this->client($server)->usage->record('req_2026_0002', 'acme', 'tokens', 10000);
            $this->fail('expected LimitReached');
        } catch (LimitReached $e) {
            $this->assertSame(402, $e->status);
            $this->assertSame('insufficient_credits', $e->errorCode);
            $this->assertSame(3, $e->balance());
            $this->assertSame(10, $e->required());
            $this->assertFalse($e->isRetryable());
            $this->assertSame('req_x', $e->requestId);
        }
        $this->assertCount(1, $server->requests());
    }

    public function testAuthAndInvalidRequestClassesAreFinal(): void
    {
        foreach ([[401, AuthError::class], [403, AuthError::class], [400, InvalidRequest::class], [404, InvalidRequest::class], [409, InvalidRequest::class]] as [$status, $class]) {
            $server = $this->serve([self::error($status, 'some_code')]);
            try {
                $this->client($server)->customers->get('acme');
                $this->fail("expected $class");
            } catch (VunlyError $e) {
                $this->assertInstanceOf($class, $e);
                $this->assertSame($status, $e->status);
                $this->assertSame('some_code', $e->errorCode);
                $this->assertFalse($e->isRetryable());
            }
            $this->assertCount(1, $server->requests(), "$status must not be retried");
        }
    }

    public function testErrorWithoutJsonBodyIsClassifiedByStatus(): void
    {
        foreach ([[502, Unavailable::class, 'internal_error'], [404, InvalidRequest::class, 'validation_error'], [401, AuthError::class, 'unauthenticated']] as [$status, $class, $code]) {
            $server = $this->serve([['respond' => ['status' => $status, 'body' => '<html>proxy</html>']]]);
            try {
                $this->client($server, maxRetries: 0)->credits->balance('acme');
                $this->fail("expected $class");
            } catch (VunlyError $e) {
                $this->assertInstanceOf($class, $e);
                $this->assertSame($code, $e->errorCode);
            }
        }
    }

    // ---- retries by class ----------------------------------------------------------------------------------------------

    public function testReadsRetryServerErrorsThenSucceed(): void
    {
        $server = $this->serve([self::error(503, 'unavailable'), self::error(500, 'internal_error'), self::json(200, self::balance())]);
        $b = $this->client($server)->credits->balance('acme');
        $this->assertSame(90, $b->remaining);
        $this->assertSame(10, $b->consumed);
        $this->assertSame(100, $b->granted);
        $this->assertCount(3, $server->requests());
        $this->assertCount(2, $this->sleeps);
        // backoff: min(8, 0.25 * 2^(n-1)) times a factor between 0.5 and 1
        $this->assertGreaterThanOrEqual(0.125, $this->sleeps[0]);
        $this->assertLessThanOrEqual(0.25, $this->sleeps[0]);
        $this->assertGreaterThanOrEqual(0.25, $this->sleeps[1]);
        $this->assertLessThanOrEqual(0.5, $this->sleeps[1]);
    }

    public function testRetriesAreBounded(): void
    {
        $server = $this->serve([self::error(503, 'unavailable'), self::error(503, 'unavailable'), self::error(503, 'unavailable'), self::json(200, self::balance())]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(503, $e->status);
            $this->assertTrue($e->isRetryable());
        }
        $this->assertCount(3, $server->requests(), 'one attempt and two retries');

        $server = $this->serve([self::error(503, 'unavailable'), self::json(200, self::balance())]);
        try {
            $this->client($server, maxRetries: 0)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable) {
        }
        $this->assertCount(1, $server->requests(), 'maxRetries 0 turns retries off');
    }

    public function testKeyedRetryAfterALostResponseUsesTheSameIdAndBody(): void
    {
        $server = $this->serve([['drop' => true], self::json(200, self::usage(), ['Idempotent-Replayed' => 'true'])]);
        $r = $this->client($server)->usage->record('req_2026_0003', 'acme', 'tokens', 7);
        $this->assertTrue($r->replayed);
        $reqs = $server->requests();
        $this->assertCount(2, $reqs);
        $this->assertSame($reqs[0]['headers']['idempotency-key'], $reqs[1]['headers']['idempotency-key']);
        $this->assertSame('req_2026_0003', $reqs[1]['headers']['idempotency-key']);
        $this->assertSame($reqs[0]['body'], $reqs[1]['body']);
    }

    public function testALibraryNeverReplaysADroppedRequest(): void
    {
        // retries off: a keyed POST whose answer was lost is sent exactly once. cURL's own "reused connection died, try
        // again" must not run (fresh connection, no reuse).
        $server = $this->serve([['drop' => true], self::json(200, self::usage())]);
        try {
            $this->client($server, maxRetries: 0)->usage->record('req_2026_0004', 'acme', 'tokens', 1);
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(0, $e->status);
            $this->assertSame('unavailable', $e->errorCode);
        }
        $this->assertCount(1, $server->requests());
    }

    public function testClassXIsNeverRepeatedAfterALostAnswerOrA5xx(): void
    {
        $server = $this->serve([['drop' => true], self::json(200, ['id' => 'tok_1', 'token' => 'tok_synthetic_x', 'customerId' => 'cus_1', 'expiresAt' => '2026-01-01T00:15:00Z'])]);
        try {
            $this->client($server)->customerTokens->create('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(0, $e->status);
        }
        $this->assertCount(1, $server->requests(), 'a token mint is never repeated after an ambiguous failure');

        $server = $this->serve([self::error(503, 'unavailable'), self::json(200, [])]);
        try {
            $this->client($server)->customerTokens->create('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(503, $e->status);
        }
        $this->assertCount(1, $server->requests());
    }

    public function testClassXIsRepeatedAfterA429(): void
    {
        $token = ['id' => 'tok_1', 'token' => 'tok_synthetic_x', 'customerId' => 'cus_1', 'expiresAt' => '2026-01-01T00:15:00Z'];
        $server = $this->serve([self::error(429, 'rate_limited', [], ['Retry-After' => '2']), self::json(200, $token)]);
        $t = $this->client($server)->customerTokens->create('acme', 60);
        $this->assertSame('tok_1', $t->id);
        $this->assertSame('tok_synthetic_x', $t->token->reveal());
        $this->assertCount(2, $server->requests());
        $this->assertSame([2.0], $this->sleeps, 'Retry-After is honoured');
        $this->assertSame(['customerId' => 'acme', 'ttlSeconds' => 60], json_decode($server->requests()[0]['body'], true));
    }

    public function testPlanVolumeStopIsNeverRetried(): void
    {
        $server = $this->serve([self::error(429, 'rate_limited', ['reason' => 'plan_included_volume'], ['Retry-After' => '86400']), self::json(200, self::usage())]);
        try {
            $this->client($server)->usage->record('req_2026_0005', 'acme', 'tokens', 1);
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(429, $e->status);
            $this->assertTrue($e->isPlanStop());
            $this->assertFalse($e->isRetryable());
            $this->assertSame('plan_included_volume', $e->details['reason'] ?? null);
            $this->assertSame(86400.0, $e->retryAfterSeconds);
        }
        $this->assertCount(1, $server->requests());
        $this->assertSame([], $this->sleeps);
    }

    public function testRetryAfterAboveTheCapIsSurfacedNotWaitedOn(): void
    {
        $server = $this->serve([self::error(429, 'rate_limited', [], ['Retry-After' => '31']), self::json(200, self::balance())]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(31.0, $e->retryAfterSeconds);
        }
        $this->assertSame([], $this->sleeps);
        $this->assertCount(1, $server->requests());

        $server = $this->serve([self::error(429, 'rate_limited', [], ['Retry-After' => '30']), self::json(200, self::balance())]);
        $this->client($server)->credits->balance('acme');
        $this->assertSame([30.0], $this->sleeps, '30 seconds is still waited on');
    }

    public function testMalformedSuccessIsInvalidResponseAndNotRetried(): void
    {
        foreach ([
            ['respond' => ['status' => 200, 'body' => 'not json']],
            self::json(200, ['credits' => 'many', 'consumed' => 1, 'granted' => 2]),
            self::json(200, ['consumed' => 1, 'granted' => 2]),
            self::json(200, [1, 2, 3]),
            self::json(200, ['credits' => 1.5, 'consumed' => 1, 'granted' => 2]),
        ] as $answer) {
            $server = $this->serve([$answer, self::json(200, self::balance())]);
            try {
                $this->client($server)->credits->balance('acme');
                $this->fail('expected invalid_response');
            } catch (Unavailable $e) {
                $this->assertSame('invalid_response', $e->errorCode);
                $this->assertSame(200, $e->status);
            }
            $this->assertCount(1, $server->requests());
        }
    }

    public function testBadTimestampInAnAnswerIsInvalidResponse(): void
    {
        $server = $this->serve([self::json(200, ['id' => 'cus_1', 'externalId' => 'acme', 'createdAt' => 'not a date'])]);
        try {
            $this->client($server)->customers->upsert('acme');
            $this->fail('expected invalid_response');
        } catch (Unavailable $e) {
            $this->assertSame('invalid_response', $e->errorCode);
        }
    }

    public function testUnknownFieldsAndUnknownEnumValuesAreKept(): void
    {
        $answer = ['allowed' => false, 'reason' => 'brand_new_reason', 'remaining' => 1, 'required' => 2, 'plan' => null, 'somethingNew' => ['x' => 1]];
        $server = $this->serve([self::json(200, $answer)]);
        $e = $this->client($server)->entitlements->check('acme', 'generate', new Estimate('tokens', 5));
        $this->assertFalse($e->allowed);
        $this->assertSame('brand_new_reason', $e->reason);
        $this->assertNull($e->knownReason());
        $this->assertNull($e->plan);
        $this->assertSame(['customerId' => 'acme', 'feature' => 'generate', 'estimate' => ['meter' => 'tokens', 'quantity' => 5]], json_decode($server->requests()[0]['body'], true));
    }

    public function testEntitlementCheckFailsClosed(): void
    {
        $server = $this->serve([['drop' => true], ['drop' => true], ['drop' => true]]);
        try {
            $this->client($server)->entitlements->check('acme', 'generate');
            $this->fail('the check must throw, never return allowed');
        } catch (Unavailable $e) {
            $this->assertSame(0, $e->status);
        }
        $this->assertCount(3, $server->requests(), 'the check is a read: retried like one');
    }

    public function testIntegersStayIntegers(): void
    {
        $big = 9007199254740993; // not representable as a double
        $server = $this->serve([['respond' => ['status' => 200, 'body' => '{"credits":' . $big . ',"consumed":0,"granted":' . $big . '}']]]);
        $b = $this->client($server)->credits->balance('acme');
        $this->assertSame($big, $b->remaining);
        $this->assertIsInt($b->granted);
    }

    // ---- timeouts, caps, broken bodies, redirects ------------------------------------------------------------------------

    public function testATimeoutIsUnavailable(): void
    {
        $server = $this->serve([['hang' => 3]]);
        $started = microtime(true);
        try {
            $this->client($server, maxRetries: 0, timeout: 0.4)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(0, $e->status);
            $this->assertSame('unavailable', $e->errorCode);
            $this->assertStringContainsString('did not answer within', $e->getMessage());
        }
        $this->assertLessThan(2.0, microtime(true) - $started);
    }

    public function testTheTimeoutBoundsTheWholeAttemptNotEachPhase(): void
    {
        // answers at once, then drips a 5 byte chunk every 100 ms for ~6 s: every chunk arrives "in time", the attempt does not
        $server = $this->serve([['drip' => ['status' => 200, 'bytes' => 300, 'intervalMs' => 100, 'chunk' => 5]]]);
        $started = microtime(true);
        try {
            $this->client($server, maxRetries: 0, timeout: 0.6)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(0, $e->status);
        }
        $elapsed = microtime(true) - $started;
        $this->assertLessThan(2.0, $elapsed, 'a dripping body must be cut at the timeout');
        $this->assertGreaterThan(0.5, $elapsed);
    }

    public function testTheBodyCapIsEnforcedWhileStreaming(): void
    {
        $declared = 40 * 1024 * 1024;
        $server = $this->serve([['huge' => ['status' => 200, 'bytes' => $declared]], self::json(200, self::balance())]);
        gc_collect_cycles();
        memory_reset_peak_usage();
        $before = memory_get_usage();
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('invalid_response', $e->errorCode);
            $this->assertSame(200, $e->status);
        }
        $this->assertCount(1, $server->requests(), 'invalid_response is not retried');
        $growth = memory_get_peak_usage() - $before;
        $this->assertLessThan(Client::MAX_BODY_BYTES + 2 * 1024 * 1024, $growth, "more than the cap plus a chunk was held ($growth bytes)");
        $this->assertLessThan($declared, $growth, 'the whole body was read into memory');
    }

    public function testAnOversizedErrorBodyKeepsItsClassByStatus(): void
    {
        $server = $this->serve([['huge' => ['status' => 402, 'bytes' => 20 * 1024 * 1024]]]);
        try {
            $this->client($server, maxRetries: 0)->credits->balance('acme');
            $this->fail('expected LimitReached');
        } catch (LimitReached $e) {
            $this->assertSame(402, $e->status);
            $this->assertSame('insufficient_credits', $e->errorCode);
        }
        $server = $this->serve([['huge' => ['status' => 503, 'bytes' => 20 * 1024 * 1024]]]);
        try {
            $this->client($server, maxRetries: 0)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(503, $e->status);
        }
    }

    public function testABodyThatBreaksMidReadIsUnavailableAndRetriedLikeALostAnswer(): void
    {
        $server = $this->serve([['truncate' => ['status' => 200]], self::json(200, self::balance())]);
        $this->assertSame(90, $this->client($server)->credits->balance('acme')->remaining);
        $this->assertCount(2, $server->requests());

        $server = $this->serve([['truncate' => ['status' => 200]]]);
        try {
            $this->client($server, maxRetries: 0)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('unavailable', $e->errorCode);
            $this->assertSame(0, $e->status);
        }

        // a keyed write is retried the same way, and a token mint is not
        $server = $this->serve([['truncate' => ['status' => 200]], self::json(200, self::usage())]);
        $this->assertSame(90, $this->client($server)->usage->record('req_2026_0006', 'acme', 'tokens', 1)->remaining);
        $server = $this->serve([['truncate' => ['status' => 200]], self::json(200, [])]);
        try {
            $this->client($server)->customerTokens->create('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable) {
        }
        $this->assertCount(1, $server->requests());
    }

    public function testRedirectsAreNotFollowed(): void
    {
        $other = $this->serve([self::json(200, self::balance())]);
        $server = $this->serve([['redirect' => $other->url . '/v1/customers/acme/balance']]);
        try {
            $this->client($server, maxRetries: 2)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            // a redirect is not a client mistake (not InvalidRequest/validation_error) and is never retried
            $this->assertSame(302, $e->status);
            $this->assertSame('unexpected_redirect', $e->errorCode);
        }
        $this->assertCount(1, $server->requests());
        $this->assertSame([], $other->requests(), 'the key must never be sent to the redirect target');
    }

    // ---- paths, paging ---------------------------------------------------------------------------------------------------

    public function testPathSegmentsAreEscaped(): void
    {
        $server = $this->serve([self::json(200, self::balance())]);
        $this->client($server)->credits->balance('a/b c?d');
        $this->assertSame('/v1/customers/a%2Fb%20c%3Fd/balance', $server->requests()[0]['path']);

        foreach (['', '.', '..'] as $bad) {
            try {
                $this->client($server)->credits->balance($bad);
                $this->fail('expected InvalidRequest');
            } catch (InvalidRequest $e) {
                $this->assertSame('validation_error', $e->errorCode);
            }
        }
        $this->assertCount(1, $server->requests());
    }

    public function testPagingIteratorIsLazy(): void
    {
        $c = static fn (string $id): array => ['id' => "cus_$id", 'externalId' => $id, 'name' => null, 'createdAt' => '2026-01-01T00:00:00Z'];
        $server = $this->serve([
            self::json(200, ['data' => [$c('a'), $c('b')], 'nextCursor' => 'cur_2']),
            self::json(200, ['data' => [$c('c')], 'nextCursor' => null]),
            self::json(200, ['data' => [$c('a'), $c('b')], 'nextCursor' => 'cur_2']),
        ]);
        $client = $this->client($server);

        $seen = [];
        foreach ($client->customers->all(2) as $customer) {
            $seen[] = $customer->externalId;
        }
        $this->assertSame(['a', 'b', 'c'], $seen);
        $reqs = $server->requests();
        $this->assertCount(2, $reqs);
        $this->assertSame('/v1/customers?limit=2', $reqs[0]['path']);
        $this->assertSame('/v1/customers?limit=2&cursor=cur_2', $reqs[1]['path']);

        // stop consuming: nothing is fetched ahead
        foreach ($client->customers->all(2) as $customer) {
            break;
        }
        $this->assertCount(3, $server->requests());
    }

    public function testPageCarriesAnOpaqueCursor(): void
    {
        $server = $this->serve([self::json(200, ['data' => [], 'nextCursor' => null])]);
        $page = $this->client($server)->credits->ledger('acme', 5, 'cur_opaque/+=');
        $this->assertFalse($page->hasMore());
        $this->assertSame('/v1/customers/acme/ledger?limit=5&cursor=cur_opaque%2F%2B%3D', $server->requests()[0]['path']);
    }

    // ---- the rest of the calls ---------------------------------------------------------------------------------------------

    public function testPlansSubscriptionsAndPlanUsage(): void
    {
        $plan = ['key' => 'starter', 'name' => 'Starter', 'includedCredits' => 100, 'meter' => 'tokens', 'features' => ['generate']];
        $sub = ['id' => 'sub_1', 'customerId' => 'cus_1', 'plan' => $plan, 'status' => 'active', 'periodStart' => '2026-01-01T00:00:00Z', 'periodEnd' => '2026-02-01T00:00:00Z', 'createdAt' => '2026-01-01T00:00:00Z'];
        $usage = ['plan' => ['key' => 'free', 'name' => 'Free'], 'period' => ['start' => '2026-01-01T00:00:00Z', 'end' => '2026-02-01T00:00:00Z'], 'usage' => ['events' => 4], 'unconfirmedLimits' => ['confirmed' => false, 'note' => 'n', 'includedMonthlyEvents' => 1000, 'activityRetentionDays' => 7]];
        $server = $this->serve([self::json(200, []), self::json(200, $plan), self::json(200, $sub), self::json(200, []), self::json(200, $usage)]);
        $client = $this->client($server);

        $client->plans->upsertMeter('tokens', 1000);
        $this->assertSame(['starter'], [$client->plans->upsertPlan('starter', 'Starter', 100, 'tokens', ['generate'])->key]);
        $s = $client->plans->subscribe('acme', 'starter');
        $this->assertSame('active', $s->status);
        $this->assertSame('starter', $s->plan->key);
        $client->plans->cancel('acme');
        $u = $client->planUsage();
        $this->assertFalse($u->unconfirmedLimits['confirmed']);
        $this->assertSame(4, $u->events);

        $paths = array_map(static fn (array $r): string => $r['method'] . ' ' . $r['path'], $server->requests());
        $this->assertSame(['POST /v1/meters', 'POST /v1/plans', 'POST /v1/customers/acme/subscription', 'DELETE /v1/customers/acme/subscription', 'GET /v1/plan-usage'], $paths);
    }

    public function testCustomersAndCreditsCalls(): void
    {
        $customer = ['id' => 'cus_1', 'externalId' => 'acme', 'name' => 'Acme', 'createdAt' => '2026-01-01T00:00:00Z'];
        $server = $this->serve([self::json(200, $customer), self::json(201, self::entry(100))]);
        $client = $this->client($server);
        $c = $client->customers->upsert('acme', 'Acme');
        $this->assertSame('Acme', $c->name);
        $this->assertSame('acme', $c->externalId);
        $g = $client->credits->grant('grant_acme_0001', 'acme', 100, 'sandbox top-up');
        $this->assertSame(100, $g->credits);
        $this->assertSame('debit', $g->kind);
        $reqs = $server->requests();
        $this->assertSame(['externalId' => 'acme', 'name' => 'Acme'], json_decode($reqs[0]['body'], true));
        $this->assertSame('grant_acme_0001', $reqs[1]['headers']['idempotency-key']);
        $this->assertSame(['credits' => 100, 'reason' => 'sandbox top-up'], json_decode($reqs[1]['body'], true));
    }
}
