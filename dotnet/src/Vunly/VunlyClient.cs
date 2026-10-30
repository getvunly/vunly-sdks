using System;
using System.Net.Http;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;

namespace Vunly;

/// <summary>
/// Configures a <see cref="VunlyClient"/>. <see cref="ApiKey"/> and <see cref="BaseUrl"/> are required. Neither is read from
/// the environment or from disk by the SDK.
/// </summary>
public sealed class VunlyClientOptions
{
    /// <summary>
    /// The sandbox key (vk_test_...). Read it from your configuration or secret store; never put it in source. One final
    /// line break (LF or CRLF, as a secret read from a file has) is ignored; any other space, tab, NUL, lone CR, control or
    /// non-ASCII character, at either end or inside, is refused.
    /// </summary>
    public string? ApiKey { get; set; }

    /// <summary>
    /// The API address from the setup step, for example <c>https://host</c>. A trailing slash or /v1 is accepted. Credentials
    /// (<c>user:password@</c>), a query string and a fragment are refused.
    /// </summary>
    public string? BaseUrl { get; set; }

    /// <summary>The limit for ONE attempt, answer included (default 10 s; above zero, at most 10 minutes).</summary>
    public TimeSpan Timeout { get; set; } = TimeSpan.FromSeconds(10);

    /// <summary>Retries after a lost answer, timeout, 429 or 5xx, by operation class (default 2; 0 turns retries off; at most 10).</summary>
    public int MaxRetries { get; set; } = 2;
}

/// <summary>
/// The Vunly client. Thread-safe: create one and share it (a singleton in dependency injection). Every call is asynchronous and
/// takes a <see cref="CancellationToken"/>. No global state.
/// </summary>
/// <example>
/// <code>
/// var vunly = new VunlyClient(new VunlyClientOptions { ApiKey = key, BaseUrl = url });
/// var result = await vunly.Usage.RecordAsync("req_2026_0001", "cus_demo_01", "tokens", 1050, ct);
/// </code>
/// </example>
public sealed class VunlyClient : IDisposable
{
    /// <summary>The SDK version, also sent in the User-Agent.</summary>
    public const string Version = "0.1.0";

    private static readonly Regex VersionSuffix = new("/v1\\z", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);

    private readonly string _baseUrl;
    private readonly HttpClient? _ownedHttp;

    /// <summary>Creates a client with its own <see cref="HttpClient"/> (it never follows redirects).</summary>
    public VunlyClient(VunlyClientOptions options)
        : this(options, null)
    {
    }

    /// <summary>
    /// Creates a client over your own <see cref="HttpClient"/> (a typed client from <c>IHttpClientFactory</c>, a proxy, a test
    /// handler). Its handler must not follow redirects: a redirect would carry the Authorization header to another host. If it
    /// does follow one anyway, the SDK refuses the answer of any host other than the configured one
    /// (<c>unexpected_redirect</c>), but the request with the key has already been sent there: configure the handler
    /// correctly. The client's own <see cref="HttpClient.Timeout"/> should be at least <see cref="VunlyClientOptions.Timeout"/>.
    /// With <see cref="VunlyClientOptions.MaxRetries"/> 0 a request without a body carries an empty one (Content-Length: 0),
    /// because <c>SocketsHttpHandler</c> repeats a body-less request up to 3 times by itself when a connection closes before an
    /// answer and never one with a body; a handler of your own may still replay.
    /// </summary>
    public VunlyClient(VunlyClientOptions options, HttpClient? httpClient)
        : this(options, httpClient, null)
    {
    }

    internal VunlyClient(VunlyClientOptions options, HttpClient? httpClient, Func<TimeSpan, CancellationToken, Task>? delay)
    {
        ArgumentNullException.ThrowIfNull(options);
        if (Problem(options) is { } problem)
        {
            throw new ArgumentException(problem, nameof(options));
        }
        _baseUrl = NormalizeBaseUrl(options.BaseUrl!);
        var uri = new Uri(_baseUrl, UriKind.Absolute);
        if (httpClient is null)
        {
            _ownedHttp = new HttpClient(new SocketsHttpHandler { AllowAutoRedirect = false, PooledConnectionLifetime = TimeSpan.FromMinutes(2), MaxResponseHeadersLength = MaxResponseHeadersKb })
            {
                Timeout = System.Threading.Timeout.InfiniteTimeSpan, // the per-attempt timeout is ours
            };
            httpClient = _ownedHttp;
        }
        var transport = new Transport(httpClient, _baseUrl, uri, CleanKey(options.ApiKey!), options.Timeout, options.MaxRetries)
        {
            Delay = delay ?? ((d, ct) => Task.Delay(d, ct)),
        };
        Customers = new CustomersApi(transport);
        Credits = new CreditsApi(transport);
        Usage = new UsageApi(transport);
        Entitlements = new EntitlementsApi(transport);
        Plans = new PlansApi(transport);
        CustomerTokens = new CustomerTokensApi(transport);
        _transport = transport;
    }

    private readonly Transport _transport;

    /// <summary>Customers.</summary>
    public CustomersApi Customers { get; }

    /// <summary>Credits: grants, balances and the ledger.</summary>
    public CreditsApi Credits { get; }

    /// <summary>Usage events.</summary>
    public UsageApi Usage { get; }

    /// <summary>Entitlement checks.</summary>
    public EntitlementsApi Entitlements { get; }

    /// <summary>Meters, plans and subscriptions.</summary>
    public PlansApi Plans { get; }

    /// <summary>Short-lived read tokens for one customer.</summary>
    public CustomerTokensApi CustomerTokens { get; }

    /// <summary>
    /// The project's plan and this month's recorded usage. Its limits are inside <see cref="PlanUsage.UnconfirmedLimits"/> and
    /// are not a guarantee (<c>Confirmed</c> is false).
    /// </summary>
    public async Task<PlanUsage> GetPlanUsageAsync(CancellationToken cancellationToken = default) =>
        (await _transport.SendAsync(Transport.Get("/v1/plan-usage", null), PlanUsage.From, cancellationToken).ConfigureAwait(false)).Value;

    /// <summary>The key as it is sent: ONE final LF or CRLF (a secret read from a file) is removed, nothing else.</summary>
    internal static string CleanKey(string key) =>
        key.EndsWith("\r\n", StringComparison.Ordinal) ? key[..^2] : key.EndsWith('\n') ? key[..^1] : key;

    /// <summary>
    /// The most a response head (status line and all headers) may be, in KB: the handler's default, stated so the bound does not
    /// depend on a default. A longer head fails the call as <c>unavailable</c>. An <see cref="HttpClient"/> you supply keeps its
    /// own value.
    /// </summary>
    internal const int MaxResponseHeadersKb = 64;

    /// <summary>The longest <see cref="VunlyClientOptions.Timeout"/> accepted for one attempt.</summary>
    internal static readonly TimeSpan MaxTimeout = TimeSpan.FromMinutes(10);

    /// <summary>The most <see cref="VunlyClientOptions.MaxRetries"/> accepted.</summary>
    internal const int MaxMaxRetries = 10;

    /// <summary>
    /// Why these options cannot make a client, or null when they can. The text names the setting, never its value (a key or an
    /// address with credentials must not end up in a log). Also used by the dependency-injection package at host start.
    /// </summary>
    internal static string? Problem(VunlyClientOptions options)
    {
        if (string.IsNullOrWhiteSpace(options.ApiKey))
        {
            return "vunly: the API key is empty (read it from your configuration)";
        }
        foreach (var c in CleanKey(options.ApiKey))
        {
            if (c is < '!' or > '~')
            {
                return "vunly: ApiKey contains a space, a tab, a control character or a non-ASCII character (only a final line break is ignored); it is sent as an HTTP header (check how it was read from your configuration)";
            }
        }
        if (string.IsNullOrWhiteSpace(options.BaseUrl))
        {
            return "vunly: the base URL is empty (set BaseUrl to the API address from the setup step)";
        }
        if (options.BaseUrl.Contains('?', StringComparison.Ordinal) || options.BaseUrl.Contains('#', StringComparison.Ordinal))
        {
            return "vunly: BaseUrl must be the address only: no query string and no fragment";
        }
        var normalized = NormalizeBaseUrl(options.BaseUrl);
        if (!Uri.TryCreate(normalized, UriKind.Absolute, out var uri) || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
        {
            return "vunly: BaseUrl must be an absolute http(s) address such as https://host";
        }
        var authorityEnd = normalized.IndexOf('/', uri.Scheme.Length + 3);
        var authority = authorityEnd < 0 ? normalized[(uri.Scheme.Length + 3)..] : normalized[(uri.Scheme.Length + 3)..authorityEnd];
        if (uri.UserInfo.Length > 0 || authority.Contains('@', StringComparison.Ordinal))
        {
            return "vunly: BaseUrl must not contain credentials (user:password@); the key is set in ApiKey";
        }
        if (options.MaxRetries is < 0 or > MaxMaxRetries)
        {
            return $"vunly: MaxRetries must be between 0 and {MaxMaxRetries}";
        }
        if (options.Timeout <= TimeSpan.Zero || options.Timeout > MaxTimeout)
        {
            return $"vunly: Timeout must be positive and at most {MaxTimeout.TotalMinutes:0} minutes (it limits ONE attempt)";
        }
        return null;
    }

    /// <summary>Never contains the key; the address never contains credentials (they are refused at construction).</summary>
    public override string ToString() => $"VunlyClient {{ BaseUrl = {_baseUrl}, ApiKey = <hidden> }}";

    /// <summary>The address without a trailing slash and without a trailing <c>/v1</c>: the SDK adds <c>/v1/...</c> itself.</summary>
    public static string NormalizeBaseUrl(string url)
    {
        var s = url.Trim().TrimEnd('/');
        s = VersionSuffix.Replace(s, "");
        return s.TrimEnd('/');
    }

    /// <summary>Releases the <see cref="HttpClient"/> the client created for itself; one you passed in is yours.</summary>
    public void Dispose() => _ownedHttp?.Dispose();
}
