<?php

declare(strict_types=1);

namespace Vunly\Http;

/**
 * The default transport: PHP's cURL extension, nothing else.
 *
 * What it guarantees, each with a test:
 *  - One attempt is bounded as a WHOLE by CURLOPT_TIMEOUT_MS (connecting, waiting for headers and reading the body),
 *    so a server that answers at once and then drips its body is cut at the timeout.
 *  - The body is capped while it streams (CURLOPT_WRITEFUNCTION refuses the chunk that would cross the cap), so at most
 *    the cap is ever held in memory.
 *  - libcurl never repeats a request behind the SDK's back: every attempt uses a fresh connection and forbids reuse, so
 *    libcurl's "the reused connection died, try again" path cannot run. Only the SDK retries, by operation class.
 *  - Redirects are not followed (the Authorization header would go to another host), HTTP/1.1 is forced (no upgrade or
 *    HTTP/2 prior knowledge towards a cleartext local API), and `Expect: 100-continue` is switched off.
 *  - What is NOT bounded by a size, said plainly: response HEADERS have no cap of ours (libcurl 8.14.1, the pinned image, refuses
 *    a header line over 100 KB and more than 300 KB of headers in all, and a canary test pins that), and an endless chunked
 *    body whose chunk extensions never end has no libcurl size option either. For that the only bound is CURLOPT_TIMEOUT_MS
 *    above; memory stays flat meanwhile (measured: 186 MB read in 5 s with a resident size of 25 MB), because nothing of
 *    the extension is kept. Do not read the body cap as a bound on those two.
 *  - The cURL error text (which can contain the address) is dropped: the result is a {@see Failure} and nothing else.
 */
final class CurlTransport implements Transport
{
    public function __construct()
    {
        if (!extension_loaded('curl')) {
            throw new \RuntimeException('The Vunly PHP SDK needs the cURL extension (ext-curl).');
        }
    }

    public function send(Request $request): Response
    {
        $status = 0;
        /** @var array<string, string> $headers */
        $headers = [];
        // ONE buffer, no second copy: the caps are small (1 MiB for an answer, 64 KB for an error), so growing it is cheap, and
        // nothing is read that Json::decode would refuse anyway.
        $body = '';
        $tooLarge = false;
        $cap = $request->maxBodyBytes;
        $errorCap = min($cap, $request->maxErrorBodyBytes);

        $onHeader = static function ($ch, string $line) use (&$status, &$headers): int {
            $length = strlen($line);
            $trimmed = trim($line);
            if (str_starts_with($trimmed, 'HTTP/')) {
                // a new status line (an interim 1xx or the final answer): start the header list again
                $parts = explode(' ', $trimmed, 3);
                $status = (int) ($parts[1] ?? 0);
                $headers = [];
            } elseif (($pos = strpos($trimmed, ':')) !== false) {
                $name = strtolower(trim(substr($trimmed, 0, $pos)));
                $value = trim(substr($trimmed, $pos + 1));
                $headers[$name] = isset($headers[$name]) ? $headers[$name] . ', ' . $value : $value;
            }

            return $length;
        };
        $onBody = static function ($ch, string $chunk) use (&$body, &$status, &$tooLarge, $cap, $errorCap): int {
            $length = strlen($chunk);
            // an error answer only needs its status (and a short JSON error): a much lower cap than a success
            $limit = $status >= 200 && $status < 300 ? $cap : $errorCap;
            if (strlen($body) + $length > $limit) {
                $tooLarge = true;
                if ($limit === $errorCap && !($status >= 200 && $status < 300)) {
                    // an error answer keeps its first bytes: the error envelope is at the start, and a 429's details
                    // (plan_included_volume) must survive padding after it
                    $body .= substr($chunk, 0, $limit - strlen($body));
                }

                return 0; // anything but $length makes cURL stop the transfer
            }
            $body .= $chunk;

            return $length;
        };

        $lines = [];
        foreach ($request->headers as $name => $value) {
            $lines[] = $name . ': ' . $value;
        }
        $lines[] = 'Expect:'; // no 100-continue round trip

        $ms = max(1, (int) ceil($request->timeoutSeconds * 1000));
        $ch = curl_init();
        // @phpstan-ignore argument.type (the stub wants non-empty strings for the URL and method, which the caller guarantees)
        curl_setopt_array($ch, [
            CURLOPT_URL => $request->url,
            CURLOPT_CUSTOMREQUEST => $request->method,
            CURLOPT_HTTPHEADER => $lines,
            CURLOPT_TIMEOUT_MS => $ms,
            CURLOPT_CONNECTTIMEOUT_MS => $ms,
            CURLOPT_NOSIGNAL => true,
            CURLOPT_FOLLOWLOCATION => false,
            CURLOPT_MAXREDIRS => 0,
            CURLOPT_FRESH_CONNECT => true,
            CURLOPT_FORBID_REUSE => true,
            CURLOPT_HTTP_VERSION => CURL_HTTP_VERSION_1_1,
            CURLOPT_HEADERFUNCTION => $onHeader,
            CURLOPT_WRITEFUNCTION => $onBody,
        ]);
        if ($request->body !== null) {
            curl_setopt($ch, CURLOPT_POSTFIELDS, $request->body);
        }

        curl_exec($ch);
        $errno = curl_errno($ch);
        // the handle is released when it goes out of scope (curl_close() is deprecated since PHP 8.5)
        unset($ch);

        if ($tooLarge) {
            // a success that is too large keeps nothing; an error keeps the first bytes it was allowed
            return new Response($status, $headers, $status >= 200 && $status < 300 ? '' : $body, Failure::TooLarge);
        }
        if ($errno === CURLE_OPERATION_TIMEDOUT) {
            return Response::failed(Failure::Timeout);
        }
        if ($errno !== 0 || $status === 0) {
            // refused, reset, a body that broke half way (partial file, receive error), TLS, DNS: no complete answer
            return Response::failed(Failure::Connection);
        }

        return new Response($status, $headers, $body);
    }
}
