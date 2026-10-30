<?php

declare(strict_types=1);

namespace Vunly\Tests;

use PHPUnit\Framework\Attributes\DataProvider;
use Vunly\Client;
use Vunly\Executor;
use Vunly\InvalidRequest;
use Vunly\Unavailable;
use Vunly\VunlyError;
use Vunly\WebhookError;
use Vunly\Webhooks;

/** Regression tests for hardening cases (oversized bodies, malformed input, unsafe responses), one test per case. */
final class HardeningTest extends ServerTestCase
{
    // ---- a valid body under the cap must not take the process down ---------------------------------------------------------

    /**
     * Valid JSON of the shapes PHP amplifies most: a tiny array costs about 400 bytes, so 600 KB of text is enough to pass a
     * 32 MB memory_limit, and 2.4 MB passes 128 MB. Every one must end in an SDK exception, never in a fatal error.
     *
     * @return iterable<string, array{string, int, string}>
     */
    public static function amplifyingBodies(): iterable
    {
        $shapes = [
            'nested arrays 2.4 MB' => static fn (): string => '{"data":[' . str_repeat('[1],', 600_000) . '[1]]}',
            'nested arrays 600 KB' => static fn (): string => '{"data":[' . str_repeat('[1],', 150_000) . '[1]]}',
            'small objects 2.4 MB' => static fn (): string => '{"data":[' . str_repeat('{"a":1},', 300_000) . '{"a":1}]}',
            'small objects 600 KB' => static fn (): string => '{"data":[' . str_repeat('{"a":1},', 75_000) . '{"a":1}]}',
            'objects with keys 900 KB' => static fn (): string => '{' . implode(',', array_map(static fn (int $i): string => '"k' . $i . '":{"v":[' . $i . ']}', range(1, 40_000))) . '}',
            'top-level list 2.4 MB' => static fn (): string => '[' . str_repeat('[1],', 600_000) . '[1]]',
        ];
        foreach ($shapes as $name => $make) {
            foreach ([['32M', 200], ['128M', 200], ['32M', 400], ['128M', 429], ['128M', 500]] as [$limit, $status]) {
                yield "$name, $status, memory_limit $limit" => [$make(), $status, $limit];
            }
        }
    }

    #[DataProvider('amplifyingBodies')]
    public function testAnAmplifyingBodyEndsInAnSdkExceptionNeverAFatalError(string $body, int $status, string $limit): void
    {
        $server = $this->serve([['respond' => ['status' => $status, 'body' => $body]]]);
        unset($body);

        $cmd = sprintf('%s -d memory_limit=%s %s %s 2>&1', escapeshellarg(PHP_BINARY), $limit, escapeshellarg(__DIR__ . '/Support/decode-child.php'), escapeshellarg($server->url));
        exec($cmd, $lines, $code);
        $out = implode("\n", $lines);
        $this->assertStringNotContainsString('Allowed memory size', $out, 'the process ran out of memory');
        $this->assertSame(0, $code, $out);
        $expected = match ($status) {
            200 => 'RESULT Vunly\Unavailable invalid_response 200',
            400 => 'RESULT Vunly\InvalidRequest validation_error 400',
            429 => 'RESULT Vunly\Unavailable rate_limited 429',
            default => 'RESULT Vunly\Unavailable internal_error 500',
        };
        $this->assertStringContainsString($expected, $out);
    }

    public function testARealSizedPageStillDecodesUnderASmallMemoryLimit(): void
    {
        $c = static fn (int $i): array => ['id' => "cus_$i", 'externalId' => "ext_$i", 'name' => 'n', 'createdAt' => '2026-01-01T00:00:00Z'];
        $server = $this->serve([self::json(200, ['data' => array_map($c, range(1, 100)), 'nextCursor' => null])]);
        $cmd = sprintf('%s -d memory_limit=32M %s %s list 2>&1', escapeshellarg(PHP_BINARY), escapeshellarg(__DIR__ . '/Support/decode-child.php'), escapeshellarg($server->url));
        exec($cmd, $lines, $code);
        $this->assertStringContainsString('RESULT success', implode("\n", $lines));
    }

    public function testAnAmplifyingWebhookPayloadIsInvalidPayloadNotAFatalError(): void
    {
        $cmd = sprintf('%s -d memory_limit=32M %s 2>&1', escapeshellarg(PHP_BINARY), escapeshellarg(__DIR__ . '/Support/webhook-child.php'));
        exec($cmd, $lines, $code);
        $out = implode("\n", $lines);
        $this->assertStringNotContainsString('Allowed memory size', $out);
        $this->assertStringContainsString('RESULT invalid_payload', $out);
    }

    public function testVerifyingALargeSignedWebhookNeedsNoSecondCopyOfTheBody(): void
    {
        // 14 MB body, memory_limit 28M: one copy fits, two (the old "<t>." . body string) do not
        $cmd = sprintf('%s -d memory_limit=28M %s 2>&1', escapeshellarg(PHP_BINARY), escapeshellarg(__DIR__ . '/Support/webhook-big-child.php'));
        exec($cmd, $lines, $code);
        $out = implode("\n", $lines);
        $this->assertStringNotContainsString('Allowed memory size', $out);
        // signature verified (no_matching_signature would mean the streamed HMAC differs), then refused as too large to decode
        $this->assertStringContainsString('RESULT invalid_payload', $out);
    }

    /** @return iterable<string, array{int, int}> */
    public static function hugeBodies(): iterable
    {
        foreach ([16_777_100, 16_777_216, 1_048_577, 10_000_000] as $size) {
            foreach ([200, 400, 500] as $status) {
                yield "$size bytes, $status" => [$size, $status];
            }
        }
    }

    #[DataProvider('hugeBodies')]
    public function testABodyNearOrOverTheReadLimitUnderASmallMemoryLimitIsNeverAFatalError(int $size, int $status): void
    {
        $server = $this->serve([['huge' => ['status' => $status, 'bytes' => $size]]]);
        $cmd = sprintf('%s -d memory_limit=32M %s %s 2>&1', escapeshellarg(PHP_BINARY), escapeshellarg(__DIR__ . '/Support/decode-child.php'), escapeshellarg($server->url));
        exec($cmd, $lines, $code);
        $out = implode("\n", $lines);
        $this->assertStringNotContainsString('Allowed memory size', $out, 'the process ran out of memory');
        $this->assertSame(0, $code, $out);
        $expected = match ($status) {
            200 => 'RESULT Vunly\Unavailable invalid_response 200',
            400 => 'RESULT Vunly\InvalidRequest validation_error 400',
            default => 'RESULT Vunly\Unavailable internal_error 500',
        };
        $this->assertStringContainsString($expected, $out);
        $this->assertMatchesRegularExpression('/PEAK (\d+)/', $out);
        preg_match('/PEAK (\d+)/', $out, $m);
        $this->assertLessThan(Client::MAX_BODY_BYTES + 6 * 1024 * 1024, (int) $m[1], 'more than the read limit plus a small margin was held');
    }

    /** @return iterable<string, array{int}> */
    public static function paddedErrorSizes(): iterable
    {
        foreach ([1_000, 65_536, 65_537, 70_000, 300_000] as $size) {
            yield "$size bytes" => [$size];
        }
    }

    /** An error envelope in the first bytes, then padding up to exactly $size bytes. */
    private static function paddedError(string $envelopeJson, int $size): string
    {
        $head = substr($envelopeJson, 0, -1) . ',"pad":"';
        $tail = '"}';

        return $head . str_repeat('x', max(0, $size - strlen($head) - strlen($tail))) . $tail;
    }

    #[DataProvider('paddedErrorSizes')]
    public function testAnErrorBodyOverTheReadLimitKeepsItsEnvelopeSoThePlanStopIsStillNeverRetried(int $size): void
    {
        $envelope = (string) json_encode(['error' => ['code' => 'rate_limited', 'message' => 'plan volume', 'requestId' => 'req_big', 'details' => ['reason' => 'plan_included_volume']]]);
        $body = self::paddedError($envelope, $size);
        $server = $this->serve([['respond' => ['status' => 429, 'body' => $body, 'headers' => ['Retry-After' => '86400']]], self::json(200, self::balance())]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(429, $e->status);
            $this->assertTrue($e->isPlanStop(), 'the plan stop must survive an oversized body');
            $this->assertFalse($e->isRetryable());
            $this->assertSame('plan_included_volume', $e->details['reason'] ?? null);
            $this->assertSame('req_big', $e->requestId);
            $this->assertSame('rate_limited', $e->errorCode);
        }
        $this->assertCount(1, $server->requests(), 'the plan stop must not be retried');
    }

    #[DataProvider('paddedErrorSizes')]
    public function testAnOversizedErrorKeepsCodeMessageRequestIdAndDetailsOf402(int $size): void
    {
        $envelope = (string) json_encode(['error' => ['code' => 'insufficient_credits', 'message' => 'not enough', 'requestId' => 'req_402', 'details' => ['balance' => 3, 'required' => 10]]]);
        $server = $this->serve([['respond' => ['status' => 402, 'body' => self::paddedError($envelope, $size)]]]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected LimitReached');
        } catch (\Vunly\LimitReached $e) {
            $this->assertSame(3, $e->balance());
            $this->assertSame(10, $e->required());
            $this->assertSame('req_402', $e->requestId);
            $this->assertSame('not enough', $e->getMessage());
        }
    }

    /** @return iterable<string, array{string}> */
    public static function bloatedDetails(): iterable
    {
        yield '20,001 containers inside details' => ['"bloat":[' . str_repeat('[],', 20_000) . '[]]'];
        yield 'nesting 65 inside details' => ['"deep":' . str_repeat('[', 65) . str_repeat(']', 65)];
        yield 'both' => ['"deep":' . str_repeat('[', 65) . str_repeat(']', 65) . ',"bloat":[' . str_repeat('{},', 20_000) . '{}]'];
    }

    #[DataProvider('bloatedDetails')]
    public function testACompleteErrorObjectThatExceedsTheDecodeLimitsStillKeepsThePlanStop(string $bloat): void
    {
        // the whole error object fits in 64 KB (so nothing is cut) but Json::decode refuses it: the members are read one by one
        $body = '{"error":{"code":"rate_limited","message":"plan volume","requestId":"req_bloat","details":{"reason":"plan_included_volume",' . $bloat . '}}}';
        $this->assertLessThan(Client::MAX_ERROR_BODY_BYTES, strlen($body));
        $server = $this->serve([['respond' => ['status' => 429, 'body' => $body, 'headers' => ['Retry-After' => '86400']]], self::json(200, self::balance())]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertTrue($e->isPlanStop(), 'the plan stop must survive a bloated envelope');
            $this->assertSame('req_bloat', $e->requestId);
            $this->assertSame('rate_limited', $e->errorCode);
            $this->assertSame('plan_included_volume', $e->details['reason'] ?? null);
        }
        $this->assertCount(1, $server->requests(), 'the plan stop must not be retried');
    }

    /** @return iterable<string, array{string}> */
    public static function badCustomers(): iterable
    {
        yield 'empty' => [''];
        yield 'dot' => ['.'];
        yield 'dot dot' => ['..'];
    }

    #[DataProvider('badCustomers')]
    public function testEmptyAndDotCustomersAreRefusedLocallyInEveryCallThatTakesOne(string $customer): void
    {
        $server = $this->serve([self::json(200, [])]);
        $client = $this->client($server);
        foreach ([
            fn () => $client->customerTokens->create($customer),
            fn () => $client->entitlements->check($customer, 'generate'),
            fn () => $client->usage->record('req_2026_9999', $customer, 'tokens', 1),
            fn () => $client->credits->balance($customer),
        ] as $call) {
            try {
                $call();
                $this->fail('expected InvalidRequest');
            } catch (InvalidRequest $e) {
                $this->assertSame('validation_error', $e->errorCode);
                $this->assertArrayHasKey('customer', $e->details['fields'] ?? []);
            }
        }
        $this->assertSame([], $server->requests());
    }

    public function testObjectPrefixReadsOnlyWhatIsComplete(): void
    {
        $this->assertSame(['error' => ['code' => 'a', 'details' => ['reason' => 'r']]], \Vunly\Json::objectPrefix('{"error":{"code":"a","details":{"reason":"r"},"message":"cut off here'));
        $this->assertSame(['a' => 1], \Vunly\Json::objectPrefix('{"a":1,"b":tru'));
        $this->assertSame([], \Vunly\Json::objectPrefix('<html>not json</html>'));
        $this->assertSame([], \Vunly\Json::objectPrefix(''));
        $this->assertSame(['k' => 'a"b{'], \Vunly\Json::objectPrefix('{"k":"a\"b{","z":['));
    }

    /** @return iterable<string, array{int, int}> */
    public static function hostileHeaders(): iterable
    {
        yield 'one header of 1 MiB' => [1, 1_048_576];
        yield '300 headers of 1 KB' => [300, 1_024];
    }

    /**
     * CANARY test, not a guard we control: the SDK has no header cap of its own and libcurl offers no option for one. What
     * keeps PHP's memory flat here is libcurl's built-in limits (a header line of 100 KB and 300 KB of headers in all in
     * libcurl 8.14.1, the version in the pinned image). The test pins that behaviour: if a libcurl without those limits ever
     * shipped, a hostile header would grow PHP's memory and this test would fail. There is no mutation of SDK code that
     * turns it red.
     */
    #[DataProvider('hostileHeaders')]
    public function testHostileResponseHeadersEndInUnavailableAtALowMemoryLimit(int $count, int $size): void
    {
        $server = $this->serve([['bigheaders' => ['count' => $count, 'size' => $size]]]);
        $cmd = sprintf('%s -d memory_limit=32M %s %s 2>&1', escapeshellarg(PHP_BINARY), escapeshellarg(__DIR__ . '/Support/decode-child.php'), escapeshellarg($server->url));
        exec($cmd, $lines, $code);
        $out = implode("\n", $lines);
        $this->assertStringNotContainsString('Allowed memory size', $out);
        $this->assertSame(0, $code, $out);
        $this->assertStringContainsString('RESULT Vunly\Unavailable unavailable 0', $out);
    }

    public function testTheReadLimitsAreTheDocumentedOnes(): void
    {
        $this->assertSame(1_048_576, Client::MAX_BODY_BYTES);
        $this->assertSame(65_536, Client::MAX_ERROR_BODY_BYTES);
    }

    public function testTheDecodeLimitsAreExactAndDocumented(): void
    {
        $this->assertSame(1_048_576, \Vunly\Json::MAX_BYTES);
        $this->assertSame(20_000, \Vunly\Json::MAX_CONTAINERS);
        $this->assertTrue(\Vunly\Json::decode('{"a":[1,2,{"b":null}]}') !== null);
        $this->expectException(\UnexpectedValueException::class);
        \Vunly\Json::decode('[' . str_repeat('[],', 20_000) . '[]]');
    }

    // ---- unexpected_redirect is not worth retrying later ----------------------------------------------------------------

    public function testAnUnexpectedRedirectIsNotRetryable(): void
    {
        $server = $this->serve([['redirect' => 'http://127.0.0.1:1/']]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('unexpected_redirect', $e->errorCode);
            $this->assertFalse($e->isRetryable());
        }
        $this->assertTrue((new Unavailable('x', 'unavailable', 0))->isRetryable());
    }

    // ---- empty pages with fresh cursors ----------------------------------------------------------------------------------

    public function testEndlessEmptyPagesWithFreshCursorsStopPaging(): void
    {
        $empty = static fn (int $i): array => self::json(200, ['data' => [], 'nextCursor' => "cur_$i"]);
        $server = $this->serve(array_map($empty, range(1, 12)));
        try {
            foreach ($this->client($server)->customers->all(1) as $_) {
            }
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('invalid_response', $e->errorCode);
        }
        $this->assertCount(3, $server->requests(), 'stops at the third empty page that promises another (the literal 3 of the contract)');

        $server = $this->serve(array_map($empty, range(1, 12)));
        try {
            foreach ($this->client($server)->credits->allLedger('acme', 1) as $_) {
            }
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('invalid_response', $e->errorCode);
        }
        $this->assertCount(3, $server->requests(), 'stops at the third empty page that promises another (the literal 3 of the contract)');
    }

    public function testAnOccasionalEmptyPageIsFine(): void
    {
        $c = ['id' => 'cus_a', 'externalId' => 'a', 'name' => null, 'createdAt' => '2026-01-01T00:00:00Z'];
        $server = $this->serve([
            self::json(200, ['data' => [], 'nextCursor' => 'c1']),
            self::json(200, ['data' => [], 'nextCursor' => 'c2']),
            self::json(200, ['data' => [$c], 'nextCursor' => 'c3']),
            self::json(200, ['data' => [], 'nextCursor' => 'c4']),
            self::json(200, ['data' => [$c], 'nextCursor' => null]),
        ]);
        $n = 0;
        foreach ($this->client($server)->customers->all(1) as $_) {
            $n++;
        }
        $this->assertSame(2, $n);
    }

    // ---- paging ---------------------------------------------------------------------------------------------------------

    public function testACursorThatDoesNotAdvanceStopsPagingWithInvalidResponse(): void
    {
        $c = static fn (string $id): array => ['id' => "cus_$id", 'externalId' => $id, 'name' => null, 'createdAt' => '2026-01-01T00:00:00Z'];
        $page = self::json(200, ['data' => [$c('a')], 'nextCursor' => 'cur_same']);
        $server = $this->serve([$page, $page, $page, $page, $page]);
        $seen = 0;
        try {
            foreach ($this->client($server)->customers->all(1) as $_) {
                $seen++;
            }
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('invalid_response', $e->errorCode);
        }
        $this->assertLessThanOrEqual(3, count($server->requests()), 'the loop must stop at the repeat, not fetch forever');

        $entry = ['id' => 'led_1', 'customerId' => 'cus_1', 'kind' => 'grant', 'credits' => 1, 'createdAt' => '2026-01-01T00:00:00Z'];
        $page = self::json(200, ['data' => [$entry], 'nextCursor' => 'cur_loop']);
        $server = $this->serve([$page, $page, $page, $page, $page]);
        try {
            foreach ($this->client($server)->credits->allLedger('acme', 1) as $_) {
            }
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame('invalid_response', $e->errorCode);
        }
        $this->assertLessThanOrEqual(3, count($server->requests()));
    }

    // ---- what the server sends is data, not a log line ---------------------------------------------------------------------

    public function testServerSuppliedTextIsSanitisedAndBounded(): void
    {
        $evil = "line one\r\nFORGED: log line\x1b[31m red \0nul \u{202E}rtl\u{200B}zw \x7f del \u{0085}c1 " . str_repeat('x', 900);
        $server = $this->serve([['respond' => ['status' => 400, 'json' => ['error' => ['code' => "bad\ncode" . str_repeat('c', 200), 'message' => $evil, 'requestId' => "req\r\n" . str_repeat('r', 300)]]]]]);
        try {
            $this->client($server, maxRetries: 0)->credits->balance('acme');
            $this->fail('expected InvalidRequest');
        } catch (InvalidRequest $e) {
            foreach ([$e->getMessage(), $e->errorCode, (string) $e->requestId, (string) $e] as $text) {
                $this->assertSame(0, preg_match('/[\x{0000}-\x{001F}\x{007F}-\x{009F}\x{202A}-\x{202E}\x{2066}-\x{2069}\x{200B}-\x{200F}]/u', $text), 'a control or bidi character survived: ' . json_encode($text));
            }
            $this->assertLessThanOrEqual(500, mb_strlen($e->getMessage()));
            $this->assertLessThanOrEqual(64, strlen($e->errorCode));
            $this->assertLessThanOrEqual(128, strlen((string) $e->requestId));
            $this->assertStringStartsWith('line one FORGED: log line', $e->getMessage());
        }
    }

    public function testTheRequestIdHeaderIsSanitisedToo(): void
    {
        $server = $this->serve([self::json(200, self::usage(), ['Request-Id' => 'req_' . str_repeat('h', 300)])]);
        $res = $this->client($server)->usage->record('req_2026_0100', 'acme', 'tokens', 1);
        $this->assertLessThanOrEqual(128, strlen((string) $res->requestId));
    }

    public function testInvalidUtf8InAnErrorBecomesPrintableAscii(): void
    {
        $this->assertSame('ab c', Executor::clean("ab\xff\xfe c", 50));
    }

    // ---- the key -----------------------------------------------------------------------------------------------------------

    public function testAKeyWithATrailingNewlineIsTrimmedAndSentClean(): void
    {
        $server = $this->serve([self::json(200, self::balance())]);
        (new Client(apiKey: self::KEY . "\n", baseUrl: $server->url))->credits->balance('acme');
        $this->assertSame('Bearer ' . self::KEY, $server->requests()[0]['headers']['authorization']);
    }

    /** @return iterable<string, array{string}> */
    public static function badKeys(): iterable
    {
        yield 'embedded CRLF' => ["vk_test_synthetic_a\r\nX-Injected: 1"];
        yield 'embedded space' => ['vk_test_synthetic a'];
        yield 'tab inside' => ["vk_test_synthetic\tb"];
        yield 'NUL' => ["vk_test_synthetic\0b"];
        yield 'non-ASCII' => ["vk_test_synthetic_\u{00e9}"];
        yield 'DEL' => ["vk_test_synthetic\x7f"];
        yield 'leading space' => [' vk_test_synthetic_a'];
        yield 'trailing space' => ['vk_test_synthetic_a '];
        yield 'trailing tab' => ["vk_test_synthetic_a\t"];
        yield 'trailing NUL' => ["vk_test_synthetic_a\0"];
        yield 'trailing lone CR' => ["vk_test_synthetic_a\r"];
        yield 'leading newline' => ["\nvk_test_synthetic_a"];
        yield 'leading CRLF' => ["\r\nvk_test_synthetic_a"];
        yield 'two trailing newlines' => ["vk_test_synthetic_a\n\n"];
        yield 'space then newline' => ["vk_test_synthetic_a \n"];
    }

    public function testOnlyOneTrailingLineBreakIsRemovedLfOrCrlf(): void
    {
        foreach (["\n", "\r\n"] as $ending) {
            $server = $this->serve([self::json(200, self::balance())]);
            (new Client(apiKey: self::KEY . $ending, baseUrl: $server->url))->credits->balance('acme');
            $this->assertSame('Bearer ' . self::KEY, $server->requests()[0]['headers']['authorization']);
        }
    }

    #[DataProvider('badKeys')]
    public function testAKeyThatCouldInjectAHeaderIsRefusedWithoutEchoingIt(string $key): void
    {
        try {
            new Client(apiKey: $key, baseUrl: 'http://127.0.0.1:1');
            $this->fail('expected an exception');
        } catch (\InvalidArgumentException $e) {
            $this->assertStringContainsString('API key', $e->getMessage());
            $this->assertStringNotContainsString('synthetic', $e->getMessage());
            $this->assertStringNotContainsString('synthetic', $e->getTraceAsString());
        }
    }

    // ---- the address ---------------------------------------------------------------------------------------------------------

    /** @return iterable<string, array{string}> */
    public static function badUrls(): iterable
    {
        yield 'no host' => ['http://'];
        yield 'empty host' => ['http:///x'];
        yield 'port zero' => ['http://host:0'];
        yield 'port too large' => ['http://host:99999'];
        yield 'space' => ['http://exa mple.com'];
        yield 'bad host char' => ['http://ex_ample!.com'];
        yield 'no scheme' => ['example.com'];
        yield 'file' => ['file:///etc/passwd'];
        yield 'newline' => ["http://host\n.example"];
        yield 'credentials' => ['http://user:hunter2-synthetic@host'];
        yield 'query' => ['http://host?x=1'];
        yield 'fragment' => ['http://host#f'];
        yield 'unclosed ipv6' => ['http://[::1'];
    }

    #[DataProvider('badUrls')]
    public function testAMalformedBaseUrlIsRefusedWithoutEchoingIt(string $url): void
    {
        try {
            new Client(apiKey: self::KEY, baseUrl: $url);
            $this->fail('expected an exception');
        } catch (\InvalidArgumentException $e) {
            $this->assertStringNotContainsString('hunter2', $e->getMessage());
            $this->assertStringNotContainsString('hunter2', $e->getTraceAsString());
        }
    }

    public function testWellFormedBaseUrlsAreAccepted(): void
    {
        foreach (['http://127.0.0.1:8787', 'https://api.example.com', 'https://api.example.com/v1/', 'http://[::1]:8080', 'http://localhost'] as $url) {
            $this->assertNotSame('', Client::normalizeBaseUrl($url));
        }
    }

    // ---- ids, timeouts ---------------------------------------------------------------------------------------------------------

    public function testMessagesNameTheRealProblem(): void
    {
        $client = $this->client($this->serve([]));
        $cases = [
            [fn () => $client->credits->balance(''), 'customer must not be empty', 'customer'],
            [fn () => $client->credits->balance('.'), 'must not be "." or ".."', 'customer'],
            [fn () => $client->credits->balance('..'), 'must not be "." or ".."', 'customer'],
            [fn () => $client->credits->balance(null), 'customer must be a string, got null', 'customer'],
            [fn () => $client->customers->upsert(null), 'id must be a string, got null', 'id'],
            [fn () => $client->customers->get(42), 'customer must be a string, got int', 'customer'],
            [fn () => $client->plans->subscribe(null, 'p'), 'customer must be a string, got null', 'customer'],
            [fn () => $client->plans->cancel([]), 'customer must be a string, got array', 'customer'],
            [fn () => $client->customerTokens->create(null), 'customer must be a string, got null', 'customer'],
            [fn () => $client->customerTokens->revoke(null), 'id must be a string, got null', 'id'],
            [fn () => $client->entitlements->check(null, 'f'), 'customer must be a string, got null', 'customer'],
            [fn () => $client->usage->record('req_2026_9998', null, 'm', 1), 'customer must be a string, got null', 'customer'],
            [fn () => $client->credits->grant('req_2026_9998', null, 1), 'customer must be a string, got null', 'customer'],
            [fn () => $client->usage->record(null, 'acme', 'm', 1), 'id must be a string of 8 to 128 characters', 'id'],
        ];
        foreach ($cases as [$call, $message, $field]) {
            try {
                $call();
                $this->fail("expected InvalidRequest: $message");
            } catch (InvalidRequest $e) {
                $this->assertStringContainsString($message, $e->getMessage());
                $this->assertSame('validation_error', $e->errorCode);
                $this->assertSame(400, $e->status);
                $this->assertArrayHasKey($field, $e->details['fields'] ?? []);
            }
        }
    }

    public function testANullOrNonStringIdIsAnInvalidRequestWithFieldsIdNotATypeError(): void
    {
        $server = $this->serve([self::json(200, self::usage())]);
        $client = $this->client($server);
        foreach ([null, 123, 1.5, [], new \stdClass()] as $id) {
            foreach ([
                fn () => $client->usage->record($id, 'acme', 'tokens', 1),
                fn () => $client->credits->grant($id, 'acme', 1),
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
        }
        $this->assertSame([], $server->requests());
    }

    /** @return iterable<string, array{float}> */
    public static function badTimeouts(): iterable
    {
        yield 'INF' => [INF];
        yield 'NAN' => [NAN];
        yield 'negative' => [-1.0];
        yield 'zero' => [0.0];
        yield 'one nanosecond' => [1e-9];
        yield 'a day' => [86400.0];
    }

    #[DataProvider('badTimeouts')]
    public function testATimeoutOutsideTheSaneRangeIsRefused(float $timeout): void
    {
        $this->expectException(\InvalidArgumentException::class);
        $this->expectExceptionMessageMatches('/timeout must be between 0\.05 and 600/');
        new Client(apiKey: self::KEY, baseUrl: 'http://127.0.0.1:1', timeout: $timeout);
    }

    public function testTheEdgesOfTheTimeoutRangeAreAccepted(): void
    {
        $this->assertInstanceOf(Client::class, new Client(apiKey: self::KEY, baseUrl: 'http://127.0.0.1:1', timeout: Client::MIN_TIMEOUT_SECONDS));
        $this->assertInstanceOf(Client::class, new Client(apiKey: self::KEY, baseUrl: 'http://127.0.0.1:1', timeout: Client::MAX_TIMEOUT_SECONDS));
    }

    // ---- Retry-After: the shared rule (conformance/README.md) ---------------------------------------------

    /** @return iterable<string, array{string, string, float|null}> */
    public static function retryAfterVectors(): iterable
    {
        /** @var array{vectors: list<array{header: string, kind: string, seconds?: int|float}>} $doc */
        $doc = json_decode((string) file_get_contents(__DIR__ . '/retry-after-vectors.json'), true, 32, JSON_THROW_ON_ERROR);
        foreach ($doc['vectors'] as $i => $v) {
            yield "#$i " . substr(json_encode($v['header'], JSON_UNESCAPED_UNICODE) ?: '', 0, 50) => [$v['header'], $v['kind'], isset($v['seconds']) ? (float) $v['seconds'] : null];
        }
    }

    #[DataProvider('retryAfterVectors')]
    public function testRetryAfterVectors(string $header, string $kind, ?float $seconds): void
    {
        $got = Executor::parseRetryAfter($header);
        $this->assertSame($kind, $got['kind'], "kind of " . json_encode($header));
        if ($kind !== 'ignored') {
            $this->assertEqualsWithDelta($seconds, $got['seconds'], 1e-9);
        } else {
            $this->assertNull($got['seconds']);
        }
    }

    /** @return iterable<string, array{string}> */
    public static function unrepresentable(): iterable
    {
        yield '1e999' => ['1e999'];
        yield 'Infinity' => ['Infinity'];
        yield '400 digits' => [str_repeat('9', 400)];
    }

    #[DataProvider('unrepresentable')]
    public function testAnUnrepresentableRetryAfterIsAVeryLongWaitNeverAnEarlyRetry(string $header): void
    {
        $server = $this->serve([self::error(429, 'rate_limited', [], ['Retry-After' => $header]), self::json(200, self::balance())]);
        try {
            $this->client($server)->credits->balance('acme');
            $this->fail('expected Unavailable');
        } catch (Unavailable $e) {
            $this->assertSame(86400.0, $e->retryAfterSeconds);
        }
        $this->assertCount(1, $server->requests(), 'no early retry');
        $this->assertSame([], $this->sleeps);
    }

    public function testAnUnparseableRetryAfterIsTheNormalBackoff(): void
    {
        $server = $this->serve([self::error(429, 'rate_limited', [], ['Retry-After' => 'Wed, 21 Oct 2026 07:28:00 GMT']), self::json(200, self::balance())]);
        $this->client($server)->credits->balance('acme');
        $this->assertCount(1, $this->sleeps);
        $this->assertLessThanOrEqual(0.25, $this->sleeps[0]);
    }

    // ---- webhooks: a missing secret must not make deliveries forgeable -----------------------------------------------------------

    /** @return iterable<string, array{string|list<string>}> */
    public static function emptySecrets(): iterable
    {
        yield 'empty string' => [''];
        yield 'spaces' => ['   '];
        yield 'empty list' => [[]];
        yield 'list with an empty secret' => [['good-secret', '']];
        yield 'list with spaces' => [['  ', 'good-secret']];
    }

    /** @param string|list<string> $secrets */
    #[DataProvider('emptySecrets')]
    public function testAnEmptyWebhookSecretIsRefusedEvenForAForgedDelivery(string|array $secrets): void
    {
        $payload = '{"id":"evt_1","type":"credits.low","data":{}}';
        $t = time();
        // exactly what an attacker computes when the receiver's secret is '' (an unset environment variable)
        $forged = "t=$t,v1=" . hash_hmac('sha256', "$t.$payload", '');
        try {
            Webhooks::verify($payload, $forged, $secrets);
            $this->fail('a delivery signed with the empty key was accepted');
        } catch (\InvalidArgumentException $e) {
            $this->assertStringNotContainsString('good-secret', $e->getMessage());
            $this->assertStringContainsString('webhook signing secret', $e->getMessage());
        } catch (WebhookError $e) {
            $this->fail('expected a configuration error, got ' . $e->reason);
        }
    }

    public function testErrorsAreVunlyErrors(): void
    {
        $this->assertTrue(is_subclass_of(Unavailable::class, VunlyError::class));
    }
}
