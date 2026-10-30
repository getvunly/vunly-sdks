using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;

namespace Vunly;

/// <summary>HTTP, retries by operation class, and error mapping. conformance/README.md.</summary>
internal sealed class Transport
{
    /// <summary>The operation class of the contract.</summary>
    internal enum OpClass
    {
        /// <summary>Reads and the entitlement check.</summary>
        R,

        /// <summary>Keyed mutations (the same id on a retry).</summary>
        K,

        /// <summary>Naturally idempotent writes: upserts, deletes, subscribing to the same plan.</summary>
        N,

        /// <summary>Everything else: never repeated after an ambiguous failure, only after a 429.</summary>
        X,
    }

    internal sealed record Request(
        HttpMethod Method,
        string Path,
        IReadOnlyDictionary<string, string>? Query,
        IReadOnlyDictionary<string, object?>? Body,
        string? Idem,
        OpClass Class);

    internal sealed record Meta(int Status, string? RequestId, bool Replayed, bool PlanOverage);

    /// <summary>
    /// The most a successful answer may be: 1 MiB. Real answers are tens of KB (a page of 100 customers is about 33 KB, a page of
    /// 100 ledger entries about 53 KB). The cap is also the memory bound: System.Text.Json needs about 12 bytes per token plus
    /// the copy of the body, so a 16 MB body of tiny nested values (<c>[[1],[1],...]</c>) made 250+ MB of heap. Over the cap a 2xx
    /// is <c>invalid_response</c>, and reading stops one chunk past it.
    /// </summary>
    internal const int MaxBody = 1 << 20;

    /// <summary>
    /// An error answer from this API is a few hundred bytes. At most this much is read and parsed; a bigger one is not
    /// buffered further and its status classifies it.
    /// </summary>
    internal const int MaxErrorBodyToParse = 64 << 10;

    /// <summary>Nesting a body may have: the API's answers are three or four levels deep. System.Text.Json's own default is 64.</summary>
    internal static readonly System.Text.Json.JsonDocumentOptions JsonLimits = new() { MaxDepth = 32 };

    // \z, not $: in .NET "$" also matches before a final "\n", so "job-12345678\n" would pass and a header with an LF in it
    // would be sent as a different key from "job-12345678"
    private static readonly System.Text.RegularExpressions.Regex IdPattern = new("^[A-Za-z0-9_.:-]{8,128}\\z", System.Text.RegularExpressions.RegexOptions.CultureInvariant);

    private readonly HttpClient _http;
    private readonly string _baseUrl;
    private readonly Uri _baseUri;
    private readonly string _apiKey;
    private readonly TimeSpan _timeout;
    private readonly int _maxRetries;
    private readonly string _userAgent;

    /// <summary>Waits between attempts. Replaced in tests.</summary>
    internal Func<TimeSpan, CancellationToken, Task> Delay { get; init; } = (d, ct) => Task.Delay(d, ct);

    internal Transport(HttpClient http, string baseUrl, Uri baseUri, string apiKey, TimeSpan timeout, int maxRetries)
    {
        _http = http;
        _baseUrl = baseUrl;
        _baseUri = baseUri;
        _apiKey = apiKey;
        _timeout = timeout;
        _maxRetries = maxRetries;
        _userAgent = $"vunly-dotnet/{VunlyClient.Version} ({RuntimeInformation.FrameworkDescription}; {RuntimeInformation.OSArchitecture})";
    }

    /// <summary>Calls, retrying as the class allows, and parses a 2xx body with <paramref name="parse"/>.</summary>
    internal async Task<(T Value, Meta Meta)> SendAsync<T>(Request r, Func<Fields, T> parse, CancellationToken ct)
    {
        var (value, meta) = await SendCoreAsync(r, f => parse(f), ct).ConfigureAwait(false);
        return (value!, meta);
    }

    /// <summary>As <see cref="SendAsync{T}"/> for a call whose success body is not read.</summary>
    internal async Task<Meta> SendAsync(Request r, CancellationToken ct) =>
        (await SendCoreAsync<object?>(r, null, ct).ConfigureAwait(false)).Meta;

    private async Task<(T? Value, Meta Meta)> SendCoreAsync<T>(Request r, Func<Fields, T>? parse, CancellationToken ct)
    {
        // A keyed call (class K) has no meaning without its id: null is as invalid as a short one.
        if ((r.Idem is null && r.Class == OpClass.K) || (r.Idem is not null && !IdPattern.IsMatch(r.Idem)))
        {
            throw VunlyException.Of(
                400,
                "validation_error",
                "id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)",
                null,
                new Dictionary<string, object?> { ["fields"] = new Dictionary<string, object?> { ["id"] = "8-128 chars of A-Za-z0-9_.:-" } },
                null);
        }
        var payload = r.Body is null ? null : JsonBody.Write(r.Body);
        var target = _baseUrl + r.Path + QueryString(r.Query);
        for (var attempt = 1; ; attempt++)
        {
            try
            {
                return await OnceAsync(r, target, payload, parse, ct).ConfigureAwait(false);
            }
            catch (UnavailableException e) when (attempt <= _maxRetries && MayRetry(r.Class, e) && RetryDelay(attempt, e.RetryAfter) is not null)
            {
                await Delay(RetryDelay(attempt, e.RetryAfter)!.Value, ct).ConfigureAwait(false);
            }
        }
    }

    /// <summary>
    /// R, K and N are repeated after a lost answer, a timeout or a 5xx; X only after a 429 (refused before it ran). The
    /// plan-volume stop is never repeated.
    /// </summary>
    private static bool MayRetry(OpClass cls, UnavailableException e)
    {
        if (e.Status is not (0 or 429 or 500 or 502 or 503 or 504))
        {
            return false; // invalid_response and anything else
        }
        if (e.IsPlanStop || e.Code == "unexpected_redirect")
        {
            return false; // whatever status the other host answered with: a wrong address is not a transient failure
        }
        return !(cls == OpClass.X && e.Status != 429);
    }

    /// <summary>Null when the server asked for a wait longer than we will wait: the caller decides.</summary>
    internal static TimeSpan? RetryDelay(int attempt, TimeSpan? retryAfter)
    {
        if (retryAfter is { } ra && ra > TimeSpan.Zero)
        {
            return ra <= RetryAfterHeader.Cap ? ra : null;
        }
        var baseMs = Math.Min(8000d, 250d * Math.Pow(2, attempt - 1));
        return TimeSpan.FromMilliseconds(baseMs * (0.5 + (Random.Shared.NextDouble() / 2)));
    }

    private async Task<(T? Value, Meta Meta)> OnceAsync<T>(Request r, string target, byte[]? payload, Func<Fields, T>? parse, CancellationToken ct)
    {
        using var attemptCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        attemptCts.CancelAfter(_timeout);
        using var req = new HttpRequestMessage(r.Method, target) { Version = System.Net.HttpVersion.Version11 };
        req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + _apiKey);
        req.Headers.TryAddWithoutValidation("User-Agent", _userAgent);
        req.Headers.TryAddWithoutValidation("Accept", "application/json");
        if (r.Idem is not null)
        {
            req.Headers.TryAddWithoutValidation("Idempotency-Key", r.Idem);
        }
        if (payload is not null)
        {
            req.Content = new ByteArrayContent(payload);
            req.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json");
        }
        else if (_maxRetries == 0)
        {
            // MaxRetries = 0 means ONE request on the wire. SocketsHttpHandler repeats a request that has no body (a GET, a
            // DELETE) up to 3 times by itself when the connection closes before an answer: 4 requests behind the caller's
            // back. It does not repeat one that carries a body, so an empty body (Content-Length: 0) turns that off. With
            // retries on, the handler's own replay stays: reads are safe to repeat, and it is bounded by the attempt timeout.
            req.Content = new ByteArrayContent(Array.Empty<byte>());
        }

        HttpResponseMessage res;
        byte[] raw;
        try
        {
            res = await _http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, attemptCts.Token).ConfigureAwait(false);
            using (res)
            {
                if (res.RequestMessage?.RequestUri is { } answeredBy && !SameOrigin(answeredBy, _baseUri))
                {
                    // a caller-supplied handler followed a redirect: the answer is from a host nobody configured
                    throw new UnavailableException("unexpected_redirect", (int)res.StatusCode, "the answer came from a different host than the configured BaseUrl (a redirect was followed by the HttpClient you supplied); not used", null, null, null);
                }
                var success = (int)res.StatusCode is >= 200 and < 300;
                raw = await ReadCappedAsync(res, success ? MaxBody : MaxErrorBodyToParse, attemptCts.Token).ConfigureAwait(false);
                return Interpret(res, raw, parse);
            }
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        {
            // our own per-attempt timeout (the caller did not cancel)
            throw new UnavailableException("unavailable", 0, $"the Vunly API did not answer within {_timeout}", null, null, null);
        }
        catch (HttpRequestException)
        {
            // Deliberately drop the underlying exception: some transports put the URL or headers in the message.
            throw new UnavailableException("unavailable", 0, "could not reach the Vunly API", null, null, null);
        }
        catch (IOException)
        {
            throw new UnavailableException("unavailable", 0, "could not reach the Vunly API", null, null, null);
        }
    }

    private static bool SameOrigin(Uri a, Uri b) =>
        string.Equals(a.Scheme, b.Scheme, StringComparison.OrdinalIgnoreCase)
        && string.Equals(a.IdnHost, b.IdnHost, StringComparison.OrdinalIgnoreCase)
        && a.Port == b.Port;

    private static async Task<byte[]> ReadCappedAsync(HttpResponseMessage res, int cap, CancellationToken ct)
    {
        await using var stream = await res.Content.ReadAsStreamAsync(ct).ConfigureAwait(false);
        using var buffer = new MemoryStream();
        var chunk = new byte[16 * 1024];
        int n;
        while ((n = await stream.ReadAsync(chunk.AsMemory(), ct).ConfigureAwait(false)) > 0)
        {
            buffer.Write(chunk, 0, n);
            if (buffer.Length > cap)
            {
                return buffer.ToArray(); // over the cap: stop reading (at most cap plus one chunk is ever held)
            }
        }
        return buffer.ToArray();
    }

    private (T? Value, Meta Meta) Interpret<T>(HttpResponseMessage res, byte[] raw, Func<Fields, T>? parse)
    {
        var status = (int)res.StatusCode;
        var requestId = Header(res, "Request-Id");
        var meta = new Meta(status, requestId, Header(res, "Idempotent-Replayed") == "true", Header(res, "Vunly-Plan-Overage") == "true");
        if (status is >= 200 and < 300)
        {
            if (parse is null)
            {
                return (default, meta);
            }
            if (raw.Length > MaxBody)
            {
                throw VunlyException.InvalidResponse(status, requestId, "the body is too large");
            }
            try
            {
                using var doc = JsonDocument.Parse(raw, JsonLimits);
                return (parse(Fields.Of(doc.RootElement)), meta);
            }
            catch (JsonException)
            {
                throw VunlyException.InvalidResponse(status, requestId, "the body is not JSON");
            }
            catch (InvalidOperationException)
            {
                // JsonElement.GetString on a lone surrogate ("\ud800") or invalid UTF-8
                throw VunlyException.InvalidResponse(status, requestId, "the body has a string that is not valid text");
            }
            catch (FormatException e)
            {
                throw VunlyException.InvalidResponse(status, requestId, e.Message);
            }
        }
        if (status is >= 300 and < 400)
        {
            // The SDK never follows a redirect (the key would travel to another host) and a 3xx is a wrong address, not a bad
            // request: Unavailable, not retried. Neither the Location nor the body is used.
            throw new UnavailableException(
                "unexpected_redirect",
                status,
                $"the API answered {status} (a redirect); check BaseUrl: it should be the API address itself (for example https://host), and redirects are never followed",
                requestId,
                null,
                null);
        }
        throw ErrorFromResponse(res, requestId, raw);
    }

    private static string? Header(HttpResponseMessage res, string name) =>
        res.Headers.TryGetValues(name, out var v) ? string.Join(", ", v) : null;

    private static VunlyException ErrorFromResponse(HttpResponseMessage res, string? requestId, byte[] raw)
    {
        var status = (int)res.StatusCode;
        string? code = null;
        string? message = null;
        Dictionary<string, object?>? details = null;
        try
        {
            // an oversized error body is not parsed (nothing legitimate is that big): the status classifies the failure
            using var doc = JsonDocument.Parse(raw.Length <= MaxErrorBodyToParse ? raw : ReadOnlyMemory<byte>.Empty, JsonLimits);
            if (doc.RootElement.ValueKind == JsonValueKind.Object
                && doc.RootElement.TryGetProperty("error", out var err)
                && err.ValueKind == JsonValueKind.Object)
            {
                code = err.TryGetProperty("code", out var c) && c.ValueKind == JsonValueKind.String ? c.GetString() : null;
                message = err.TryGetProperty("message", out var m) && m.ValueKind == JsonValueKind.String ? m.GetString() : null;
                if (err.TryGetProperty("requestId", out var rid) && rid.ValueKind == JsonValueKind.String && !string.IsNullOrEmpty(rid.GetString()))
                {
                    requestId = rid.GetString();
                }
                if (err.TryGetProperty("details", out var d) && d.ValueKind == JsonValueKind.Object)
                {
                    details = Fields.ToObject(d) as Dictionary<string, object?>;
                }
            }
        }
        catch (Exception e) when (e is JsonException or InvalidOperationException or FormatException)
        {
            // (InvalidOperationException: a lone surrogate or invalid UTF-8 in a string; FormatException: a number that is no double)
            // an error with no JSON body (a proxy, an empty 5xx) is still classified by its status
        }
        if (string.IsNullOrEmpty(code))
        {
            code = VunlyException.FallbackCode(status);
        }
        if (string.IsNullOrEmpty(message))
        {
            message = $"request failed with status {status}";
        }
        if (status == 404 && message.Contains("no such route", StringComparison.OrdinalIgnoreCase))
        {
            message += " (check the API address you configured: it should be the address only, for example https://host; a trailing /v1 is accepted)";
        }
        // The RAW header: HttpResponseHeaders.RetryAfter rejects what this contract calls a very long wait (an integer above
        // Int32, 1e12, Infinity) and would lose it. NonValidated holds the string exactly as the server sent it.
        TimeSpan? retryAfter = null;
        if (((IReadOnlyDictionary<string, HeaderStringValues>)res.Headers.NonValidated).TryGetValue("Retry-After", out var rawRetryAfter))
        {
            retryAfter = RetryAfterHeader.Parse(rawRetryAfter.ToString()).Value; // two header lines join with ", " and are not a number
        }
        return VunlyException.Of(status, code, message, requestId, details, retryAfter);
    }

    private static string QueryString(IReadOnlyDictionary<string, string>? query)
    {
        if (query is null || query.Count == 0)
        {
            return "";
        }
        var parts = new List<string>();
        foreach (var kv in query)
        {
            parts.Add(Uri.EscapeDataString(kv.Key) + "=" + Uri.EscapeDataString(kv.Value));
        }
        return "?" + string.Join("&", parts);
    }

    internal static Request Get(string path, IReadOnlyDictionary<string, string>? query) =>
        new(HttpMethod.Get, path, query, null, null, OpClass.R);

    /// <summary>Percent-encodes one path segment, so a customer id cannot change the route.</summary>
    /// <remarks>
    /// An empty id, "." and ".." are refused before any request: they are not escaped (a dot is unreserved), and a URL path
    /// treats them as "nothing" and "up one level", so GetAsync("..") would have sent GET /v1/ and an empty id GET /v1/customers/
    /// (or DELETE /v1/customer-tokens/).
    /// </remarks>
    internal static string Segment(string? s, string field)
    {
        if (string.IsNullOrEmpty(s) || s is "." or "..")
        {
            throw VunlyException.Of(
                400,
                "validation_error",
                $"{field} must not be empty, \".\" or \"..\"",
                null,
                new Dictionary<string, object?> { ["fields"] = new Dictionary<string, object?> { [field] = "not empty, not . or .." } },
                null);
        }
        return Uri.EscapeDataString(s);
    }

    internal static IReadOnlyDictionary<string, string>? Query(ListOptions? o)
    {
        var q = new Dictionary<string, string>();
        if (o is not null)
        {
            if (o.Limit != 0)
            {
                q["limit"] = o.Limit.ToString(System.Globalization.CultureInfo.InvariantCulture);
            }
            if (!string.IsNullOrEmpty(o.Cursor))
            {
                q["cursor"] = o.Cursor;
            }
        }
        return q;
    }
}
