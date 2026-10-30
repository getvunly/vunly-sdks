using System;
using System.Collections.Generic;

namespace Vunly;

/// <summary>
/// Every failure of a Vunly call. Catch the four subclasses that matter (<see cref="AuthException"/>,
/// <see cref="LimitReachedException"/>, <see cref="InvalidRequestException"/>, <see cref="UnavailableException"/>) or this
/// class; match on <see cref="Code"/>, never on the message. The message never contains your key.
/// A call cancelled through its <see cref="System.Threading.CancellationToken"/> throws <see cref="OperationCanceledException"/>,
/// as every .NET API does; it is the caller's decision, not an outage.
/// </summary>
public class VunlyException : Exception
{
    private static readonly IReadOnlyDictionary<string, object?> NoDetails = new Dictionary<string, object?>();

    internal VunlyException(
        string code,
        int status,
        string message,
        string? requestId,
        IReadOnlyDictionary<string, object?>? details,
        TimeSpan? retryAfter)
        : base(Format(Sanitize.Text(code, Sanitize.MaxCode)!, status, Sanitize.Text(message, Sanitize.MaxMessage)!, Sanitize.Text(requestId, Sanitize.MaxRequestId)))
    {
        // what the server sent is data, not text to trust: control characters (LF, CR, ESC), bidirectional and zero-width
        // characters become spaces and the length is bounded, so a message is safe in a log line or a terminal
        Code = Sanitize.Text(code, Sanitize.MaxCode)!;
        Status = status;
        RequestId = Sanitize.Text(requestId, Sanitize.MaxRequestId);
        Details = details ?? NoDetails;
        RetryAfter = retryAfter;
    }

    /// <summary>
    /// The API's stable machine code ("insufficient_credits", "rate_limited", ...) or one of the SDK's own: "unavailable"
    /// (no answer) and "invalid_response" (a 2xx whose body lacks a promised field).
    /// </summary>
    public string Code { get; }

    /// <summary>The HTTP status, or 0 when there was no response.</summary>
    public int Status { get; }

    /// <summary>The API's Request-Id: quote it in a support request. May be null.</summary>
    public string? RequestId { get; }

    /// <summary>The API's structured detail (for example balance and required on a 402). Never null.</summary>
    public IReadOnlyDictionary<string, object?> Details { get; }

    /// <summary>
    /// The server's Retry-After, read by the rule every Vunly SDK shares and capped at one day; null when it sent none (or
    /// something that is not a number of seconds).
    /// </summary>
    public TimeSpan? RetryAfter { get; }

    private static string Format(string code, int status, string message, string? requestId) =>
        $"vunly: {message} ({(status != 0 ? status + " " : "")}{code}){(string.IsNullOrEmpty(requestId) ? "" : " request " + requestId)}";

    internal static VunlyException Of(
        int status,
        string code,
        string message,
        string? requestId,
        IReadOnlyDictionary<string, object?>? details,
        TimeSpan? retryAfter) =>
        status switch
        {
            402 => new LimitReachedException(code, status, message, requestId, details, retryAfter),
            401 or 403 => new AuthException(code, status, message, requestId, details, retryAfter),
            0 or 429 or >= 500 or (>= 300 and < 400) => new UnavailableException(code, status, message, requestId, details, retryAfter),
            _ => new InvalidRequestException(code, status, message, requestId, details, retryAfter),
        };

    /// <summary>The code the API would send for a status when the error body is missing (a proxy, an empty 5xx).</summary>
    internal static string FallbackCode(int status) =>
        status switch
        {
            401 or 403 => "unauthenticated",
            402 => "insufficient_credits",
            429 => "rate_limited",
            >= 500 => "internal_error",
            _ => "validation_error",
        };

    /// <summary>A 2xx whose body lacks a promised field: unknown outcome, so Unavailable whatever the status.</summary>
    internal static UnavailableException InvalidResponse(int status, string? requestId, string what) =>
        new("invalid_response", status, $"the API answered {status} but the response is malformed ({what})", requestId, null, null);
}

/// <summary>401 or 403: a missing, wrong or revoked key, or a key without the scope. Not retryable.</summary>
public sealed class AuthException : VunlyException
{
    internal AuthException(string code, int status, string message, string? requestId, IReadOnlyDictionary<string, object?>? details, TimeSpan? retryAfter)
        : base(code, status, message, requestId, details, retryAfter)
    {
    }
}

/// <summary>Every other 4xx (400, 404, 409) and any local validation failure: fix the request. Not retryable.</summary>
public sealed class InvalidRequestException : VunlyException
{
    internal InvalidRequestException(string code, int status, string message, string? requestId, IReadOnlyDictionary<string, object?>? details, TimeSpan? retryAfter)
        : base(code, status, message, requestId, details, retryAfter)
    {
    }
}

/// <summary>402: the customer has too few credits. Nothing was debited. Do not run the work. Not retryable.</summary>
public sealed class LimitReachedException : VunlyException
{
    internal LimitReachedException(string code, int status, string message, string? requestId, IReadOnlyDictionary<string, object?>? details, TimeSpan? retryAfter)
        : base(code, status, message, requestId, details, retryAfter)
    {
    }

    /// <summary>The customer's balance reported by the 402, when the API sent it.</summary>
    public long? Balance => Number("balance");

    /// <summary>The cost the refused request needed, when the API sent it.</summary>
    public long? Required => Number("required");

    private long? Number(string key) =>
        Details.TryGetValue(key, out var v)
            ? v switch
            {
                long l => l,
                double d => (long)d,
                _ => null,
            }
            : null;
}

/// <summary>
/// No answer (network, timeout), a 429, a 5xx that survived the SDK's own retries, or a malformed answer
/// (<c>invalid_response</c>). The outcome is unknown or temporary: it is safe to try again later with the SAME id, or to fail
/// closed. A 429 whose details say <c>reason=plan_included_volume</c> is the Free plan's monthly volume stop, not a rate limit
/// (<see cref="IsPlanStop"/>): it is never retried.
/// </summary>
public sealed class UnavailableException : VunlyException
{
    internal UnavailableException(string code, int status, string message, string? requestId, IReadOnlyDictionary<string, object?>? details, TimeSpan? retryAfter)
        : base(code, status, message, requestId, details, retryAfter)
    {
    }

    /// <summary>True for the Free plan's monthly volume stop (never retried; <see cref="VunlyException.RetryAfter"/> is the time to next month).</summary>
    public bool IsPlanStop =>
        Status == 429 && Details.TryGetValue("reason", out var r) && r is string s && s == "plan_included_volume";
}
