using System;
using System.Net.Http;
using System.Threading;
using Microsoft.Extensions.DependencyInjection.Extensions;
using Microsoft.Extensions.Options;
using Vunly;

namespace Microsoft.Extensions.DependencyInjection;

/// <summary>Registers the Vunly client.</summary>
public static class VunlyServiceCollectionExtensions
{
    /// <summary>The name of the <see cref="IHttpClientFactory"/> client the Vunly client uses.</summary>
    public const string HttpClientName = "Vunly";

    /// <summary>
    /// Registers a singleton <see cref="VunlyClient"/> built from <see cref="VunlyClientOptions"/>, over an
    /// <see cref="IHttpClientFactory"/> client that never follows redirects. Options are validated when the host starts, so a
    /// missing key or address stops the application at boot, not on the first billing call.
    /// </summary>
    /// <remarks>
    /// Bind from configuration with <c>services.Configure&lt;VunlyClientOptions&gt;(configuration.GetSection("Vunly"))</c>
    /// (environment variables <c>Vunly__ApiKey</c> and <c>Vunly__BaseUrl</c> work), or set them in <paramref name="configure"/>.
    /// Read the key from a secret store, never from source.
    /// </remarks>
    public static IServiceCollection AddVunly(this IServiceCollection services, Action<VunlyClientOptions>? configure = null)
    {
        ArgumentNullException.ThrowIfNull(services);
        var options = services.AddOptions<VunlyClientOptions>();
        if (configure is not null)
        {
            options.Configure(configure);
        }
        options
            .Validate(o => !string.IsNullOrWhiteSpace(o.ApiKey), "Vunly: ApiKey is not set (read it from your secret store)")
            .Validate(o => !string.IsNullOrWhiteSpace(o.BaseUrl), "Vunly: BaseUrl is not set (the API address from the setup step)")
            .ValidateOnStart();
        // the same rules the client applies when it is built (key characters, a BaseUrl that is an http(s) address without
        // credentials, query or fragment, Timeout and MaxRetries in range), so a bad value stops the host at boot; the text
        // names the setting, never its value
        services.TryAddEnumerable(ServiceDescriptor.Singleton<IValidateOptions<VunlyClientOptions>, SettingsValidator>());

        services
            .AddHttpClient(HttpClientName)
            // the per-attempt timeout is the Vunly client's own
            .ConfigureHttpClient(c => c.Timeout = Timeout.InfiniteTimeSpan)
            // The HttpClient logging handlers (category System.Net.Http.HttpClient.Vunly.*) write request headers at Trace level, and
            // on .NET 8 the Authorization header, with the key, was among them. Authorization is the only secret the SDK sets.
            .RedactLoggedHeaders(new[] { "Authorization" })
            .ConfigurePrimaryHttpMessageHandler(() => new SocketsHttpHandler
            {
                AllowAutoRedirect = false, // a redirect would carry the Authorization header to another host
                MaxResponseHeadersLength = VunlyClient.MaxResponseHeadersKb, // 64 KB: explicit, not a default
                PooledConnectionLifetime = TimeSpan.FromMinutes(2),
            })
            // one long-lived client: the handler's own connection lifetime handles DNS changes
            .SetHandlerLifetime(Timeout.InfiniteTimeSpan);

        services.TryAddSingleton(sp =>
            new VunlyClient(
                sp.GetRequiredService<IOptions<VunlyClientOptions>>().Value,
                sp.GetRequiredService<IHttpClientFactory>().CreateClient(HttpClientName)));
        return services;
    }
}

internal sealed class SettingsValidator : IValidateOptions<VunlyClientOptions>
{
    public ValidateOptionsResult Validate(string? name, VunlyClientOptions options)
    {
        // an unset key or address is reported by the two checks in AddVunly, with their own text
        if (string.IsNullOrWhiteSpace(options.ApiKey) || string.IsNullOrWhiteSpace(options.BaseUrl))
        {
            return ValidateOptionsResult.Success;
        }
        return VunlyClient.Problem(options) is { } problem ? ValidateOptionsResult.Fail(problem) : ValidateOptionsResult.Success;
    }
}
