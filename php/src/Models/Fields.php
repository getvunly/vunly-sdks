<?php

declare(strict_types=1);

namespace Vunly\Models;

/**
 * Reads typed fields out of a decoded response. A value of the wrong type throws, and the client turns that into
 * `invalid_response`: fail closed, never a zero value that looks like an answer.
 *
 * @internal
 */
final class Fields
{
    /** @param array<string, mixed> $d */
    public static function string(array $d, string $key): string
    {
        $v = $d[$key] ?? null;
        if (!is_string($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not a string");
        }

        return $v;
    }

    /** @param array<string, mixed> $d */
    public static function optString(array $d, string $key): ?string
    {
        $v = $d[$key] ?? null;
        if ($v === null) {
            return null;
        }
        if (!is_string($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not a string");
        }

        return $v;
    }

    /** @param array<string, mixed> $d */
    public static function int(array $d, string $key): int
    {
        $v = $d[$key] ?? null;
        if (!is_int($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not an integer");
        }

        return $v;
    }

    /** @param array<string, mixed> $d */
    public static function optInt(array $d, string $key): ?int
    {
        $v = $d[$key] ?? null;
        if ($v === null) {
            return null;
        }
        if (!is_int($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not an integer");
        }

        return $v;
    }

    /** @param array<string, mixed> $d */
    public static function bool(array $d, string $key): bool
    {
        $v = $d[$key] ?? null;
        if (!is_bool($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not a boolean");
        }

        return $v;
    }

    /**
     * @param array<string, mixed> $d
     *
     * @return array<string, mixed>
     */
    public static function object(array $d, string $key): array
    {
        $v = $d[$key] ?? null;
        if (!is_array($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not an object");
        }
        /** @var array<string, mixed> $v */

        return $v;
    }

    /**
     * @param array<string, mixed> $d
     *
     * @return list<mixed>
     */
    public static function list(array $d, string $key): array
    {
        $v = $d[$key] ?? null;
        if (!is_array($v) || !array_is_list($v)) {
            throw new \UnexpectedValueException("field \"$key\" is not an array");
        }

        return $v;
    }

    /**
     * The objects of a list field.
     *
     * @param array<string, mixed> $d
     *
     * @return list<array<string, mixed>>
     */
    public static function rows(array $d, string $key): array
    {
        $rows = [];
        foreach (self::list($d, $key) as $row) {
            if (!is_array($row)) {
                throw new \UnexpectedValueException('a list item is not an object');
            }
            /** @var array<string, mixed> $row */
            $rows[] = $row;
        }

        return $rows;
    }

    public static function date(string $value): \DateTimeImmutable
    {
        try {
            return new \DateTimeImmutable($value);
        } catch (\Exception) {
            throw new \UnexpectedValueException('a timestamp is not a valid date');
        }
    }

    /** @param array<string, mixed> $d */
    public static function optDate(array $d, string $key): ?\DateTimeImmutable
    {
        $s = self::optString($d, $key);

        return $s === null ? null : self::date($s);
    }

    public static function iso(?\DateTimeImmutable $t): ?string
    {
        return $t?->setTimezone(new \DateTimeZone('UTC'))->format('Y-m-d\TH:i:s.v\Z');
    }
}
