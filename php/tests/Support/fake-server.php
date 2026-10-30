<?php

declare(strict_types=1);

/*
 * A scripted HTTP server for the tests (run by FakeServer as a separate process; PHP cannot serve and call itself).
 *
 *   php fake-server.php <script.json> <log.jsonl> <port-file>
 *
 * Connection n gets action n of the script (a JSON list); after the list, every connection gets a 599 so a test notices an
 * attempt it did not expect. Every request is logged as one JSON line {method, path, headers, body} BEFORE the action
 * runs, so a dropped connection is still counted. Actions:
 *
 *   {"respond": {"status": 200, "headers": {...}, "json": {...} | "body": "text"}}
 *   {"drop": true}                      read the request, close without answering (the server "did the work")
 *   {"hang": 2.0}                       read the request, then say nothing for that many seconds
 *   {"drip": {"status": 200, "bytes": 200, "intervalMs": 100, "chunk": 5}}
 *                                       answer at once, then send the body slowly
 *   {"huge": {"status": 200, "bytes": 40000000}}
 *                                       answer with a body this large, written in 64 KB chunks
 *   {"truncate": {"status": 200}}       promise 1000 bytes, send 10, close
 *   {"redirect": "http://elsewhere/"}   302 with a Location header
 */

[, $scriptFile, $logFile, $portFile] = $argv;
/** @var list<array<string, mixed>> $script */
$script = json_decode((string) file_get_contents($scriptFile), true, 64, JSON_THROW_ON_ERROR);

$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
if ($server === false) {
    fwrite(STDERR, "cannot listen: $errstr\n");
    exit(1);
}
$name = (string) stream_socket_get_name($server, false);
file_put_contents($portFile, (string) substr($name, (int) strrpos($name, ':') + 1));

$reasons = [200 => 'OK', 201 => 'Created', 302 => 'Found', 400 => 'Bad Request', 401 => 'Unauthorized', 402 => 'Payment Required', 403 => 'Forbidden', 404 => 'Not Found', 409 => 'Conflict', 429 => 'Too Many Requests', 500 => 'Internal Server Error', 503 => 'Service Unavailable', 599 => 'Unexpected'];

function readRequest($conn): ?array
{
    $head = '';
    while (!str_contains($head, "\r\n\r\n")) {
        $chunk = fread($conn, 8192);
        if ($chunk === '' || $chunk === false) {
            return null;
        }
        $head .= $chunk;
    }
    [$headText, $rest] = explode("\r\n\r\n", $head, 2);
    $lines = explode("\r\n", $headText);
    [$method, $target] = explode(' ', array_shift($lines));
    $headers = [];
    foreach ($lines as $l) {
        [$k, $v] = explode(':', $l, 2) + [1 => ''];
        $headers[strtolower(trim($k))] = trim($v);
    }
    $want = (int) ($headers['content-length'] ?? 0);
    $body = $rest;
    while (strlen($body) < $want) {
        $chunk = fread($conn, 8192);
        if ($chunk === '' || $chunk === false) {
            break;
        }
        $body .= $chunk;
    }

    return ['method' => $method, 'path' => $target, 'headers' => $headers, 'body' => $body];
}

function head(int $status, array $headers, array $reasons, ?int $length): string
{
    $out = "HTTP/1.1 $status " . ($reasons[$status] ?? 'Status') . "\r\n";
    $headers += ['Connection' => 'close'];
    if ($length !== null) {
        $headers['Content-Length'] = (string) $length;
    }
    foreach ($headers as $k => $v) {
        $out .= "$k: $v\r\n";
    }

    return $out . "\r\n";
}

for ($n = 0;; $n++) {
    $conn = @stream_socket_accept($server, -1);
    if ($conn === false) {
        continue;
    }
    $req = readRequest($conn);
    if ($req === null) {
        fclose($conn);
        continue;
    }
    file_put_contents($logFile, json_encode($req, JSON_UNESCAPED_SLASHES) . "\n", FILE_APPEND | LOCK_EX);
    $action = $script[$n] ?? ['respond' => ['status' => 599, 'json' => ['error' => ['code' => 'unexpected_attempt', 'message' => 'the script has no action for this attempt']]]];

    if (isset($action['respond'])) {
        $r = $action['respond'];
        $body = array_key_exists('json', $r) ? json_encode($r['json']) : (string) ($r['body'] ?? '');
        $headers = ($r['headers'] ?? []) + (array_key_exists('json', $r) ? ['Content-Type' => 'application/json'] : []);
        @fwrite($conn, head($r['status'] ?? 200, $headers, $reasons, strlen($body)) . $body);
    } elseif (isset($action['bigheaders'])) {
        // a valid, small answer whose HEADERS are hostile: one header of `size` bytes, repeated `count` times
        $h = $action['bigheaders'];
        $body = '{"credits":1,"consumed":0,"granted":1}';
        $raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: " . strlen($body) . "\r\n";
        for ($k = 0; $k < $h['count']; $k++) {
            $raw .= "X-Pad-$k: " . str_repeat('h', $h['size']) . "\r\n";
        }
        @fwrite($conn, $raw . "\r\n" . $body);
    } elseif (isset($action['drop'])) {
        // nothing
    } elseif (isset($action['hang'])) {
        usleep((int) ($action['hang'] * 1_000_000));
    } elseif (isset($action['drip'])) {
        $d = $action['drip'];
        @fwrite($conn, head($d['status'] ?? 200, ['Content-Type' => 'application/json'], $reasons, null));
        for ($sent = 0; $sent < $d['bytes']; $sent += $d['chunk']) {
            if (@fwrite($conn, str_repeat(' ', $d['chunk'])) === false) {
                break;
            }
            usleep($d['intervalMs'] * 1000);
        }
    } elseif (isset($action['huge'])) {
        $h = $action['huge'];
        @fwrite($conn, head($h['status'] ?? 200, ['Content-Type' => 'application/json'], $reasons, (int) $h['bytes']));
        $chunk = str_repeat('x', 65536);
        for ($sent = 0; $sent < $h['bytes']; $sent += 65536) {
            if (@fwrite($conn, $chunk) === false) {
                break;
            }
        }
    } elseif (isset($action['truncate'])) {
        @fwrite($conn, head($action['truncate']['status'] ?? 200, ['Content-Type' => 'application/json'], $reasons, 1000) . '{"half":tru');
    } elseif (isset($action['redirect'])) {
        @fwrite($conn, head(302, ['Location' => $action['redirect']], $reasons, 0));
    }
    @fclose($conn);
}
