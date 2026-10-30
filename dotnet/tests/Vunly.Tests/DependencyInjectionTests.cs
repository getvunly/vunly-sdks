using System;
using System.Net.Http;
using System.Threading.Tasks;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Options;
using Xunit;

namespace Vunly.Tests;

public class DependencyInjectionTests
{
    [Fact]
    public void AddVunlyRegistersOneSharedClient()
    {
        var services = new ServiceCollection();
        services.AddVunly(o =>
        {
            o.ApiKey = TestServer.Key;
            o.BaseUrl = "http://127.0.0.1:1/v1/";
        });
        using var provider = services.BuildServiceProvider();
        var a = provider.GetRequiredService<VunlyClient>();
        var b = provider.GetRequiredService<VunlyClient>();
        Assert.Same(a, b);
        Assert.DoesNotContain(TestServer.Key, a.ToString());
    }

    [Fact]
    public void AMissingKeyIsRefusedWhenTheOptionsAreFirstRead()
    {
        var services = new ServiceCollection();
        services.AddVunly(o => o.BaseUrl = "http://127.0.0.1:1");
        using var provider = services.BuildServiceProvider();
        var e = Assert.Throws<OptionsValidationException>(() => provider.GetRequiredService<VunlyClient>());
        Assert.Contains("ApiKey", string.Join(" ", e.Failures));
    }

    [Fact]
    public void ValidationAlsoRunsAtHostStartup()
    {
        var services = new ServiceCollection();
        services.AddVunly(o => o.ApiKey = TestServer.Key); // no address
        using var provider = services.BuildServiceProvider();
        // the generic host calls IStartupValidator before it starts serving: a bad configuration stops the application at boot
        var e = Assert.Throws<OptionsValidationException>(() => provider.GetRequiredService<IStartupValidator>().Validate());
        Assert.Contains("BaseUrl", string.Join(" ", e.Failures));
    }

    private sealed class Capture : Microsoft.Extensions.Logging.ILoggerProvider
    {
        internal readonly System.Collections.Concurrent.ConcurrentQueue<string> Lines = new();

        public Microsoft.Extensions.Logging.ILogger CreateLogger(string categoryName) => new Sink(this, categoryName);

        public void Dispose()
        {
        }

        private sealed class Sink : Microsoft.Extensions.Logging.ILogger
        {
            private readonly Capture _owner;
            private readonly string _category;

            internal Sink(Capture owner, string category)
            {
                _owner = owner;
                _category = category;
            }

            public IDisposable? BeginScope<TState>(TState state)
                where TState : notnull => null;

            public bool IsEnabled(Microsoft.Extensions.Logging.LogLevel logLevel) => true;

            public void Log<TState>(Microsoft.Extensions.Logging.LogLevel logLevel, Microsoft.Extensions.Logging.EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter)
            {
                // the formatted text AND every structured value (the headers are logged as state, not only in the text)
                var text = new System.Text.StringBuilder(_category).Append(' ').Append(formatter(state, exception));
                if (state is System.Collections.IEnumerable pairs)
                {
                    foreach (var p in pairs)
                    {
                        text.Append(' ').Append(p);
                    }
                }
                _owner.Lines.Enqueue(text.ToString());
            }
        }
    }

    /// <summary>
    /// On .NET 8 the HttpClient logging handlers wrote "Authorization: Bearer vk_..." into the Trace log of the named client. The key
    /// must never reach a log, at the most verbose level, on either runtime.
    /// </summary>
    [Fact]
    public async Task TheKeyNeverReachesTheHttpClientLogAtTraceLevel()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"credits\":5,\"consumed\":1,\"granted\":6}"));
        var capture = new Capture();
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(Microsoft.Extensions.Logging.LogLevel.Trace).AddProvider(capture));
        services.AddVunly(o =>
        {
            o.ApiKey = TestServer.Key;
            o.BaseUrl = s.Url;
            o.MaxRetries = 0;
        });
        using var provider = services.BuildServiceProvider();
        await provider.GetRequiredService<VunlyClient>().Credits.GetBalanceAsync("acme");

        var lines = capture.Lines.ToArray();
        // the control: the handlers did log this request, headers included, so an empty log cannot make the test pass
        Assert.Contains(lines, l => l.Contains("System.Net.Http.HttpClient.Vunly", StringComparison.Ordinal) && l.Contains("Authorization", StringComparison.Ordinal));
        Assert.DoesNotContain(lines, l => l.Contains(TestServer.Key, StringComparison.Ordinal));
        Assert.DoesNotContain(lines, l => l.Contains("vk_test_", StringComparison.Ordinal));
    }

    [Fact]
    public async Task TheRegisteredClientDoesNotFollowRedirects()
    {
        var followed = 0;
        using var target = new TestServer(ctx =>
        {
            System.Threading.Interlocked.Increment(ref followed);
            return Task.CompletedTask;
        });
        using var s = new TestServer(ctx =>
        {
            ctx.Response.StatusCode = 302;
            ctx.Response.Headers["Location"] = target.Url + "/x";
            return Task.CompletedTask;
        });
        var services = new ServiceCollection();
        services.AddVunly(o =>
        {
            o.ApiKey = TestServer.Key;
            o.BaseUrl = s.Url;
            o.MaxRetries = 0;
        });
        using var provider = services.BuildServiceProvider();
        await Assert.ThrowsAnyAsync<VunlyException>(() => provider.GetRequiredService<VunlyClient>().Credits.GetBalanceAsync("acme"));
        Assert.Equal(0, followed);
        // the factory client has no overall timeout of its own: the per-attempt timeout is the Vunly client's
        Assert.Equal(System.Threading.Timeout.InfiniteTimeSpan, provider.GetRequiredService<IHttpClientFactory>().CreateClient(VunlyServiceCollectionExtensions.HttpClientName).Timeout);
    }
}
