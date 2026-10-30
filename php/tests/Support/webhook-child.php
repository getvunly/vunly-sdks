<?php

declare(strict_types=1);

// Run by HardeningTest under a small memory_limit: a correctly signed webhook whose payload is valid JSON of the shape PHP
// amplifies most. It must end in a WebhookError, not in a fatal error.

require __DIR__ . '/../../vendor/autoload.php';

use Vunly\WebhookError;
use Vunly\Webhooks;

$payload = '{"id":"evt_1","type":"x","data":[' . str_repeat('[1],', 600_000) . '[1]]}';
$t = time();
$header = "t=$t,v1=" . hash_hmac('sha256', "$t.$payload", 'whsec_synthetic_child');
try {
    Webhooks::verify($payload, $header, 'whsec_synthetic_child');
    echo "RESULT accepted\n";
} catch (WebhookError $e) {
    echo 'RESULT ', $e->reason, "\n";
}
