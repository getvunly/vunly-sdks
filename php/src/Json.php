<?php

declare(strict_types=1);

namespace Vunly;

/**
 * json_decode that cannot take the process down.
 *
 * The transport reads at most {@see MAX_BYTES} of a success answer (64 KB of an error), but that does not by itself protect
 * the decode, which is also checked here: PHP needs 50 to 100 times the text for
 * small nested arrays (`[[1],[1],...]` costs about 400 bytes per `[1]`), and running past memory_limit is a fatal error
 * nothing can catch. So a body is only decoded when a cheap scan of the TEXT, before json_decode runs, says it is small:
 *
 *  - at most {@see MAX_BYTES} (1 MiB; a page of 100 records is a few tens of kilobytes),
 *  - at most {@see MAX_CONTAINERS} arrays and objects, counted as `[` plus `{` (strings that contain them only make the
 *    count higher, which is the safe side),
 *  - and the memory still available under memory_limit must cover a conservative estimate (600 bytes per container, 64
 *    per element or key, 4 per byte of text).
 *
 * Anything else throws, and the caller turns that into `invalid_response`. Nesting depth is bounded by json_decode itself (64).
 *
 * @internal
 */
final class Json
{
    public const MAX_BYTES = 1_048_576;
    public const MAX_CONTAINERS = 20_000;

    private function __construct()
    {
    }

    /**
     * @throws \UnexpectedValueException when the text is too large to decode safely
     * @throws \JsonException            when it is not JSON
     */
    public static function decode(string $raw): mixed
    {
        $bytes = strlen($raw);
        if ($bytes > self::MAX_BYTES) {
            throw new \UnexpectedValueException('the body is larger than ' . (self::MAX_BYTES / 1_048_576) . ' MiB, the most that is decoded');
        }
        $containers = substr_count($raw, '[') + substr_count($raw, '{');
        if ($containers > self::MAX_CONTAINERS) {
            throw new \UnexpectedValueException('the body has more than ' . self::MAX_CONTAINERS . ' arrays and objects, the most that is decoded');
        }
        $elements = $containers + substr_count($raw, ',') + substr_count($raw, ':');
        $limit = self::memoryLimit();
        if ($limit !== null && ($limit - memory_get_usage()) < 600 * $containers + 64 * $elements + 4 * $bytes) {
            throw new \UnexpectedValueException('the body is too large to decode within memory_limit');
        }

        return json_decode($raw, true, 64, JSON_THROW_ON_ERROR);
    }

    /**
     * The members of a JSON object text that are COMPLETE at its start, for a text that was cut short (an error answer read up
     * to its limit). Members are read one by one; a value that is cut off is dropped, except an object, which is read the same
     * way (to depth 3). Every complete value goes through {@see decode}, so the same limits apply. Text that does not start
     * with `{` gives an empty array.
     *
     * @return array<string, mixed>
     */
    public static function objectPrefix(string $raw, int $depth = 0): array
    {
        $n = strlen($raw);
        $i = strspn($raw, " \t\r\n");
        if ($i >= $n || $raw[$i] !== '{') {
            return [];
        }
        $i++;
        $out = [];
        while (true) {
            $i += strspn($raw, " \t\r\n,", $i);
            if ($i >= $n || $raw[$i] !== '"') {
                break;
            }
            $keyStart = $i;
            $keyEnd = self::stringEnd($raw, $i);
            if ($keyEnd === null) {
                break;
            }
            $i = $keyEnd;
            $i += strspn($raw, " \t\r\n", $i);
            if ($i >= $n || $raw[$i] !== ':') {
                break;
            }
            $i++;
            $i += strspn($raw, " \t\r\n", $i);
            if ($i >= $n) {
                break;
            }
            $key = json_decode(substr($raw, $keyStart, $keyEnd - $keyStart), true);
            $end = self::valueEnd($raw, $i);
            if ($end === null) {
                if ($raw[$i] === '{' && $depth < 2 && is_string($key)) {
                    $out[$key] = self::objectPrefix(substr($raw, $i), $depth + 1);
                }
                break;
            }
            try {
                if (is_string($key)) {
                    $out[$key] = self::decode(substr($raw, $i, $end - $i));
                }
            } catch (\Throwable) {
                // A complete member that the decode limits refuse (more than 20,000 containers, nesting beyond 64): an object is
                // read member by member too, so the rest of an error envelope (code, requestId, details.reason) survives a
                // hostile or bloated member. Anything else unreadable is skipped.
                if ($raw[$i] === '{' && $depth < 2 && is_string($key)) {
                    $out[$key] = self::objectPrefix(substr($raw, $i, $end - $i), $depth + 1);
                }
            }
            $i = $end;
        }

        return $out;
    }

    /** Index just after the string that starts at $i (a `"`), or null when it is not closed. */
    private static function stringEnd(string $raw, int $i): ?int
    {
        $n = strlen($raw);
        for ($j = $i + 1; $j < $n; $j++) {
            if ($raw[$j] === '\\') {
                $j++;
            } elseif ($raw[$j] === '"') {
                return $j + 1;
            }
        }

        return null;
    }

    /** Index just after the complete value that starts at $i, or null when the text ends inside it. */
    private static function valueEnd(string $raw, int $i): ?int
    {
        $n = strlen($raw);
        $c = $raw[$i];
        if ($c === '"') {
            return self::stringEnd($raw, $i);
        }
        if ($c === '{' || $c === '[') {
            $depth = 0;
            for ($j = $i; $j < $n; $j++) {
                $d = $raw[$j];
                if ($d === '"') {
                    $e = self::stringEnd($raw, $j);
                    if ($e === null) {
                        return null;
                    }
                    $j = $e - 1;
                } elseif ($d === '{' || $d === '[') {
                    $depth++;
                } elseif ($d === '}' || $d === ']') {
                    if (--$depth === 0) {
                        return $j + 1;
                    }
                }
            }

            return null;
        }
        $len = strcspn($raw, ",}] \t\r\n", $i);

        return $i + $len < $n ? $i + $len : null; // a bare value is only complete when a delimiter follows it
    }

    private static function memoryLimit(): ?int
    {
        $v = trim((string) ini_get('memory_limit'));
        if ($v === '' || $v === '-1') {
            return null;
        }
        $n = (int) $v;

        return match (strtolower(substr($v, -1))) {
            'g' => $n * 1024 ** 3,
            'm' => $n * 1024 ** 2,
            'k' => $n * 1024,
            default => $n,
        };
    }
}
