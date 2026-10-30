using System;
using System.Collections.Concurrent;
using System.Text.RegularExpressions;
using System.Threading;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Vunly;

// A small web service that meters its own work with Vunly.
//
//   VUNLY_API_URL=http://127.0.0.1:8787 VUNLY_API_KEY=vk_test_... dotnet run
//
// The key and the address come from the environment (or any other configuration source); there is deliberately no default for
// either. Nothing in this application prints them: the client's ToString hides the key, and there is no endpoint that dumps
// configuration.
//
// The id that names a unit of work (the Idempotency-Key) is generated HERE, per request, and never taken from the caller by
// default. Why: the work runs BEFORE usage is recorded. If a caller could choose the id, sending the same X-Request-Id again
// would run the work again and the second record would be a replay: billable work for free. Trusting the caller's header is
// an explicit option (VUNLY_TRUST_CLIENT_REQUEST_ID=true), and then the service must also remember the answer it gave for that
// id and return it without running the work again; this example does that, in memory, for the last 1000 ids.
var builder = WebApplication.CreateBuilder(args);
builder.WebHost.ConfigureKestrel(o => o.Limits.MaxRequestBodySize = 64 * 1024);
builder.Services.AddVunly(o =>
{
    o.ApiKey = builder.Configuration["VUNLY_API_KEY"];
    o.BaseUrl = builder.Configuration["VUNLY_API_URL"];
});

var trustClientRequestId = string.Equals(builder.Configuration["VUNLY_TRUST_CLIENT_REQUEST_ID"], "true", StringComparison.OrdinalIgnoreCase);
var answers = new ConcurrentDictionary<string, object>(); // used only when the caller's id is trusted

var app = builder.Build();

const string Meter = "tokens";
const string Feature = "generate";
const int MaxPrompt = 10_000;
var safeId = new Regex("^[A-Za-z0-9_.:-]{8,128}\\z", RegexOptions.CultureInvariant);
var safeCustomer = new Regex("^[A-Za-z0-9_.:@-]{1,128}\\z", RegexOptions.CultureInvariant);

app.MapPost("/generate", async (GenerateRequest? req, VunlyClient vunly, HttpContext http, CancellationToken ct) =>
{
    // Validate what the caller sent before anything else: a bad request is the caller's error (400), never a server error.
    if (req?.Customer is not { } customer || !safeCustomer.IsMatch(customer))
    {
        return Results.Json(new { error = "invalid_request", field = "customer" }, statusCode: StatusCodes.Status400BadRequest);
    }
    if (req.Prompt is not { Length: > 0 and <= MaxPrompt } prompt)
    {
        return Results.Json(new { error = "invalid_request", field = "prompt" }, statusCode: StatusCodes.Status400BadRequest);
    }

    string id;
    if (trustClientRequestId && http.Request.Headers.TryGetValue("X-Request-Id", out var header) && header.Count > 0)
    {
        id = header.ToString();
        if (!safeId.IsMatch(id))
        {
            return Results.Json(new { error = "invalid_request", field = "X-Request-Id" }, statusCode: StatusCodes.Status400BadRequest);
        }
        if (answers.TryGetValue(customer + "|" + id, out var earlier))
        {
            return Results.Ok(earlier); // the same unit of work again: the earlier answer, and the work does not run twice
        }
    }
    else
    {
        id = "req_" + Guid.NewGuid().ToString("N");
    }
    var tokens = Math.Max(1, prompt.Length * 10L);

    try
    {
        // 1. Ask first. A "no", and an error, both mean: do not run the work.
        var decision = await vunly.Entitlements.CheckAsync(customer, Feature, new Estimate(Meter, tokens), ct);
        if (!decision.Allowed)
        {
            return Results.Json(new { error = "not_allowed", reason = decision.Reason }, statusCode: StatusCodes.Status403Forbidden);
        }

        // 2. The work. A real application calls its model here.
        var output = "echo: " + prompt;

        // 3. Record it once. A LimitReachedException means the credits were not there when it counted (the check is only
        // advisory); nothing was debited.
        var usage = await vunly.Usage.RecordAsync(id, customer, Meter, tokens, ct);
        var answer = new { output, id, creditsRemaining = usage.Remaining, replayed = usage.Replayed };
        if (trustClientRequestId)
        {
            if (answers.Count >= 1000)
            {
                answers.Clear(); // a bounded memory is enough for an example
            }
            answers[customer + "|" + id] = answer;
        }
        return Results.Ok(answer);
    }
    catch (LimitReachedException e)
    {
        return Results.Json(new { error = "insufficient_credits", requestId = e.RequestId }, statusCode: StatusCodes.Status402PaymentRequired);
    }
    catch (InvalidRequestException e) when (e.Status == 404)
    {
        // the customer does not exist (create it with Customers.UpsertAsync first)
        return Results.Json(new { error = "unknown_customer", requestId = e.RequestId }, statusCode: StatusCodes.Status404NotFound);
    }
    catch (InvalidRequestException e)
    {
        return Results.Json(new { error = "invalid_request", code = e.Code, requestId = e.RequestId }, statusCode: StatusCodes.Status400BadRequest);
    }
    catch (AuthException e)
    {
        // This service's own key is wrong, revoked or lacks a scope: not the caller's fault and not something to tell them
        // about. Fail closed; the operator reads the Request-Id in Vunly. The message and the key are never returned.
        app.Logger.LogWarning("Vunly refused this service's key: {Code} request {RequestId}", e.Code, e.RequestId);
        return Results.Json(new { error = "billing_misconfigured" }, statusCode: StatusCodes.Status502BadGateway);
    }
    catch (UnavailableException e)
    {
        // Fail closed: when Vunly cannot be reached the work is not done, and the caller may retry.
        return Results.Json(new { error = "billing_unavailable", code = e.Code }, statusCode: StatusCodes.Status503ServiceUnavailable);
    }
    catch (VunlyException e)
    {
        app.Logger.LogWarning("Vunly call failed: {Code} request {RequestId}", e.Code, e.RequestId);
        return Results.Json(new { error = "billing_error" }, statusCode: StatusCodes.Status502BadGateway);
    }
});

app.Run();

internal sealed record GenerateRequest(string? Customer, string? Prompt);
