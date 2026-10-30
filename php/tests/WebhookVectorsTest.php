<?php

declare(strict_types=1);

namespace Vunly\Tests;

use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;
use Vunly\WebhookError;
use Vunly\Webhooks;

/**
 * The shared webhook vectors (webhook-vectors.json, an identical copy in every SDK; a repository check keeps the copies
 * identical). The signatures were computed outside any SDK. "expect" is "ok", a WebhookError reason, or "config" (an
 * InvalidArgumentException raised before the signature is looked at). A null header is the empty string here: the header
 * parameter is a string, and both mean a missing header.
 */
final class WebhookVectorsTest extends TestCase
{
    /** @return array{now: int, vectors: list<array{name: string, payload: string, header: ?string, secrets: list<string>, expect: string}>} */
    private static function load(): array
    {
        /** @var array{now: int, vectors: list<array{name: string, payload: string, header: ?string, secrets: list<string>, expect: string}>} $doc */
        $doc = json_decode((string) file_get_contents(__DIR__ . '/webhook-vectors.json'), true, 32, JSON_THROW_ON_ERROR);

        return $doc;
    }

    public static function vectors(): iterable
    {
        foreach (self::load()['vectors'] as $i => $v) {
            yield "#$i " . $v['name'] => [$i];
        }
    }

    public function testThereAreEnoughVectors(): void
    {
        $this->assertGreaterThanOrEqual(20, count(self::load()['vectors']));
    }

    #[DataProvider('vectors')]
    public function testEveryVectorGetsItsAnswer(int $index): void
    {
        $doc = self::load();
        $v = $doc['vectors'][$index];
        $got = 'ok';
        try {
            Webhooks::verify($v['payload'], $v['header'] ?? '', $v['secrets'], $doc['now']);
        } catch (WebhookError $e) {
            $got = $e->reason;
        } catch (\InvalidArgumentException) {
            $got = 'config';
        }
        $this->assertSame($v['expect'], $got, $v['name']);
    }
}
