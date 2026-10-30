<?php

declare(strict_types=1);

// Run by HardeningTest in a child process with a small memory_limit: one call against the scripted server, and what happened.
// A fatal "Allowed memory size exhausted" would end the process without printing a line starting with RESULT.

require __DIR__ . '/../../vendor/autoload.php';

use Vunly\Client;
use Vunly\VunlyError;

[, $url] = $argv;
$call = $argv[2] ?? 'balance';
$client = new Client(apiKey: 'vk_test_synthetic_php_child_000001', baseUrl: $url, maxRetries: 0, timeout: 20);
try {
    if ($call === 'list') {
        $client->customers->list();
    } else {
        $client->credits->balance('acme');
    }
    echo "RESULT success\n";
} catch (VunlyError $e) {
    echo 'RESULT ', $e::class, ' ', $e->errorCode, ' ', $e->status, "\n";
}
echo 'PEAK ', memory_get_peak_usage(), "\n";
