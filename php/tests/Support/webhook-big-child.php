<?php

declare(strict_types=1);

// Run by HardeningTest under a small memory_limit: a correctly signed webhook with a 14 MB body. The receiver holds the body
// once; verifying it must not need a second copy.

require __DIR__ . '/../../vendor/autoload.php';

use Vunly\WebhookError;
use Vunly\Webhooks;

$payload = str_repeat('x', 14_000_000);
$t = time();
$ctx = hash_init('sha256', HASH_HMAC, 'whsec_synthetic_child');
hash_update($ctx, "$t.");
hash_update($ctx, $payload);
$header = "t=$t,v1=" . hash_final($ctx);
try {
    Webhooks::verify($payload, $header, 'whsec_synthetic_child');
    echo "RESULT accepted\n";
} catch (WebhookError $e) {
    echo 'RESULT ', $e->reason, "\n";
}
