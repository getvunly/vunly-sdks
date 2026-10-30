<?php

declare(strict_types=1);

namespace Vunly;

/**
 * Holds a value (the API key, a customer token) that must never be printed.
 *
 * The value is not stored in a property of the object: it lives in a static WeakMap keyed by the object. So
 * var_dump, print_r, var_export, serialize, json_encode, (array) casts, ReflectionObject and debug_zval_dump all show
 * nothing but the placeholder, and the entry disappears when the object is garbage-collected. Reading it takes a call
 * to reveal(), which only the SDK itself makes.
 */
final class Secret implements \JsonSerializable, \Stringable
{
    public const PLACEHOLDER = '<hidden>';

    /** @var \WeakMap<Secret, string>|null */
    private static ?\WeakMap $values = null;

    public function __construct(#[\SensitiveParameter] string $value)
    {
        self::$values ??= new \WeakMap();
        self::$values[$this] = $value;
    }

    /** The raw value. Never log it. */
    public function reveal(): string
    {
        return self::$values[$this] ?? '';
    }

    public function __toString(): string
    {
        return self::PLACEHOLDER;
    }

    /** @return array<string, string> */
    public function __debugInfo(): array
    {
        return ['value' => self::PLACEHOLDER];
    }

    public function jsonSerialize(): string
    {
        return self::PLACEHOLDER;
    }

    /** @return never */
    public function __serialize(): array
    {
        throw new \LogicException('A Vunly secret cannot be serialized.');
    }

    /** @param array<mixed> $data */
    public function __unserialize(array $data): void
    {
        throw new \LogicException('A Vunly secret cannot be unserialized.');
    }

    /** A copy would have no entry in the map; secrets are shared by reference instead. */
    private function __clone()
    {
    }
}
