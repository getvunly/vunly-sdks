<?php

declare(strict_types=1);

use Illuminate\Http\Request;
use Illuminate\Support\Facades\Route;
use Vunly\Laravel\Facades\Vunly;

// POST /generate  {"customer": "acme", "prompt": "hello"}   header X-Request-Id: <your id for this request> (optional)
//
// The `vunly` middleware (registered by the package) works in two steps around the closure below:
//   before: asks Vunly whether the customer may use the feature "generate" (pricing in 100 units of the meter "tokens");
//           a "no", or any failure of the check, means the closure does not run;
//   after:  records the usage once, under an id the SERVER makes for this request, with the quantity the closure set. The
//           caller's X-Request-Id is ignored by default, so a client that repeats a request IS billed again (and cannot get
//           the work for free by repeating a header). Honouring the header is an opt-in with a warning: see the README.
Route::post('/generate', function (Request $request) {
    $prompt = (string) $request->input('prompt', '');
    $tokens = max(1, strlen($prompt) * 10);

    // The work. A real application calls its model here.
    $output = 'echo: ' . $prompt;

    $request->attributes->set('vunly.quantity', $tokens);

    return response()->json(['output' => $output, 'id' => $request->attributes->get('vunly.request_id')]);
})->middleware('vunly:generate,tokens,100');

// GET /balance/{customer}   the facade reaches the same shared client
Route::get('/balance/{customer}', static fn (string $customer) => Vunly::credits()->balance($customer));
