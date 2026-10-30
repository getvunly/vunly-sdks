<?php

declare(strict_types=1);

/*
 * Vunly SDK settings. Publish this file with:  php artisan vendor:publish --tag=vunly-config
 *
 * There is deliberately no default for the key or the address: set both in your environment (.env), never in source.
 * Note that `php artisan config:cache` writes resolved values, the key included, to bootstrap/cache/config.php: keep that
 * file out of version control (a default Laravel .gitignore already does) and out of web-readable paths.
 */

return [
    // The API key (a sandbox key starts with vk_test_). Required.
    'api_key' => env('VUNLY_API_KEY'),

    // The API address, for example https://host or http://127.0.0.1:8787. A trailing slash or /v1 is accepted. Required.
    'base_url' => env('VUNLY_API_URL'),

    // Seconds for ONE attempt (connect, headers and body together).
    'timeout' => (float) env('VUNLY_TIMEOUT', 10),

    // The `vunly` middleware records usage under an id it makes on the server for every request. Set this to true ONLY if you
    // want it to use the caller's X-Request-Id header instead (so a client that repeats a request is not billed twice). The
    // price: whoever repeats the header gets the work again for free, because the API answers "replayed" and charges nothing.
    // Enable it only for callers you authenticate, and see the README.
    'trust_request_id' => filter_var(env('VUNLY_TRUST_REQUEST_ID', false), FILTER_VALIDATE_BOOLEAN),

    // How many times a retryable failure is repeated (0 turns retries off). A retry waits, and PHP blocks while it does.
    'max_retries' => (int) env('VUNLY_MAX_RETRIES', 2),
];
