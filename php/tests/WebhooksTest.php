<?php

declare(strict_types=1);

namespace Vunly\Tests;

use PHPUnit\Framework\TestCase;
use Vunly\WebhookError;
use Vunly\Webhooks;

final class WebhooksTest extends TestCase
{
    private const NOW = 1_800_000_000;
    private const PAYLOAD = '{"id":"evt_1","type":"credits.low","createdAt":"2026-01-01T00:00:00Z","data":{"a":1}}';

    private static function sign(string $secret, int $t, string $payload): string
    {
        return "t=$t,v1=" . hash_hmac('sha256', "$t.$payload", $secret);
    }

    private function reason(string $payload, string $header, string|array $secrets): string
    {
        try {
            Webhooks::verify($payload, $header, $secrets, self::NOW);
        } catch (WebhookError $e) {
            return $e->reason;
        }

        return 'verified';
    }

    public function testAVerifiedDeliveryIsDecoded(): void
    {
        $event = Webhooks::verify(self::PAYLOAD, self::sign('old', self::NOW, self::PAYLOAD), ['new', 'old'], self::NOW);
        $this->assertSame('evt_1', $event->id);
        $this->assertSame('credits.low', $event->type);
        $this->assertSame(['a' => 1], $event->data);
        $this->assertSame('2026-01-01', $event->createdAt?->format('Y-m-d'));
    }

    public function testASingleSecretAsAStringAndTheToleranceEdges(): void
    {
        $this->assertSame('verified', $this->reason(self::PAYLOAD, self::sign('s', self::NOW - 300, self::PAYLOAD), 's'));
        $this->assertSame('verified', $this->reason(self::PAYLOAD, self::sign('s', self::NOW + 300, self::PAYLOAD), 's'));
        $this->assertSame('stale_timestamp', $this->reason(self::PAYLOAD, self::sign('s', self::NOW - 301, self::PAYLOAD), 's'));
        $this->assertSame('stale_timestamp', $this->reason(self::PAYLOAD, self::sign('s', self::NOW + 301, self::PAYLOAD), 's'));
    }

    public function testEveryRefusal(): void
    {
        $this->assertSame('missing_header', $this->reason(self::PAYLOAD, '', 's'));
        $this->assertSame('missing_header', $this->reason(self::PAYLOAD, '   ', 's'));
        $this->assertSame('malformed_header', $this->reason(self::PAYLOAD, 'garbage', 's'));
        $this->assertSame('malformed_header', $this->reason(self::PAYLOAD, 't=abc,v1=00', 's'));
        $this->assertSame('malformed_header', $this->reason(self::PAYLOAD, 't=1800000000', 's'));
        $this->assertSame('no_matching_signature', $this->reason(self::PAYLOAD, self::sign('other', self::NOW, self::PAYLOAD), 's'));
        // a v1 that is not 64 hex characters is not a signature (the shared vector "v1 is not 64 hex characters")
        $this->assertSame('malformed_header', $this->reason(self::PAYLOAD, 't=' . self::NOW . ',v1=zz', 's'));
        $this->assertSame('no_matching_signature', $this->reason(self::PAYLOAD . ' ', self::sign('s', self::NOW, self::PAYLOAD), 's'), 'a tampered body');
        $bad = '{"nope":true}';
        $this->assertSame('invalid_payload', $this->reason($bad, self::sign('s', self::NOW, $bad), 's'));
        $this->assertSame('invalid_payload', $this->reason('not json', self::sign('s', self::NOW, 'not json'), 's'));
    }

    public function testASecondSignatureInTheHeaderIsEnough(): void
    {
        $good = hash_hmac('sha256', self::NOW . '.' . self::PAYLOAD, 's');
        $this->assertSame('verified', $this->reason(self::PAYLOAD, 't=' . self::NOW . ',v1=' . str_repeat('0', 64) . ",v1=$good", 's'));
    }

    public function testTheErrorMessageHasNoSecret(): void
    {
        try {
            Webhooks::verify(self::PAYLOAD, 'garbage', 'whsec_synthetic_do_not_print', self::NOW);
            $this->fail('expected WebhookError');
        } catch (WebhookError $e) {
            $this->assertStringNotContainsString('whsec_synthetic_do_not_print', (string) $e);
            $this->assertStringNotContainsString('whsec_synthetic_do_not_print', $e->getTraceAsString());
        }
    }
}
